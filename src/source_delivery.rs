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
#[serde(deny_unknown_fields)]
struct Metadata {
    version: u32,
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
    json!({"version":1,"metadata":META,"receiptsMethod":METHOD,"incorporationIsCompletion":false})
}
pub fn register(state: &AppState, principal: &str, params: &Value) -> Result<Option<String>> {
    let Some(meta) = params["_meta"].get(META) else {
        return Ok(None);
    };
    let m: Metadata = serde_json::from_value(meta.clone())?;
    ensure!(m.version == 1, "Unsupported source delivery version");
    valid(&m.sources)?;
    let sid = params["sessionId"].as_str().unwrap_or("");
    let input = params["_meta"]["steering"]["inputId"]
        .as_str()
        .unwrap_or("");
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
        ensure!(
            row["sources"] == json!(m.sources),
            "Input ID already belongs to different source revisions"
        );
        return Ok(Some(input.into()));
    }
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
    let row = json!({"principal":principal,"sessionId":sid,"inputId":input,"sources":m.sources,"state":"unknown","incorporated":false,"observedAt":now_ms()});
    db.execute(
        "INSERT INTO records(collection,id,value,updated_at) VALUES('source_delivery',?1,?2,?3)",
        params![id, row.to_string(), now_ms()],
    )?;
    Ok(Some(input.into()))
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
pub fn facts(state: &AppState, principal: &str, sid: &str, sources: &[Source]) -> Result<Value> {
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
        if let Some(row) = row {
            result.push(json!({"sourceId":s.id,"revision":s.revision,"occurredAt":s.occurred_at,"inputId":row["inputId"],"state":row["state"],"incorporated":row["incorporated"],"observedAt":row["observedAt"],"completion":"not_established"}));
        }
    }
    Ok(json!(result))
}
pub fn query(state: &AppState, principal: &str, params: &Value) -> Result<Value> {
    let sources: Vec<Source> = serde_json::from_value(params["sources"].clone())?;
    Ok(
        json!({"version":1,"receipts":facts(state,principal,params["sessionId"].as_str().unwrap_or(""),&sources)?}),
    )
}
