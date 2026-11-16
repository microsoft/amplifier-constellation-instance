//! Installed-agent approval policy. The engine remains the execution authority.
//! An authorization is durably recorded before forwarding; a lost response is
//! never replayed. Remembered rules are semantic engine attestations, not prose
//! or arguments interpreted by this host.
use crate::{
    auth,
    config::BackendConfig,
    error::{ApiError, ApiResult},
    state::{AppState, now_ms},
};
use anyhow::{Context, Result, bail, ensure};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};

pub const META: &str = "com.microsoft.amplifier.approval-policy";
pub const CAPABILITY: &str = "agent-instance.approval-policy/1";
pub const REVOCATION_NOTICE: &str = "Future matching requests will ask again; already approved work or conversation grants can remain valid until completed/stopped/revoked in engine.";
const MAX_RULES: usize = 256;
const MAX_CLIENTS: usize = 128;

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Ask,
    Automatic,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    revision: u64,
    mode: Mode,
    delegated_clients: Vec<String>,
    rules: Vec<Rule>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    id: String,
    revision: u64,
    status: String,
    backend: String,
    producer: String,
    key: String,
    description: String,
    authority_lifetime: String,
    creator: String,
    created_at: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    expected_revision: u64,
    mode: Mode,
    delegated_clients: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revoke {
    expected_revision: u64,
}

