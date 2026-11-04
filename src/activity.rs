//! A bounded, metadata-only view of the useful things this individual does.
//!
//! Raw event polling and SSE remain available for UI synchronization. Their
//! diagnostic traffic has a separate retention budget and does not evict work.
use crate::{
    auth,
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
};
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub const DIAGNOSTIC_RETENTION: usize = 1000;

pub(crate) struct Metadata {
    pub diagnostic: bool,
    pub category: &'static str,
    pub severity: &'static str,
    pub summary: &'static str,
    pub data: Value,
}

/// Allow only descriptive metadata. In particular, never copy arbitrary
/// backend errors, tool arguments, model text, profile bodies, or secrets.
fn safe_data(data: &Value) -> Value {
    let mut safe = Map::new();
    for key in [
        "provider_id",
        "conversation_id",
        "thread_id",
        "operation",
        "connection_id",
        "session_id",
        "work_id",
        "actor",
        "principal",
        "instance_id",
        "id",
        "name",
        "version",
        "method",
        "direction",
        "update_kind",
        "stage",
        "state",
        "reason",
        "action",
        "stop_reason",
        "profile_sha256",
        "sha256",
    ] {
        if let Some(value) = data[key].as_str() {
            safe.insert(
                key.into(),
                json!(value.chars().take(512).collect::<String>()),
            );
        }
    }
    for key in [
        "result_count",
        "provider_count",
        "revision",
        "config_revision",
        "prompt_config_revision",
        "duration_ms",
        "port",
        "error_code",
    ] {
        if let Some(value) = data[key].as_i64() {
            safe.insert(key.into(), json!(value));
        }
    }
    for key in ["enabled", "success", "complete"] {
        if let Some(value) = data[key].as_bool() {
            safe.insert(key.into(), json!(value));
        }
    }
    if let Some(scopes) = data["scopes"].as_array() {
        safe.insert(
            "scopes".into(),
            json!(
                scopes
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|scope| matches!(
                        *scope,
                        "admin"
                            | "observe"
                            | "ui"
                            | "acp"
                            | "profiles"
                            | "work:read"
                            | "work"
                            | "work:execute"
                    ))
                    .collect::<Vec<_>>()
            ),
        );
    }
    Value::Object(safe)
}

