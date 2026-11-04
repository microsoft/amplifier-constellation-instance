//! Installation update requests. A separately installed host helper executes them.
//! ACP callers select operations, never executables, paths, sources or credentials.
use crate::{
    auth,
    error::{ApiError, ApiResult},
    state::{AppState, now_ms},
};
use axum::{Json, extract::State, http::HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub revision: u64,
    pub agent_initiated: bool,
    pub check_minutes: u32,
    pub install: String,
    pub channel: String,
    pub window_start_utc: u8,
    pub window_hours: u8,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            revision: 0,
            agent_initiated: false,
            check_minutes: 60,
            install: "on_request".into(),
            channel: "stable".into(),
            window_start_utc: 2,
            window_hours: 2,
        }
    }
}
impl Policy {
    pub fn validate(&self) -> ApiResult<()> {
        if ![0, 15, 60, 360, 1440].contains(&self.check_minutes)
            || !["on_request", "when_idle", "window"].contains(&self.install.as_str())
            || !["stable", "development"].contains(&self.channel.as_str())
            || self.window_start_utc > 23
            || !(1..=24).contains(&self.window_hours)
        {
            return Err(ApiError::bad(
                "Choose a supported update schedule and release channel",
            ));
        }
        Ok(())
    }
}
pub fn policy(state: &AppState) -> ApiResult<Policy> {
    state
        .get_record("updates", "policy")?
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| ApiError::bad("Invalid update policy"))
        .map(|p| p.unwrap_or_default())
}
pub fn status(state: &AppState) -> ApiResult<Value> {
    let helper = state.get_record("updates", "helper")?.unwrap_or(json!({}));
    let connected = helper["observed_at"]
        .as_i64()
        .is_some_and(|t| now_ms() - t < 90000);
    let gate = state.update_admission.lock().unwrap();
    Ok(
        json!({"schema":"constellation.installation-updates/1","policy":policy(state)?,"helper_connected":connected,
        "helper":helper,"active_prompts":gate.1,"draining":gate.0>now_ms(),
        "request":state.get_record("updates", "request")?}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub action: String,
    pub request_id: String,
    pub plan_id: Option<String>,
}
pub fn queue(state: &AppState, body: Command, agent: bool) -> ApiResult<Value> {
    if !["check", "plan", "update", "apply", "rollback"].contains(&body.action.as_str())
        || uuid::Uuid::parse_str(&body.request_id).is_err()
        || body
            .plan_id
            .as_ref()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
        || (["apply", "rollback"].contains(&body.action.as_str()) && body.plan_id.is_none())
    {
        return Err(ApiError::bad(
            "Choose check, plan, update, apply or rollback with a UUID request_id; apply/rollback also need plan_id",
        ));
    }
    let _lock = state.update_commands.lock().unwrap();
    let p = policy(state)?;
    if agent && !p.agent_initiated {
        return Err(ApiError::bad(
            "Agent-initiated updates are disabled in this instance's Updates settings",
        ));
    }
    if let Some(previous) = state.get_record("updates", "request")? {
        if previous["id"] == body.request_id {
            return Ok(previous);
        }
        if previous["pending"] == true {
            return Err(ApiError::conflict(previous));
        }
    }
    let helper = state.get_record("updates", "helper")?.unwrap_or(json!({}));
    if helper["observed_at"]
        .as_i64()
        .is_none_or(|t| now_ms() - t >= 90000)
    {
        return Err(ApiError::bad(
            "The installation's update helper is offline. Start its installed update service.",
        ));
    }
    let request = json!({"id":body.request_id,"action":body.action,"plan_id":body.plan_id,"actor":if agent{"agent"}else{"operator"},"policy_revision":p.revision,"created_at":now_ms(),"pending":true});
    state.set_record("updates", "request", request.clone())?;
    state.event(
        "updates.requested",
        json!({"id":body.request_id,"action":body.action,"actor":request["actor"]}),
    )?;
    Ok(request)
}
pub async fn get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "observe", false)?;
    Ok(Json(status(&state)?))
}
pub async fn configure(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut body): Json<Policy>,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", true)?;
    body.validate()?;
    let _lock = state.update_commands.lock().unwrap();
    let current = policy(&state)?;
    if body.revision != current.revision {
        return Err(ApiError::conflict(json!(current)));
    }
    body.revision += 1;
    state.set_record("updates", "policy", json!(body))?;
    state.event("updates.configured", json!({"revision":body.revision}))?;
    Ok(Json(json!(body)))
}
pub async fn command(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Command>,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", true)?;
    Ok(Json(queue(&state, body, false)?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub status: Value,
    pub finished_request: Option<String>,
    #[serde(default)]
    pub drain_seconds: u16,
}
pub async fn report(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut body): Json<Report>,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", true)?;
    if !body.status.is_object()
        || body.status.to_string().len() > 128 * 1024
        || body.drain_seconds > 60
    {
        return Err(ApiError::bad("Invalid helper report"));
    }
    let _lock = state.update_commands.lock().unwrap();
    let until = if body.drain_seconds > 0 {
        now_ms() + i64::from(body.drain_seconds) * 1000
    } else {
        0
    };
    state.update_admission.lock().unwrap().0 = until;
    body.status["drain_until"] = json!(until);
    body.status["observed_at"] = json!(now_ms());
    let previous = state.get_record("updates", "helper")?.unwrap_or(json!({}));
    if previous["state"] != body.status["state"] || previous["error"] != body.status["error"] {
        state.event(
            "updates.progress",
            json!({"state":body.status["state"],"error":body.status["error"]}),
        )?;
    }
    state.set_record("updates", "helper", body.status)?;
    if let Some(id) = body.finished_request
        && let Some(mut request) = state.get_record("updates", "request")?
        && request["id"] == id
    {
        request["pending"] = json!(false);
        state.set_record("updates", "request", request)?;
    }
    Ok(Json(status(&state)?))
}
/// Lifetime covers the prompt through response forwarding, including errors/disconnects.
pub struct Admission(Arc<AppState>);
impl Drop for Admission {
    fn drop(&mut self) {
        self.0.update_admission.lock().unwrap().1 -= 1;
    }
}
pub fn admit(state: &Arc<AppState>) -> Option<Admission> {
    let mut gate = state.update_admission.lock().unwrap();
    if gate.0 > now_ms() {
        return None;
    }
    gate.1 += 1;
    Some(Admission(state.clone()))
}
pub fn mcp_tools() -> Vec<Value> {
    vec![
        json!({"name":"installation_updates","description":"Check installed component versions and update availability, prepare a plan, install the latest update when idle, or roll back. Agent-initiated updates are controlled by one instance setting, independent of who requests them. Use status/check first; update checks and installs in one durable request. An accepted request is queued, not completed; inspect status for the final receipt after reconnecting. No shell command is needed.","inputSchema":{"type":"object","properties":{"action":{"type":"string","enum":["status","check","plan","update","apply","rollback"]},"request_id":{"type":"string"},"plan_id":{"type":"string"}},"required":["action"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":true}}),
    ]
}
pub fn mcp_call(state: &AppState, args: &Value) -> ApiResult<Value> {
    if args["action"] == "status" {
        return status(state);
    }
    let mut args = args.clone();
    if args.get("request_id").is_none() {
        args["request_id"] = json!(uuid::Uuid::new_v4().to_string());
    }
    let command =
        serde_json::from_value(args).map_err(|_| ApiError::bad("Invalid update command"))?;
    queue(state, command, true)
}
