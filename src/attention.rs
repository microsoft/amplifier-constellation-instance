//! Optional installation-owned ambient event admission. Ordinary ACP is unchanged.
use crate::{
    attention_policy::{self, Context},
    state::{AppState, now_ms},
};
use anyhow::{Context as _, Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};

pub const META: &str = "com.microsoft.amplifier.attention";
pub const METHOD: &str = "_amplifier/attention";
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub endpoint: String,
    pub model: String,
    /// system_one (JEV / Microsoft Decision) or openai_decisions.
    pub protocol: String,
    /// bearer or api-key. The credential is an installation secret, never adapter data.
    pub auth: String,
    pub credential_secret: String,
    #[serde(default = "decision_limit")]
    pub decisions_per_hour: usize,
    #[serde(default = "turn_limit")]
    pub turns_per_hour: usize,
    #[serde(default = "uncertain_limit")]
    pub uncertain_turns_per_hour: usize,
}
fn decision_limit() -> usize {
    600
}
fn turn_limit() -> usize {
    60
}
fn uncertain_limit() -> usize {
    12
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        let u = reqwest::Url::parse(&self.endpoint)?;
        ensure!(
            (u.scheme() == "https"
                || (u.scheme() == "http"
                    && matches!(u.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"))))
                && u.username().is_empty()
                && u.password().is_none()
                && u.query().is_none()
                && u.fragment().is_none(),
            "Attention requires HTTPS or loopback without URL credentials"
        );
        ensure!(
            ["system_one", "openai_decisions"].contains(&self.protocol.as_str())
                && ["bearer", "api-key"].contains(&self.auth.as_str()),
            "Unknown attention protocol or authentication"
        );
        ensure!(
            crate::config::valid_name(&self.credential_secret)
                && !self.model.is_empty()
                && self.model.len() <= 200,
            "Set an attention model and secret reference"
        );
        ensure!(
            (1..=10000).contains(&self.decisions_per_hour)
                && (1..=1000).contains(&self.turns_per_hour)
                && self.uncertain_turns_per_hour <= self.turns_per_hour,
            "Invalid attention budgets"
        );
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Event {
    pub session_id: String,
    /// Original source event timestamp; retries must not refresh it.
    pub occurred_at: i64,
    pub context: Context,
}
pub fn capability(state: &AppState) -> Value {
    json!({"version":1,"enabled":state.instance_config().attention.is_some(),"method":METHOD,"promptMetadata":META,"scope":"ambient_conversation","uncertain":"bounded_engage","ticketLifetimeMs":30000})
}
fn key(parts: Value) -> String {
    format!("{:x}", Sha256::digest(parts.to_string().as_bytes()))
}
fn policy_key(config: &crate::config::InstanceConfig) -> String {
    key(json!([config.attention, config.name, config.purpose]))
}
fn fresh(at: i64) -> bool {
    at <= now_ms() + 5000 && at >= now_ms() - 600_000
}
fn rows(state: &AppState) -> Result<Vec<Value>> {
    let db = state.db.lock().unwrap();
    let mut q=db.prepare("SELECT value FROM records WHERE collection='attention_events' AND updated_at>=?1 ORDER BY updated_at DESC")?;
    let values = q
        .query_map([now_ms() - 86400000], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    values
        .into_iter()
        .map(|s| Ok(serde_json::from_str(&s)?))
        .collect()
}
fn save(state: &AppState, row: &Value) -> Result<()> {
    state.set_record("attention_events", row["id"].as_str().unwrap(), row.clone())
}
fn head(state: &AppState, row: &Value) -> Result<bool> {
    Ok(state
        .get_record("attention_heads", row["audience"].as_str().unwrap())?
        .is_some_and(|v| v["id"] == row["id"]))
}

/// Reserve one event before network I/O. Call only after verifying the loaded ACP session.
pub fn reserve(
    state: &AppState,
    principal: &str,
    connection: &str,
    event: &Event,
    binding: Option<&Value>,
) -> Result<Value> {
    attention_policy::validate(&event.context)?;
    let installed = state.instance_config();
    let config = installed
        .attention
        .as_ref()
        .context("Attention is not configured")?;
    config.validate()?;
    ensure!(
        event.session_id.len() <= 512 && !event.session_id.is_empty(),
        "Missing session"
    );
    if let Some(b) = binding.filter(|b| b["conversation_id"].is_string()) {
        ensure!(
            b["conversation_id"] == event.context.conversation_id
                && b["thread_id"].as_str().unwrap_or("") == event.context.thread_id,
            "Attention evidence does not match this session's verified audience"
        );
    }
    let _lock = state.attention_lock.lock().unwrap();
    // Retain deduplication longer than the maximum event age. No conversation content in the journal.
    state.db.lock().unwrap().execute("DELETE FROM records WHERE collection IN ('attention_events','attention_heads') AND updated_at < ?1",[now_ms()-86_400_000])?;
    let audience = key(json!([
        principal,
        binding.and_then(|b| b.get("provider_id")),
        event.context.conversation_id,
        event.context.thread_id
    ]));
    let id = key(json!([audience, event.context.trigger_id]));
    if let Some(mut prior) = state.get_record("attention_events", &id)? {
        if prior["evidence_sha256"] != key(json!(event.context))
            && matches!(prior["status"].as_str(), Some("evaluating" | "offered"))
        {
            prior["status"] = json!("invalidated");
            save(state, &prior)?;
        }
        return Ok(json!({"action":"hold","reason":"duplicate","id":id}));
    }
    if !fresh(event.occurred_at) {
        return Ok(json!({"action":"hold","reason":"stale","id":id}));
    }
    if let Some(latest) = state.get_record("attention_heads", &audience)?
        && latest["occurred_at"].as_i64().unwrap_or(0) > event.occurred_at
    {
        return Ok(json!({"action":"hold","reason":"superseded","id":id}));
    }
    let attempts = rows(state)?
        .iter()
        .filter(|r| r["requested_at"].as_i64().unwrap_or(0) > now_ms() - 3600000)
        .count();
    if attempts >= config.decisions_per_hour {
        state.set_record(
            "attention_heads",
            &audience,
            json!({"id":id,"occurred_at":event.occurred_at}),
        )?;
        return Ok(json!({"action":"hold","reason":"decision_budget","id":id}));
    }
    let status = "evaluating";
    let row = json!({"id":id,"audience":audience,"connection":connection,"principal":principal,"session_id":event.session_id,"event_id":event.context.trigger_id,"occurred_at":event.occurred_at,"requested_at":now_ms(),"status":status,"reason":status,"evidence_sha256":key(json!(event.context)),"review":"unreviewed","config_sha256":policy_key(&installed)});
    save(state, &row)?;
    state.set_record(
        "attention_heads",
        &audience,
        json!({"id":id,"occurred_at":event.occurred_at}),
    )?;
    Ok(row)
}

async fn query(
    state: &AppState,
    installed: &crate::config::InstanceConfig,
    config: &Config,
    context: &Context,
) -> Result<Value> {
    let mut evidence = json!(context);
    evidence["installed_purpose"] = json!(installed.purpose);
    let instructions = attention_policy::instructions();
    let choices = attention_policy::choices();
    let body = if config.protocol == "openai_decisions" {
        json!({"model":config.model,"input":[{"role":"user","content":[{"type":"input_json","data":evidence}]}],"questions":[{"name":"participation","type":"choice","instructions":instructions,"choices":choices.as_object().unwrap().iter().map(|(value,description)|json!({"value":value,"description":description})).collect::<Vec<_>>()}]})
    } else {
        json!({"model":config.model,"state":evidence,"questions":{"participation":{"type":"choice","instructions":instructions,"criteria":choices}}})
    };
    let secret = state.read_secret(&config.credential_secret)?;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(8))
        .build()?;
    let req = http.post(&config.endpoint).json(&body);
    let req = if config.auth == "api-key" {
        req.header("api-key", secret.trim())
    } else {
        req.bearer_auth(secret.trim())
    };
    let mut response = req.send().await?;
    ensure!(response.status().is_success(), "Attention provider failed");
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 65536,
            "Attention response exceeds 64 KiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// Evaluations create short-lived offers, never agent turns or game authority.
pub async fn evaluate(state: Arc<AppState>, mut row: Value, event: Event) -> Result<Value> {
    if row["status"] != "evaluating" {
        return Ok(json!({"action":"hold","reason":row["reason"],"id":row["id"]}));
    }
    let installed = state.instance_config();
    let config = installed.attention.as_ref().context("Attention disabled")?;
    state.event("attention.requested", json!({"id":row["id"]}))?;
    let start = now_ms();
    let result = query(&state, &installed, config, &event.context)
        .await
        .and_then(|v| {
            let answer = attention_policy::normalize(&v)?;
            row["probabilities"] = if v["answers"].is_array() {
                v["answers"][0]["probabilities"].clone()
            } else {
                v["answers"]["participation"]["probabilities"].clone()
            };
            Ok(answer)
        });
    let (judgment, reason, engage, investigate) = match result {
        Ok(v) => v,
        // A fresh event gets at most one bounded opportunity during an outage, never repeated polling.
        Err(_) => ("unavailable".into(), "provider_error".into(), true, true),
    };
    let _lock = state.attention_lock.lock().unwrap();
    let current = state
        .get_record("attention_events", row["id"].as_str().unwrap())?
        .context("Event no longer exists")?;
    let valid = current["status"] == "evaluating"
        && head(&state, &row)?
        && fresh(event.occurred_at)
        && state.enabled()
        && row["config_sha256"] == policy_key(&installed)
        && row["config_sha256"] == policy_key(&state.instance_config());
    row["judgment"] = json!(judgment);
    row["reason"] = json!(reason);
    row["investigate"] = json!(investigate);
    row["elapsed_ms"] = json!(now_ms() - start);
    row["status"] = json!(if !valid {
        "invalidated"
    } else if engage {
        "offered"
    } else {
        "held"
    });
    let ticket = uuid::Uuid::new_v4().to_string();
    row["ticket_hash"] = json!(key(json!(ticket)));
    row["expires_at"] = json!(now_ms() + 30000);
    save(&state, &row)?;
    state.event("attention.evaluated",json!({"id":row["id"],"judgment":row["judgment"],"reason":row["reason"],"status":row["status"],"investigate":investigate}))?;
    if row["status"] == "offered" {
        Ok(
            json!({"action":"engage","reason":reason,"id":row["id"],"ticket":ticket,"expiresAt":row["expires_at"]}),
        )
    } else {
        Ok(
            json!({"action":"hold","reason":if valid {reason.as_str()}else{"invalidated"},"id":row["id"]}),
        )
    }
}

/// Consumed immediately before forwarding the ordinary ACP prompt. Never grant tool authority.
pub fn consume(
    state: &AppState,
    principal: &str,
    connection: &str,
    sid: &str,
    metadata: &Value,
) -> Result<String> {
    let id = metadata["id"]
        .as_str()
        .context("Missing attention event id")?;
    let ticket = metadata["ticket"]
        .as_str()
        .context("Missing attention ticket")?;
    let installed = state.instance_config();
    let config = installed.attention.as_ref().context("Attention disabled")?;
    let _lock = state.attention_lock.lock().unwrap();
    let mut row = state
        .get_record("attention_events", id)?
        .context("Unknown attention event")?;
    ensure!(
        row["principal"] == principal
            && row["connection"] == connection
            && row["session_id"] == sid
            && row["ticket_hash"] == key(json!(ticket)),
        "Attention ticket does not belong to this connection and session"
    );
    ensure!(
        row["status"] == "offered"
            && row["expires_at"].as_i64().unwrap_or(0) > now_ms()
            && head(state, &row)?
            && fresh(row["occurred_at"].as_i64().unwrap_or(0)),
        "Attention ticket is stale, superseded, cancelled, or already used"
    );
    ensure!(
        row["config_sha256"] == policy_key(&installed),
        "Attention settings changed; old offers are invalid"
    );
    let recent: Vec<_> = rows(state)?
        .into_iter()
        .filter(|r| r["claimed_at"].as_i64().unwrap_or(0) > now_ms() - 3600000)
        .collect();
    let uncertain = recent.iter().filter(|r| r["investigate"] == true).count();
    if recent.len() >= config.turns_per_hour
        || (row["investigate"] == true && uncertain >= config.uncertain_turns_per_hour)
    {
        row["status"] = json!("budget_hold");
        save(state, &row)?;
        anyhow::bail!("Attention turn budget reached");
    }
    row["status"] = json!("claimed");
    row["claimed_at"] = json!(now_ms());
    save(state, &row)?;
    Ok(id.into())
}
pub fn forwarded(state: &AppState, id: &str) -> Result<()> {
    let _lock = state.attention_lock.lock().unwrap();
    if let Some(mut row) = state.get_record("attention_events", id)? {
        row["status"] = json!("admitted");
        row["admitted_at"] = json!(now_ms());
        save(state, &row)?;
    }
    state.event("attention.admitted",json!({"id":id,"meaning":"ACP prompt forwarded; not a model request count or publication receipt"}))?;
    Ok(())
}
pub fn cancel(state: &AppState, connection: &str, sid: Option<&str>) {
    let _lock = state.attention_lock.lock().unwrap();
    if let Ok(rows) = rows(state) {
        for mut row in rows {
            if row["connection"] == connection
                && sid.is_none_or(|s| row["session_id"] == s)
                && matches!(row["status"].as_str(), Some("evaluating" | "offered"))
            {
                row["status"] = json!("cancelled");
                let _ = save(state, &row);
            }
        }
    }
}
pub fn report(state: &AppState) -> Result<Value> {
    let rows = rows(state)?;
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for r in &rows {
        for (key, value) in [
            ("status", r["status"].as_str()),
            ("reason", r["reason"].as_str()),
        ] {
            if let Some(v) = value {
                *counts.entry(format!("{key}:{v}")).or_default() += 1;
            }
        }
    }
    let choices = rows
        .iter()
        .filter(|r| {
            matches!(
                r["judgment"].as_str(),
                Some("engage" | "hold" | "uncertain")
            )
        })
        .count();
    let uncertain = rows
        .iter()
        .filter(|r| r["reason"] == "semantic_uncertainty")
        .count();
    let reviews:Vec<_>=rows.iter().filter(|r|r["investigate"]==true).map(|r|json!({"id":r["id"],"event_id":r["event_id"],"session_id":r["session_id"],"judgment":r["judgment"],"reason":r["reason"],"status":r["status"],"elapsed_ms":r["elapsed_ms"],"evidence_sha256":r["evidence_sha256"],"probabilities":r["probabilities"],"review":r["review"]})).collect();
    Ok(
        json!({"capability":capability(state),"counts":counts,"semantic_uncertainty":{"count":uncertain,"choice_evaluations":choices,"rate":if choices==0 {None}else{Some(uncertain as f64/choices as f64)}},"window":"Retained events from the last 24 hours","reviews":reviews,"note":"Offers and admitted turns are separate from model requests and useful replies. These counts do not establish cost savings."}),
    )
}