pub(crate) fn metadata(kind: &str, data: &Value) -> Metadata {
    let (category, severity, summary) = match kind {
        "updates.requested" => (
            "configuration",
            "info",
            "Queued an installation update request",
        ),
        "updates.configured" => (
            "configuration",
            "info",
            "Updated installation update preferences",
        ),
        "updates.progress" => match data["state"].as_str() {
            Some("succeeded") => (
                "outcomes",
                "info",
                "Installed and verified component updates",
            ),
            Some("rolled_back") => (
                "outcomes",
                "warning",
                "Restored the previous component versions",
            ),
            Some("failed" | "check_failed" | "recovery_required") => {
                ("problems", "warning", "Installation updates need attention")
            }
            Some("available") => ("configuration", "info", "Component updates are available"),
            Some("up_to_date") => (
                "configuration",
                "info",
                "Installed components are up to date",
            ),
            _ => ("configuration", "info", "Installation update in progress"),
        },
        "scheduler.configured" => ("configuration", "info", "Updated background work settings"),
        "scheduler.permission_denied" => (
            "problems",
            "warning",
            "Background work needs an unapproved permission",
        ),
        "scheduler.error" => ("problems", "error", "Background execution needs attention"),
        "scheduler.run_changed" => match data["action"].as_str() {
            Some("queued") => ("work", "info", "Authorized a background run"),
            Some("starting" | "running") => ("work", "info", "Started authorized background work"),
            Some("reported") => ("outcomes", "info", "Background report is ready for review"),
            Some("needs_reconciliation") => (
                "problems",
                "warning",
                "Inspect interrupted work before retrying",
            ),
            Some("failed_before_prompt") => {
                ("problems", "warning", "Background run could not start")
            }
            Some("reconciled") => (
                "outcomes",
                "info",
                "Recorded an operator's interruption review",
            ),
            _ => ("work", "info", "Updated background execution"),
        },
        "work.created" => ("work", "info", "Recorded a proposed responsibility"),
        "work.changed" => ("work", "info", "Updated a responsibility"),
        "work.exported" => (
            "work",
            "info",
            "Prepared a delegation for an authorized peer",
        ),
        "work.imported" => ("work", "info", "Received a proposed responsibility"),
        "organization.changed" => (
            "configuration",
            "info",
            "Updated assignments and sharing relationships",
        ),
        "acp.prompt.started" => ("work", "info", "Started a work turn"),
        "acp.prompt.finished" => match data["stop_reason"].as_str() {
            Some("max_tokens") => ("outcomes", "warning", "Work turn reached its token limit"),
            Some("max_turn_requests") => {
                ("outcomes", "warning", "Work turn reached its request limit")
            }
            Some("refusal") => ("outcomes", "warning", "Agent declined this work turn"),
            Some("unspecified") => ("outcomes", "info", "Agent returned a work-turn response"),
            _ => ("outcomes", "success", "Work turn finished"),
        },
        "acp.prompt.cancelled" => ("outcomes", "info", "Work turn was cancelled"),
        "acp.prompt.failed"
            if matches!(
                data["reason"].as_str(),
                Some(
                    "connection_ended" | "client_disconnected" | "runtime_stopped" | "idle_timeout"
                )
            ) =>
        {
            ("problems", "warning", "Work turn was interrupted")
        }
        "acp.prompt.failed" => ("problems", "error", "Work turn failed"),
        "acp.launch_failed" => ("problems", "error", "Could not start the agent backend"),
        "acp.closed_with_error" if data["reason"] == "runtime_stopped" => (
            "runtime",
            "info",
            "Agent connection stopped with the runtime",
        ),
        "acp.closed_with_error" if data["reason"] == "idle_timeout" => {
            ("connections", "info", "Inactive client connection closed")
        }
        "acp.closed_with_error" => ("problems", "error", "Agent connection ended unexpectedly"),
        "acp.connected" => ("connections", "info", "Client connected to this agent"),
        "acp.disconnected" => ("connections", "info", "Client disconnected"),
        "context.retrieved" if data["operation"] == "read" => {
            ("work", "info", "Read context from a connected conversation")
        }
        "context.retrieved" => (
            "work",
            "info",
            "Found conversations across connected experiences",
        ),
        "context.unavailable" => (
            "problems",
            "warning",
            "A conversation source could not be reached or declined access",
        ),
        "context.providers_changed" => ("configuration", "info", "Conversation sources updated"),
        "config.changed" => ("configuration", "info", "Agent configuration changed"),
        "profile.package_applied" => ("configuration", "info", "Persona profile applied"),
        "profile.package_restored" => {
            ("configuration", "info", "Previous persona profile restored")
        }
        "profile.changed" => ("configuration", "info", "Agent profile changed"),
        "instance.created" => ("configuration", "success", "Agent instance created"),
        "backend.installed" => ("configuration", "success", "Agent runtime installed"),
        "runtime.changed" => {
            if data["enabled"] == true {
                ("runtime", "info", "Agent runtime enabled")
            } else {
                ("runtime", "info", "Agent runtime stopped")
            }
        }
        "service.started" => ("runtime", "success", "Agent service started"),
        "service.stopped" => ("runtime", "info", "Agent service stopped"),
        "secret.changed" => ("access", "info", "Provider credential updated"),
        "secret.deleted" => ("access", "info", "Provider credential removed"),
        "token.created" => ("access", "info", "Access token created"),
        "token.revoked" => ("access", "info", "Access token revoked"),
        "auth.failed" => ("access", "warning", "Sign-in attempt failed"),
        "auth.signed_in" => ("access", "info", "Signed in to the agent dashboard"),
        "auth.signed_out" => ("access", "info", "Signed out of the agent dashboard"),
        // Older installations recorded prompt boundaries as generic traffic.
        // Preserve those observations without claiming an unknown stop reason.
        "acp.activity" if data["method"] == "session/prompt" => {
            ("work", "info", "Work turn requested")
        }
        "acp.response" if data["method"] == "session/prompt" => {
            if data["success"] == true {
                ("outcomes", "info", "Agent returned a work-turn response")
            } else {
                ("problems", "error", "Work turn failed")
            }
        }
        "acp.activity" if data["method"] == "session/request_permission" => {
            ("work", "info", "Agent requested permission for a tool")
        }
        "acp.activity" if data["method"] == "session/cancel" => {
            ("work", "info", "Cancellation requested")
        }
        "view.changed" => ("diagnostics", "info", "Dashboard view changed"),
        "view.acknowledged" | "view.ack" => {
            ("diagnostics", "info", "Dashboard rendering acknowledged")
        }
        "acp.profile_supplied" => (
            "diagnostics",
            "info",
            "Profile context supplied to the backend",
        ),
        "acp.response" => ("diagnostics", "info", "ACP response received"),
        "acp.activity" => ("diagnostics", "info", "ACP protocol activity"),
        _ => ("diagnostics", "info", "Diagnostic event"),
    };
    Metadata {
        diagnostic: category == "diagnostics",
        category,
        severity,
        summary,
        data: safe_data(data),
    }
}

