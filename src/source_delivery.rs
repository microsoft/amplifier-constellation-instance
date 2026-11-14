//! Source/revision facts bound to authenticated sessions. No social policy here.
use crate::state::{AppState, now_ms};
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
pub const META: &str = "com.microsoft.amplifier.source-delivery";
pub const METHOD: &str = "_amplifier/source_receipts";
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub revision: String,
    pub occurred_at: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Metadata {
    version: u32,
    input_id: Option<String>,
    sources: Vec<Source>,
}
fn key(principal: &str, sid: &str, input: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(json!([principal, sid, input]).to_string())
    )
}
fn valid(sources: &[Source]) -> Result<()> {
    ensure!(
        !sources.is_empty() && sources.len() <= 40,
        "Supply 1 to 40 source revisions"
    );
    for s in sources {
        ensure!(
            !s.id.is_empty()
                && s.id.len() <= 512
                && !s.revision.is_empty()
                && s.revision.len() <= 128
                && s.occurred_at > 0
                && s.occurred_at <= now_ms() + 5000,
            "Invalid source revision"
        );
    }
    Ok(())
}
pub fn capability() -> Value {
    json!({"version":1,"metadata":META,"receiptsMethod":METHOD,"incorporationIsCompletion":false,"ordinaryPrompts":true,"promptRetry":"inspectOnly","workReferences":true,"retentionMs":86400000,"maxRecords":8192,"queryRecords":256,"workQueryRecords":1000,"workReferencesPerSource":64})
}
pub fn register(state: &AppState, principal: &str, params: &Value) -> Result<Option<String>> {
    register_input(state, principal, params, None, false)
}
pub fn register_input(
    state: &AppState,
    principal: &str,
    params: &Value,
    binding: Option<&Value>,
    ordinary_prompt: bool,
) -> Result<Option<String>> {
    let Some(meta) = params["_meta"].get(META) else {
        return Ok(None);
    };
    let m: Metadata = serde_json::from_value(meta.clone())?;
    ensure!(m.version == 1, "Unsupported source delivery version");
    valid(&m.sources)?;
    let sid = params["sessionId"].as_str().unwrap_or("");
    let steering_id = params["_meta"]["steering"]["inputId"].as_str();
    ensure!(
        m.input_id
            .as_deref()
            .zip(steering_id)
            .is_none_or(|(a, b)| a == b),
        "Conflicting source input IDs"
    );
    let input = m.input_id.as_deref().or(steering_id).unwrap_or("");
    ensure!(
        !sid.is_empty() && !input.is_empty() && input.len() <= 512,
        "Source delivery needs session and stable input ID"
    );
    let id = key(principal, sid, input);
    let db = state.db.lock().unwrap();
    let saved: Option<String> = db
        .query_row(
            "SELECT value FROM records WHERE collection='source_delivery' AND id=?1",
            [&id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(raw) = saved {
        let row: Value = serde_json::from_str(&raw)?;
        // Legacy steering rows predate stored audience metadata. Their existing
        // principal/session scope remains; a safe explicit prompt promotion
        // records the freshly authorized binding without rewriting past facts.
        let legacy_binding = row.get("binding").is_none();
        ensure!(
            row["sources"] == json!(m.sources)
                && (legacy_binding || compatible_binding(&row["binding"], &json!(binding))),
            "Input ID already belongs to different source revisions or audience"
        );
        if ordinary_prompt {
            // A negotiated steering refusal may explicitly ask for the next
            // ordinary prompt. Promote that same input once, atomically. An
            // uncertain or incorporated input is never a safe fallback.
            ensure!(
                row["kind"] != "prompt"
                    && row["incorporated"] != true
                    && matches!(
                        row["state"].as_str(),
                        Some("promptRequired" | "notAccepted")
                    ),
                "Input already recorded; inspect its receipt instead of replaying it"
            );
            let mut row = row;
            row["kind"] = json!("prompt");
            row["state"] = json!("unknown");
            row["observedAt"] = json!(now_ms());
            // A renewed group grant may name another participant. Keep the
            // original input attribution; only legacy rows lack that evidence.
            if legacy_binding {
                row["binding"] = json!(binding);
            }
            db.execute("UPDATE records SET value=?1,updated_at=?2 WHERE collection='source_delivery' AND id=?3",params![row.to_string(),now_ms(),id])?;
        } else {
            ensure!(
                row["kind"] != "prompt",
                "Ordinary input already recorded; inspect its receipt"
            );
        }
        return Ok(Some(input.into()));
    }
    // A retained work origin is still proof that this input already existed,
    // even when its shorter-lived delivery receipt has been pruned.
    let linked: bool=db.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE collection='work' AND json_extract(value,'$.source_input.principal')=?1 AND json_extract(value,'$.source_input.sessionId')=?2 AND json_extract(value,'$.source_input.inputId')=?3)",params![principal,sid,input],|r|r.get(0))?;
    ensure!(
        !linked,
        "Input is retained by existing work; inspect instead of replaying it"
    );
    db.execute(
        "DELETE FROM records WHERE collection='source_delivery' AND updated_at < ?1",
        [now_ms() - 86_400_000],
    )?;
    let n: i64 = db.query_row(
        "SELECT count(*) FROM records WHERE collection='source_delivery'",
        [],
        |r| r.get(0),
    )?;
    ensure!(n < 8192, "Source delivery journal is full");
    let row = json!({"principal":principal,"sessionId":sid,"inputId":input,"sources":m.sources,"binding":binding,"kind":if ordinary_prompt {"prompt"}else{"steering"},"state":"unknown","incorporated":false,"observedAt":now_ms()});
    db.execute(
        "INSERT INTO records(collection,id,value,updated_at) VALUES('source_delivery',?1,?2,?3)",
        params![id, row.to_string(), now_ms()],
    )?;
    Ok(Some(input.into()))
}

fn compatible_binding(a: &Value, b: &Value) -> bool {
    crate::context::same_binding(a, b)
}

/// Only the authenticated context lease chooses principal/session/audience.
/// Source identifiers are caller provenance, not independently certified text.
pub fn registered_input(
    state: &AppState,
    principal: &str,
    sid: &str,
    input: &str,
    binding: &Value,
) -> Result<Value> {
    ensure!(
        !input.is_empty() && input.len() <= 512,
        "Invalid source input ID"
    );
    let row = match state.get_record("source_delivery", &key(principal, sid, input))? {
        Some(row) => row,
        None => {
            let db = state.db.lock().unwrap();
            let saved:Option<String>=db.query_row("SELECT json_extract(value,'$.source_input') FROM records WHERE collection='work' AND json_extract(value,'$.source_input.principal')=?1 AND json_extract(value,'$.source_input.sessionId')=?2 AND json_extract(value,'$.source_input.inputId')=?3 LIMIT 1",params![principal,sid,input],|r|r.get(0)).optional()?;
            serde_json::from_str(
                &saved.ok_or_else(|| {
                    anyhow::anyhow!("Source input is not retained for this session")
                })?,
            )?
        }
    };
    ensure!(
        row["binding"].is_object() && crate::context::same_binding(&row["binding"], binding),
        "Source input belongs to a different audience"
    );
    Ok(
        json!({"principal":principal,"sessionId":sid,"inputId":input,"sources":row["sources"],"binding":row["binding"]}),
    )
}
pub fn observe(
    state: &AppState,
    principal: &str,
    sid: &str,
    input: &str,
    status: &str,
) -> Result<()> {
    if ![
        "received",
        "submitted",
        "remoteQueued",
        "incorporated",
        "notAccepted",
        "promptRequired",
        "unknown",
    ]
    .contains(&status)
    {
        return Ok(());
    }
    let id = key(principal, sid, input);
    let db = state.db.lock().unwrap();
    let raw: Option<String> = db
        .query_row(
            "SELECT value FROM records WHERE collection='source_delivery' AND id=?1",
            [&id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(raw) = raw else {
        return Ok(());
    };
    let mut row: Value = serde_json::from_str(&raw)?;
    // A late weaker envelope cannot erase observed incorporation. This fact
    // means only that the provider used the input, never that the work is done.
    if status == "incorporated" {
        row["incorporated"] = json!(true);
    }
    row["state"] = json!(status);
    row["observedAt"] = json!(now_ms());
    db.execute(
        "UPDATE records SET value=?1,updated_at=?2 WHERE collection='source_delivery' AND id=?3",
        params![row.to_string(), now_ms(), id],
    )?;
    Ok(())
}

pub fn prompt_response(
    state: &AppState,
    principal: &str,
    sid: &str,
    input: &str,
    response: &Value,
) -> Result<()> {
    let id = key(principal, sid, input);
    let db = state.db.lock().unwrap();
    let raw: Option<String> = db
        .query_row(
            "SELECT value FROM records WHERE collection='source_delivery' AND id=?1",
            [&id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(raw) = raw else { return Ok(()) };
    let mut row: Value = serde_json::from_str(&raw)?;
    if row["kind"] != "prompt" {
        return Ok(());
    }
    row["response"] = if response.get("error").is_some() {
        json!({"kind":"error","code":response["error"]["code"].as_i64()})
    } else {
        json!({"kind":"result","stopReason":response["result"]["stopReason"].as_str().filter(|s|s.len()<=128)})
    };
    row["observedAt"] = json!(now_ms());
    db.execute(
        "UPDATE records SET value=?1,updated_at=?2 WHERE collection='source_delivery' AND id=?3",
        params![row.to_string(), now_ms(), id],
    )?;
    Ok(())
}
pub fn facts(state: &AppState, principal: &str, sid: &str, sources: &[Source]) -> Result<Value> {
    facts_bound(state, principal, sid, sources, None)
}
fn facts_bound(
    state: &AppState,
    principal: &str,
    sid: &str,
    sources: &[Source],
    binding: Option<&Value>,
) -> Result<Value> {
    if sources.is_empty() {
        return Ok(json!([]));
    }
    valid(sources)?;
    let db = state.db.lock().unwrap();
    let mut q=db.prepare("SELECT value FROM records WHERE collection='source_delivery' AND json_extract(value,'$.principal')=?1 AND json_extract(value,'$.sessionId')=?2 ORDER BY updated_at DESC LIMIT 256")?;
    let rows = q
        .query_map([principal, sid], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let rows: Vec<Value> = rows
        .iter()
        .map(|raw| serde_json::from_str(raw))
        .collect::<serde_json::Result<_>>()?;
    // Reuse immutable provenance on the responsibility itself. This also
    // preserves established references after the short delivery window expires.
    let mut work_query = db.prepare("SELECT value FROM records WHERE collection='work' AND json_extract(value,'$.source_input.principal')=?1 AND json_extract(value,'$.source_input.sessionId')=?2 ORDER BY id LIMIT 1001")?;
    let work: Vec<Value> = if binding.is_some() {
        work_query
            .query_map([principal, sid], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .map(|raw| serde_json::from_str(raw))
            .collect::<serde_json::Result<_>>()?
    } else {
        vec![]
    };
    let mut result = vec![];
    for s in sources {
        let matches = |row: &&Value| {
            row["sources"].as_array().is_some_and(|ss| {
                ss.iter().any(|x| {
                    x["id"] == s.id
                        && x["revision"] == s.revision
                        && x["occurredAt"] == s.occurred_at
                })
            })
        };
        // A later uncertain delivery of the same source does not hide an earlier
        // observed incorporation. Neither fact establishes task completion.
        let mut found = rows.iter().filter(matches);
        let row = found
            .clone()
            .find(|r| r["incorporated"] == true)
            .or_else(|| found.next());
        let matches_work = |w: &&Value| {
            binding.is_some_and(|b| {
                crate::context::same_binding(&w["source_input"]["binding"], b)
                    && serde_json::from_value::<crate::work::Audience>(w["audience"].clone())
                        .is_ok_and(|a| a.accessible(Some(b)))
            }) && w["source_input"]["sources"].as_array().is_some_and(|ss| {
                ss.iter().any(|x| {
                    x["id"] == s.id
                        && x["revision"] == s.revision
                        && x["occurredAt"] == s.occurred_at
                })
            })
        };
        let matched: Vec<&Value> = work
            .iter()
            .take(1000)
            .filter(matches_work)
            .take(65)
            .collect();
        let refs: Vec<Value> = matched.iter().take(64).map(|w|json!({"id":w["id"],"ownerAgentId":w["owner_agent_id"],"revision":w["revision"],"state":w["state"],"inputId":w["source_input"]["inputId"]})).collect();
        if row.is_some() || !refs.is_empty() {
            let empty = json!({"state":"unknown","incorporated":false});
            let row = row.unwrap_or(&empty);
            result.push(json!({"sourceId":s.id,"revision":s.revision,"occurredAt":s.occurred_at,"inputId":row["inputId"],"state":row["state"],"incorporated":row["incorporated"],"response":row["response"],"observedAt":row["observedAt"],"completion":"not_established","work":refs,"workCoverage":if work.len()>1000 || matched.len()>64 {"partial"}else{"retained_session_records"}}));
        }
    }
    Ok(json!(result))
}
pub fn query(state: &AppState, principal: &str, params: &Value) -> Result<Value> {
    query_bound(state, principal, params, None)
}
pub fn query_bound(
    state: &AppState,
    principal: &str,
    params: &Value,
    binding: Option<&Value>,
) -> Result<Value> {
    let sources: Vec<Source> = serde_json::from_value(params["sources"].clone())?;
    Ok(
        json!({"version":1,"receipts":facts_bound(state,principal,params["sessionId"].as_str().unwrap_or(""),&sources,binding)?,"coverage":{"delivery":"recent_session_records","work":"retained_session_records","missing":"unknown"}}),
    )
}
