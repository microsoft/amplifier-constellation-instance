//! Optional native source for the existing authorized continuity MCP layer.
//! The runtime owns its database and projects public messages over a local CLI.
use crate::{native_config, state::AppState};
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};

pub const PROVIDER: &str = "native";
pub const PROFILE: &str = "agent-runtime.history/1";

pub fn bind(state: &AppState, binding: &Value, sid: &str, history: &Value) -> Result<()> {
    // External rooms remain authoritative in their connector. Registering copies
    // would bypass source membership/revocation and duplicate search results.
    if binding["owner"] != true
        || binding["provider_id"].is_string()
        || history["profile"] != PROFILE
        || native_config::installation(state).is_err()
    {
        return Ok(());
    }
    let Some(store_id) = history["storeId"]
        .as_str()
        .filter(|v| uuid::Uuid::parse_str(v).is_ok())
    else {
        return Ok(());
    };
    uuid::Uuid::parse_str(sid)?;
    state.set_record(
        "native_history",
        &format!("{store_id}:{sid}"),
        json!({"store_id":store_id,"session_id":sid,"owner":true}),
    )?;
    Ok(())
}
async fn query(state: &AppState, body: Value) -> Result<Value> {
    let result=native_config::history_query(state, &body).await
        .map_err(|_|anyhow!("Native history is unavailable; the configured engine must support agent-runtime.history/1"))?;
    ensure!(
        result["profile"] == PROFILE,
        "Unsupported native history profile"
    );
    Ok(result)
}
async fn identity(state: &AppState) -> Result<String> {
    let info = query(state, json!({"operation":"describe"})).await?;
    let id = info["store_id"]
        .as_str()
        .filter(|v| uuid::Uuid::parse_str(v).is_ok())
        .ok_or_else(|| anyhow!("Invalid native history identity"))?;
    Ok(id.into())
}
fn authorized(state: &AppState, store_id: &str, sid: &str) -> Result<bool> {
    let record = state.get_record("native_history", &format!("{store_id}:{sid}"))?;
    let binding = state.get_record("context_bindings", sid)?;
    Ok(record
        .is_some_and(|r| r["store_id"] == store_id && r["session_id"] == sid && r["owner"] == true)
        && binding.is_some_and(|b| b["owner"] == true && !b["provider_id"].is_string()))
}
pub async fn list(
    state: &AppState,
    owner: bool,
    limit: u64,
    cursor: Option<&str>,
) -> Result<Value> {
    ensure!(
        owner,
        "Native private history requires the owner's current context access"
    );
    let store_id = identity(state).await?;
    let mut ids = vec![];
    for row in state.list_records("native_history")? {
        if row["store_id"] == store_id
            && let Some(sid) = row["session_id"].as_str()
            && authorized(state, &store_id, sid)?
        {
            ids.push(sid.to_owned());
        }
    }
    let mut value=query(state,json!({"operation":"list","store_id":store_id,"session_ids":ids,"limit":limit,"cursor":cursor})).await?;
    value["coverage"] = json!(
        "Native owner conversations registered by this instance (up to 1000 recent registrations). Unbound legacy sessions and external-room copies are excluded. Resume an owner's legacy session once to register it."
    );
    Ok(value)
}
pub async fn read(
    state: &AppState,
    owner: bool,
    sid: &str,
    limit: u64,
    before: Option<u64>,
) -> Result<Value> {
    ensure!(
        owner,
        "Native private history requires the owner's current context access"
    );
    uuid::Uuid::parse_str(sid)?;
    let store_id = identity(state).await?;
    ensure!(
        authorized(state, &store_id, sid)?,
        "Native conversation is not authorized for this source"
    );
    query(state,json!({"operation":"read","store_id":store_id,"session_id":sid,"limit":limit,"before":before})).await
}
