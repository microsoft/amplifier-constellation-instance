//! Context delivery evidence, deliberately distinct from persona quality.
use crate::{
    auth,
    error::{ApiError, ApiResult},
    profiles,
    state::{AppState, now_ms},
};
use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Deserialize, Default)]
pub struct Selection {
    pub session_id: Option<String>,
    pub max_age_seconds: Option<i64>,
}
pub fn inspect(state: &AppState, selection: &Selection) -> ApiResult<Value> {
    let max_age = selection.max_age_seconds.unwrap_or(24 * 60 * 60);
    if !(60..=30 * 24 * 60 * 60).contains(&max_age) {
        return Err(ApiError::bad(
            "max_age_seconds must be between 60 seconds and 30 days",
        ));
    }
    let (current_text, current_package) = profiles::context(state)?;
    let current_hash = auth::digest(&current_text);
    let configured = !current_text.is_empty();
    if selection.session_id.as_ref().is_some_and(|s| s.len() > 512) {
        return Err(ApiError::bad("Session ID too long"));
    }
    // Selection is based on start order, never last update/completion order or a truncated recent-session list.
    let mut latest: Option<Value> = {
        use rusqlite::OptionalExtension;
        let db = state.db.lock().unwrap();
        let raw:Option<String>=db.query_row("SELECT value FROM records WHERE collection='profile_turns' AND (?1 IS NULL OR json_extract(value,'$.session_id')=?1) ORDER BY json_extract(value,'$.started_sequence') DESC LIMIT 1",[&selection.session_id],|r|r.get(0)).optional()?;
        raw.map(|s| {
            serde_json::from_str(&s).map_err(|_| ApiError::bad("Invalid stored turn observation"))
        })
        .transpose()?
    };
    if let Some(turn) = latest.as_mut()
        && turn["outcome"] == "in_progress"
        && !state
            .connections
            .lock()
            .unwrap()
            .contains_key(turn["connection_id"].as_str().unwrap_or_default())
    {
        turn["outcome"] = json!("completion_unknown");
    }
    let at = now_ms();
    let mut status = if configured {
        "unknown"
    } else {
        "not_configured"
    };
    let mut content_match = Value::Null;
    let mut version_match = Value::Null;
    let mut age = Value::Null;
    if let Some(turn) = &latest {
        let elapsed = at.saturating_sub(turn["started_at"].as_i64().unwrap_or(0));
        age = json!(elapsed);
        // Empty guidance has no current persona to compare, even if the empty
        // hashes match. Preserve the historical turn separately.
        if configured && turn["profile_sha256"].as_str().is_some() {
            content_match = json!(turn["profile_sha256"] == current_hash);
            version_match = json!(turn["profile_package"] == current_package);
            status = if elapsed > max_age * 1000 {
                "stale"
            } else if content_match == true {
                "supplied_current"
            } else {
                "supplied_previous"
            };
        } else if configured {
            status = "partial";
        }
    }
    Ok(
        json!({"schema":"agent-instance.profile-use/1","observed_at":at,"selection":{"session_id":selection.session_id,"rule":"latest_started_turn","max_age_seconds":max_age},"status":status,"installed":{"configured":configured,"package":current_package,"profile_sha256":current_hash},"relevant_turn":latest,"age_ms":age,"content_matches":content_match,"version_matches":version_match,"behavior_quality":"not_evaluated","delivery_claim":if configured {"Context was written to the backend input; this does not prove the model read, followed, or successfully used it"} else {"No persona profile is configured; any previous turn evidence is retained"},"source_draft_status":"not_observed"}),
    )
}
pub async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(selection): Query<Selection>,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "observe", false)?;
    Ok(Json(inspect(&state, &selection)?))
}
pub fn started(
    state: &AppState,
    work_id: &str,
    session_id: &str,
    connection_id: &str,
    started_at: i64,
    sequence: i64,
    supplied: (&str, &Value),
) -> ApiResult<()> {
    let (profile_hash, package) = supplied;
    state.set_record("profile_turns",work_id,json!({"id":work_id,"session_id":session_id,"connection_id":connection_id,"started_at":started_at,"started_sequence":sequence,"profile_sha256":profile_hash,"profile_package":package,"outcome":"in_progress"}))?;
    Ok(())
}
pub fn finished(state: &AppState, work_id: &str, outcome: &str) -> ApiResult<()> {
    if let Some(mut value) = state.get_record("profile_turns", work_id)? {
        value["outcome"] = json!(outcome);
        value["finished_at"] = json!(now_ms());
        state.set_record("profile_turns", work_id, value)?;
    }
    Ok(())
}