fn load(db: &Connection) -> Result<Policy> {
    let row: Option<String> = db
        .query_row(
            "SELECT value FROM records WHERE collection='approval_policy' AND id='policy'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    row.map(|s| Ok(serde_json::from_str(&s)?))
        .unwrap_or_else(|| Ok(Policy::default()))
}
fn save(db: &Connection, policy: &Policy) -> Result<()> {
    db.execute("INSERT INTO records(collection,id,value,updated_at) VALUES('approval_policy','policy',?1,?2) ON CONFLICT(collection,id) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at", params![serde_json::to_string(policy)?,now_ms()])?;
    Ok(())
}
fn valid_client(db: &Connection, id: &str) -> Result<bool> {
    let scopes: Option<String> = db
        .query_row(
            "SELECT scopes FROM tokens WHERE id=?1 AND expires_at>?2",
            params![id, now_ms()],
            |r| r.get(0),
        )
        .optional()?;
    Ok(scopes
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .is_some_and(|s| s.iter().any(|s| s == "acp" || s == "admin")))
}
fn current_actor(state: &AppState, headers: Option<&HeaderMap>) -> Option<String> {
    let actor = auth::check(state, headers?, "acp", false).ok()?.actor;
    actor.strip_prefix("token:").filter(|s| !s.is_empty())?;
    Some(actor)
}
fn delegated(db: &Connection, policy: &Policy, actor: &str) -> Result<bool> {
    let Some(id) = actor.strip_prefix("token:") else {
        return Ok(false);
    };
    Ok(policy.delegated_clients.iter().any(|v| v == id) && valid_client(db, id)?)
}
/// Only a digest is persisted. Secret values are part of the binding, so rotating
/// a selected backend credential invalidates old rules without exposing it.
pub fn backend_fingerprint(state: &AppState, backend: &BackendConfig) -> Result<String> {
    let secrets = backend
        .secret_env
        .iter()
        .map(|(name, reference)| Ok((name.clone(), state.read_secret(reference)?)))
        .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
    fingerprint_with_secrets(backend, &secrets)
}
/// Hash exactly the values supplied to Command, not a second read after spawn.
pub fn fingerprint_with_secrets(
    backend: &BackendConfig,
    secrets: &std::collections::BTreeMap<String, String>,
) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(b"instance-approval-backend-v1\0");
    hash.update(serde_json::to_vec(backend)?);
    for (name, value) in secrets {
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name.as_bytes());
        hash.update(Sha256::digest(value.as_bytes()));
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn selected_backend(state: &AppState) -> Result<String> {
    let b = state
        .instance_config()
        .backend
        .ok_or_else(|| anyhow::anyhow!("No backend"))?;
    backend_fingerprint(state, &b)
}
fn document(state: &AppState) -> Result<Value> {
    let backend = selected_backend(state).ok();
    let db = state.db.lock().unwrap();
    let policy = load(&db)?;
    let mut stmt = db.prepare(
        "SELECT id,name,expires_at,scopes FROM tokens WHERE expires_at>?1 ORDER BY name,id",
    )?;
    let rows = stmt
        .query_map([now_ms()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let clients:Vec<Value>=rows.into_iter().filter(|(_,_,_,s)|serde_json::from_str::<Vec<String>>(s).is_ok_and(|s|s.iter().any(|s|s=="acp"||s=="admin"))).map(|(id,name,expires_at,_)|json!({"delegated":policy.delegated_clients.contains(&id),"id":id,"name":name,"expires_at":expires_at})).collect();
    let rules:Vec<Value>=policy.rules.iter().map(|r|Ok(json!({"id":r.id,"revision":r.revision,"status":r.status,"usable":r.status=="active" && backend.as_deref()==Some(r.backend.as_str()) && delegated(&db,&policy,&r.creator)?,"producer":r.producer,"description":r.description,"authority_lifetime":r.authority_lifetime,"creator":r.creator,"created_at":r.created_at}))).collect::<Result<_>>()?;
    Ok(
        json!({"schema":CAPABILITY,"revision":policy.revision,"mode":policy.mode,"delegated_clients":policy.delegated_clients,"clients":clients,"rules":rules,"scope":"Across all chats with this agent","limits":{"max_rules":MAX_RULES,"max_delegated_clients":MAX_CLIENTS},"revocation_notice":REVOCATION_NOTICE,"automatic_notice":"Approves only the current action offered by the engine. Engine, resource, operating-system, login and background-work restrictions remain in force."}),
    )
}
pub async fn get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", false)?;
    Ok(Json(document(&state)?))
}
pub async fn put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(update): Json<Update>,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", true)?;
    if update.delegated_clients.len() > MAX_CLIENTS
        || update
            .delegated_clients
            .iter()
            .any(|id| id.is_empty() || id.len() > 128)
        || update
            .delegated_clients
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != update.delegated_clients.len()
    {
        return Err(ApiError::bad("Select distinct current ACP client IDs"));
    }
    {
        let mut db = state.db.lock().unwrap();
        let tx = db.transaction()?;
        let mut policy = load(&tx)?;
        if policy.revision != update.expected_revision {
            return Err(ApiError::conflict(json!({"revision":policy.revision})));
        }
        for id in &update.delegated_clients {
            if !valid_client(&tx, id)? {
                return Err(ApiError::bad(
                    "A selected ACP client is unavailable or expired",
                ));
            }
        }
        policy.revision = policy
            .revision
            .checked_add(1)
            .ok_or_else(|| ApiError::bad("Policy revision exhausted"))?;
        policy.mode = update.mode;
        policy.delegated_clients = update.delegated_clients;
        for rule in &mut policy.rules {
            if rule.status == "active"
                && rule
                    .creator
                    .strip_prefix("token:")
                    .is_none_or(|id| !policy.delegated_clients.iter().any(|c| c == id))
            {
                rule.status = "revoked".into();
                rule.revision = policy.revision;
            }
        }
        save(&tx, &policy)?;
        tx.commit()?;
    }
    Ok(Json(document(&state)?))
}
pub async fn revoke(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<Revoke>,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", true)?;
    {
        let mut db = state.db.lock().unwrap();
        let tx = db.transaction()?;
        let mut policy = load(&tx)?;
        if policy.revision != request.expected_revision {
            return Err(ApiError::conflict(json!({"revision":policy.revision})));
        }
        let next = policy
            .revision
            .checked_add(1)
            .ok_or_else(|| ApiError::bad("Policy revision exhausted"))?;
        let rule = policy
            .rules
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(ApiError::missing)?;
        rule.status = "revoked".into();
        rule.revision = next;
        policy.revision = next;
        save(&tx, &policy)?;
        tx.commit()?;
    }
    Ok(Json(document(&state)?))
}
fn bounded(value: &Value, max: usize) -> Option<String> {
    let s = value.as_str()?;
    (!s.is_empty() && s.len() <= max && !s.chars().any(char::is_control)).then(|| s.to_owned())
}
#[derive(Clone)]
pub struct Descriptor {
    producer: String,
    key: String,
    description: String,
    authority_lifetime: String,
}
#[derive(Clone)]
pub struct Offer {
    pub id: Value,
    pub frontend_id: String,
    pub session: String,
    pub prompt: String,
    /// Connection-local verified source lease; never persisted as a rule.
    pub source: Option<String>,
    revision: u64,
    actor: String,
    backend: String,
    allow: String,
    descriptor: Option<Descriptor>,
    always: Option<String>,
    original_options: BTreeSet<String>,
}
pub struct Bridge {
    backend: String,
    producer: Option<String>,
}
impl Bridge {
    pub fn from_launch(backend: String) -> Self {
        Self {
            backend,
            producer: None,
        }
    }
    pub fn check_live(&self, state: &AppState, headers: Option<&HeaderMap>) -> Result<()> {
        ensure!(
            state.enabled() && selected_backend(state)? == self.backend,
            "Selected backend changed; reconnect"
        );
        if let Some(headers) = headers {
            auth::check(state, headers, "acp", false)
                .map_err(|_| anyhow::anyhow!("ACP authorization is no longer valid"))?;
        }
        Ok(())
    }
    pub fn initialize_request(&self, state: &AppState, value: &mut Value) {
        if !value["params"]["_meta"].is_object() {
            value["params"]["_meta"] = json!({});
        }
        value["params"]["_meta"][META] = json!({"version":1,"owner":"instance","instanceId":state.instance_config().instance_id});
    }
    pub fn initialize_result(&mut self, value: &mut Value) {
        let profile = &value["result"]["_meta"][META];
        self.producer = if profile["version"] == 1
            && profile["owner"] == "instance"
            && profile["reusableRequests"] == true
        {
            bounded(&profile["producer"], 160)
        } else {
            None
        };
        // Do not advertise backend-owned remembering as an Instance guarantee.
        if self.producer.is_none()
            && let Some(meta) = value["result"]["_meta"].as_object_mut()
        {
            meta.remove(META);
        }
    }
    pub fn offer(
        &self,
        state: &AppState,
        headers: Option<&HeaderMap>,
        value: &Value,
        prompt: &str,
    ) -> Result<Option<Offer>> {
        self.check_live(state, headers)?;
        if headers.is_none() {
            return Ok(None);
        }
        let actor = current_actor(state, headers)
            .context("ACP policy needs the authenticated principal")?;
        let Some(session) = bounded(&value["params"]["sessionId"], 512) else {
            bail!("Malformed ACP permission request");
        };
        if bounded(&value["params"]["toolCall"]["toolCallId"], 512).is_none() {
            bail!("Malformed ACP permission request");
        }
        let Some(options) = value["params"]["options"]
            .as_array()
            .filter(|o| !o.is_empty() && o.len() <= 32)
        else {
            bail!("Malformed ACP permission request");
        };
        let mut ids = BTreeSet::new();
        let mut allows = Vec::new();
        for option in options {
            let Some(id) = bounded(&option["optionId"], 512) else {
                bail!("Malformed ACP permission request");
            };
            if !ids.insert(id.clone()) {
                bail!("Malformed ACP permission request");
            };
            if option["kind"] == "allow_once" {
                allows.push(id)
            }
        }
        // Negotiated Instance ownership has one durable rule store. Do not
        // offer an engine-owned persistent option alongside the host option.
        if self.producer.is_some() {
            for option in options {
                if matches!(
                    option["kind"].as_str(),
                    Some("allow_always" | "reject_always")
                ) && let Some(id) = option["optionId"].as_str()
                {
                    ids.remove(id);
                }
            }
        }
        let allow = if allows.len() == 1 {
            allows.pop().unwrap()
        } else {
            String::new()
        };
        let d = &value["params"]["toolCall"]["_meta"][META];
        let descriptor = (|| {
            let producer = self.producer.as_ref()?;
            if allow.is_empty() {
                return None;
            }
            if d["version"] != 1 || d["producer"] != *producer || d["allowOptionId"] != allow {
                return None;
            }
            let key = d["key"].as_str()?;
            if key.len() != 64
                || !key
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return None;
            }
            let lifetime = d["authorityLifetime"].as_str()?;
            if !matches!(lifetime, "request" | "conversation") {
                return None;
            }
            Some(Descriptor {
                producer: producer.clone(),
                key: key.into(),
                description: bounded(&d["description"], 512)?,
                authority_lifetime: lifetime.into(),
            })
        })();
        let db = state.db.lock().unwrap();
        let policy = load(&db)?;
        let always = if descriptor.is_some()
            && delegated(&db, &policy, &actor)?
            && (policy.rules.len() < MAX_RULES
                || policy.rules.iter().any(|r| r.status == "revoked"))
        {
            let id = format!("instance-remember-{}", uuid::Uuid::new_v4());
            ensure!(!ids.contains(&id), "Approval option collision");
            Some(id)
        } else {
            None
        };
        Ok(Some(Offer {
            id: value["id"].clone(),
            frontend_id: format!("instance-permission-{}", uuid::Uuid::new_v4()),
            session,
            prompt: prompt.into(),
            source: None,
            revision: policy.revision,
            actor,
            backend: self.backend.clone(),
            allow,
            descriptor,
            always,
            original_options: ids,
        }))
    }
}
impl Offer {
    pub fn present(&self, value: &mut Value) {
        value["id"] = json!(self.frontend_id);
        value["params"]["options"]
            .as_array_mut()
            .unwrap()
            .retain(|option| {
                option["optionId"]
                    .as_str()
                    .is_some_and(|id| self.original_options.contains(id))
            });
        if let Some(id) = &self.always {
            value["params"]["options"].as_array_mut().unwrap().push(json!({"optionId":id,"kind":"allow_always","name":"Always allow matching requests across this agent’s chats"}));
        }
    }
    pub fn cancelled(&self) -> Value {
        json!({"jsonrpc":"2.0","id":self.id,"result":{"outcome":{"outcome":"cancelled"}}})
    }
    fn allowed(&self) -> Value {
        json!({"jsonrpc":"2.0","id":self.id,"result":{"outcome":{"outcome":"selected","optionId":self.allow}}})
    }
    /// Validate and durably authorize under one database transaction. The caller
    /// has already checked the exact active prompt/source and cancellation state.
    /// This commit is the decision boundary, not evidence of delivery or effects.
    pub fn decide(
        &self,
        state: &AppState,
        headers: Option<&HeaderMap>,
        response: Option<Value>,
    ) -> Result<Option<Value>> {
        let _approval_guard = state.approval_policy_guard.lock().unwrap();
        if current_actor(state, headers).as_deref() != Some(&self.actor)
            || !state.enabled()
            || selected_backend(state)? != self.backend
        {
            return Ok(Some(self.cancelled()));
        }
        let mut db = state.db.lock().unwrap();
        let tx = db.transaction()?;
        let mut policy = load(&tx)?;
        if policy.revision != self.revision
            || !valid_client(&tx, self.actor.strip_prefix("token:").unwrap_or(""))?
        {
            return Ok(Some(self.cancelled()));
        }
        let basis;
        let result = if let Some(mut response) = response {
            if response["id"] != self.frontend_id {
                return Ok(Some(self.cancelled()));
            }
            response["id"] = self.id.clone();
            let outcome = &response["result"]["outcome"];
            if outcome["outcome"] == "cancelled" {
                return Ok(Some(self.cancelled()));
            }
            let option = outcome["optionId"].as_str().unwrap_or("");
            if outcome["outcome"] != "selected" {
                return Ok(Some(self.cancelled()));
            }
            if self.always.as_deref() == Some(option) {
                if !delegated(&tx, &policy, &self.actor)? {
                    return Ok(Some(self.cancelled()));
                }
                while policy.rules.len() >= MAX_RULES {
                    let Some(i) = policy.rules.iter().position(|r| r.status == "revoked") else {
                        return Ok(Some(self.cancelled()));
                    };
                    policy.rules.remove(i);
                }
                let Some(d) = &self.descriptor else {
                    return Ok(Some(self.cancelled()));
                };
                policy.revision = policy
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("Policy revision exhausted"))?;
                policy.rules.push(Rule {
                    id: uuid::Uuid::new_v4().to_string(),
                    revision: policy.revision,
                    status: "active".into(),
                    backend: self.backend.clone(),
                    producer: d.producer.clone(),
                    key: d.key.clone(),
                    description: d.description.clone(),
                    authority_lifetime: d.authority_lifetime.clone(),
                    creator: self.actor.clone(),
                    created_at: now_ms(),
                });
                save(&tx, &policy)?;
                basis = "remembered_by_client";
                self.allowed()
            } else if self.original_options.contains(option) {
                basis = "client_decision";
                response
            } else {
                return Ok(Some(self.cancelled()));
            }
        } else if policy.mode == Mode::Automatic && !self.allow.is_empty() {
            basis = "automatic";
            self.allowed()
        } else if let Some(d) = &self.descriptor {
            let mut matched = false;
            for r in &policy.rules {
                if r.status == "active"
                    && r.backend == self.backend
                    && r.producer == d.producer
                    && r.key == d.key
                    && r.authority_lifetime == d.authority_lifetime
                    && delegated(&tx, &policy, &r.creator)?
                {
                    matched = true;
                    break;
                }
            }
            if !matched {
                return Ok(None);
            };
            basis = "remembered_match";
            self.allowed()
        } else {
            return Ok(None);
        };
        {
            let id = uuid::Uuid::new_v4().to_string();
            let receipt = json!({"schema":"agent-instance.approval-decision/1","id":id,"at":now_ms(),"policy_revision":policy.revision,"basis":basis,"actor":self.actor,"session_id":self.session,"prompt_id":self.prompt,"request_id":self.id,"backend":self.backend,"outcome":result["result"]["outcome"],"delivery":"unknown","effects":"unknown"});
            tx.execute("INSERT INTO records(collection,id,value,updated_at) VALUES('approval_decisions',?1,?2,?3)",params![id,receipt.to_string(),now_ms()])?;
            tx.execute("DELETE FROM records WHERE collection='approval_decisions' AND id NOT IN (SELECT id FROM records WHERE collection='approval_decisions' ORDER BY updated_at DESC,id DESC LIMIT 256)",[])?;
        }
        tx.commit()?;
        Ok(Some(result))
    }
}