/// Adds an indexed projection alongside the existing raw event columns.
/// Existing installations are classified once; their seq IDs stay unchanged.
pub(crate) fn migrate(db: &Connection, meaningful_limit: usize) -> anyhow::Result<()> {
    let columns = db
        .prepare("PRAGMA table_info(events)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let transaction = db.unchecked_transaction()?;
    for (name, definition) in [
        ("activity_diagnostic", "INTEGER NOT NULL DEFAULT 1"),
        ("activity_category", "TEXT NOT NULL DEFAULT ''"),
        ("activity_severity", "TEXT NOT NULL DEFAULT ''"),
        ("activity_summary", "TEXT NOT NULL DEFAULT ''"),
        ("activity_data", "TEXT NOT NULL DEFAULT '{}'"),
    ] {
        if !columns.iter().any(|column| column == name) {
            transaction.execute_batch(&format!(
                "ALTER TABLE events ADD COLUMN {name} {definition};"
            ))?;
        }
    }
    let rows = transaction
        .prepare("SELECT seq,kind,data FROM events WHERE activity_category='' ")?
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (seq, kind, raw_data) in rows {
        let data = serde_json::from_str(&raw_data).unwrap_or(Value::Null);
        let meta = metadata(&kind, &data);
        transaction.execute("UPDATE events SET activity_diagnostic=?1,activity_category=?2,activity_severity=?3,activity_summary=?4,activity_data=?5 WHERE seq=?6", params![meta.diagnostic,meta.category,meta.severity,meta.summary,meta.data.to_string(),seq])?;
    }
    transaction.execute_batch("CREATE INDEX IF NOT EXISTS events_activity_class_seq ON events(activity_diagnostic,seq); CREATE INDEX IF NOT EXISTS events_activity_category_seq ON events(activity_category,seq);")?;
    retain(&transaction, false, meaningful_limit)?;
    retain(&transaction, true, DIAGNOSTIC_RETENTION)?;
    transaction.commit()?;
    Ok(())
}

pub(crate) fn retain(db: &Connection, diagnostic: bool, limit: usize) -> rusqlite::Result<()> {
    db.execute("DELETE FROM events WHERE seq IN (SELECT seq FROM events WHERE activity_diagnostic=?1 ORDER BY seq DESC LIMIT -1 OFFSET ?2)", params![diagnostic, limit as i64])?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityQuery {
    #[serde(default = "default_limit")]
    limit: usize,
    before: Option<i64>,
    #[serde(default)]
    diagnostics: bool,
    #[serde(default = "all_categories")]
    category: String,
    #[serde(default)]
    search: String,
}
fn default_limit() -> usize {
    100
}
fn all_categories() -> String {
    "all".into()
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ActivityQuery>,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "observe", false)?;
    if !(1..=200).contains(&query.limit) {
        return Err(ApiError::bad("Activity limit must be between 1 and 200"));
    }
    if query.before.is_some_and(|before| before <= 0) {
        return Err(ApiError::bad(
            "Activity cursor must be a positive sequence number",
        ));
    }
    if !matches!(
        query.category.as_str(),
        "all"
            | "work"
            | "outcomes"
            | "problems"
            | "connections"
            | "configuration"
            | "access"
            | "runtime"
            | "diagnostics"
    ) {
        return Err(ApiError::bad("Unknown activity category"));
    }
    if query.search.len() > 256 {
        return Err(ApiError::bad("Activity search must be at most 256 bytes"));
    }
    let db = state.db.lock().unwrap();
    let mut statement = db.prepare("SELECT seq,at,kind,activity_data,activity_summary,activity_category,activity_severity FROM events WHERE (?1 OR activity_diagnostic=0) AND seq<?2 AND (?3='all' OR activity_category=?3) AND (?4='' OR instr(lower(activity_summary || ' ' || kind || ' ' || activity_data),lower(?4))>0) ORDER BY seq DESC LIMIT ?5")?;
    let mut items = statement.query_map(params![query.diagnostics, query.before.unwrap_or(i64::MAX), query.category, query.search.trim(), (query.limit + 1) as i64], |row| {
        let data: String = row.get(3)?;
        Ok(json!({
            "seq":row.get::<_,i64>(0)?,"at":row.get::<_,i64>(1)?,"kind":row.get::<_,String>(2)?,
            "data":serde_json::from_str::<Value>(&data).unwrap_or(Value::Null),
            "summary":row.get::<_,String>(4)?,"category":row.get::<_,String>(5)?,"severity":row.get::<_,String>(6)?
        }))
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = items.len() > query.limit;
    items.truncate(query.limit);
    let next_before = if has_more {
        items.last().and_then(|item| item["seq"].as_i64())
    } else {
        None
    };
    Ok(Json(
        json!({"items":items,"next_before":next_before,"has_more":has_more}),
    ))
}
