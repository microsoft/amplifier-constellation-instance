use crate::{
    auth,
    error::{ApiError, ApiResult},
    state::{AppState, now_ms},
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::sync::Arc;

pub async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "ui", false)?;
    Ok(Json(json!(state.list_records("views")?)))
}
pub async fn create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "ui", true)?.actor;
    let label = body["label"].as_str().unwrap_or("Browser");
    if label.len() > 200 {
        return Err(ApiError::bad("View label too long"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let value = json!({"id":id,"label":label,"revision":1,"updated_at":now_ms(),"actor":actor,"ack":null,"state":{"route":if state.instance_config().setup_complete{"dashboard"}else{"setup"},"step":0,"selected_session":null,"filters":{},"draft":{},"focus":null,"scroll":{"x":0,"y":0},"modal":null}});
    state.set_record("views", &id, value.clone())?;
    state.event("view.changed", json!({"id":id,"revision":1,"actor":actor}))?;
    Ok(Json(value))
}
pub async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "ui", false)?;
    Ok(Json(
        state
            .get_record("views", &id)?
            .ok_or_else(ApiError::missing)?,
    ))
}
fn valid_view(value: &Value) -> ApiResult<()> {
    if !value.is_object()
        || ![
            "setup",
            "dashboard",
            "configuration",
            "profiles",
            "connections",
            "activity",
            "access",
            "work",
            "memory",
            "organization",
        ]
        .contains(&value["route"].as_str().unwrap_or(""))
    {
        return Err(ApiError::bad("Unknown view route"));
    }
    if value.to_string().len() > 256 * 1024 {
        return Err(ApiError::bad("View state too large"));
    }
    fn sensitive(v: &Value) -> bool {
        match v {
            Value::Object(m) => m.iter().any(|(k, v)| {
                [
                    "password",
                    "token",
                    "secret",
                    "secret_value",
                    "authorization",
                    "csrf",
                    "control-token",
                ]
                .contains(&k.to_lowercase().as_str())
                    || sensitive(v)
            }),
            Value::Array(a) => a.iter().any(sensitive),
            _ => false,
        }
    }
    if sensitive(&value["draft"]) {
        return Err(ApiError::bad(
            "Passwords and credentials must stay out of view state",
        ));
    }
    if !value["draft"].is_object()
        || !value["filters"].is_object()
        || value["step"].as_u64().is_none_or(|v| v > 20)
    {
        return Err(ApiError::bad("Invalid view state"));
    }
    Ok(())
}
fn set_path(root: &mut Value, path: &str, value: Value) -> ApiResult<()> {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.is_empty()
        || parts.len() > 12
        || parts
            .iter()
            .any(|s| s.is_empty() || ["__proto__", "prototype", "constructor"].contains(s))
    {
        return Err(ApiError::bad("Invalid draft path"));
    }
    let mut cursor = root;
    for part in &parts[..parts.len() - 1] {
        if !cursor.is_object() {
            *cursor = json!({});
        }
        cursor = cursor
            .as_object_mut()
            .unwrap()
            .entry(part.to_string())
            .or_insert_with(|| json!({}));
    }
    if !cursor.is_object() {
        *cursor = json!({});
    }
    cursor
        .as_object_mut()
        .unwrap()
        .insert(parts.last().unwrap().to_string(), value);
    Ok(())
}
pub fn apply_command(state: &mut Value, command: &Value) -> ApiResult<()> {
    match command["type"].as_str().unwrap_or("") {
        "navigate" => {
            state["route"] = command["route"].clone();
            if command.get("step").is_some() {
                state["step"] = command["step"].clone();
            }
        }
        "set_draft" => set_path(
            &mut state["draft"],
            command["path"]
                .as_str()
                .ok_or_else(|| ApiError::bad("Draft path required"))?,
            command["value"].clone(),
        )?,
        "focus" => state["focus"] = command["target"].clone(),
        "scroll" => {
            state["scroll"] = json!({"x":command["x"].as_f64().unwrap_or(0.0),"y":command["y"].as_f64().unwrap_or(0.0)})
        }
        "select_session" => state["selected_session"] = command["session_id"].clone(),
        "set_filter" => {
            let key = command["key"]
                .as_str()
                .filter(|s| s.len() < 100)
                .ok_or_else(|| ApiError::bad("Filter key required"))?;
            state["filters"][key] = command["value"].clone();
        }
        "open_modal" => state["modal"] = command["modal"].clone(),
        "close_modal" => state["modal"] = Value::Null,
        _ => return Err(ApiError::bad("Unknown view command")),
    }
    valid_view(state)
}
fn update(
    state: &AppState,
    id: &str,
    body: Value,
    actor: String,
    command: bool,
) -> ApiResult<Value> {
    let expected = body["expected_revision"]
        .as_u64()
        .ok_or_else(|| ApiError::bad("expected_revision is required"))?;
    let value = {
        let db = state.db.lock().unwrap();
        let text: Option<String> = db
            .query_row(
                "SELECT value FROM records WHERE collection='views' AND id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        let mut value: Value = serde_json::from_str(&text.ok_or_else(ApiError::missing)?)
            .map_err(|_| ApiError::bad("Invalid stored view"))?;
        if value["revision"].as_u64() != Some(expected) {
            return Err(ApiError::conflict(json!({"revision":value["revision"]})));
        }
        if command {
            apply_command(&mut value["state"], &body["command"])?;
        } else {
            value["state"] = body["state"].clone();
            valid_view(&value["state"])?;
        }
        value["revision"] = json!(expected + 1);
        value["actor"] = json!(actor);
        value["updated_at"] = json!(now_ms());
        db.execute(
            "UPDATE records SET value=?1,updated_at=?2 WHERE collection='views' AND id=?3",
            params![value.to_string(), now_ms(), id],
        )?;
        value
    };
    state.event(
        "view.changed",
        json!({"id":id,"revision":value["revision"],"actor":actor}),
    )?;
    Ok(value)
}
pub async fn patch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "ui", true)?.actor;
    Ok(Json(update(&state, &id, body, actor, false)?))
}
pub async fn command(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "ui", true)?.actor;
    Ok(Json(update(&state, &id, body, actor, true)?))
}
pub async fn ack(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "ui", true)?.actor;
    if !["applied", "error"].contains(&body["status"].as_str().unwrap_or("")) {
        return Err(ApiError::bad(
            "Acknowledgement status must be applied or error",
        ));
    }
    let value = {
        let db = state.db.lock().unwrap();
        let text: Option<String> = db
            .query_row(
                "SELECT value FROM records WHERE collection='views' AND id=?1",
                [&id],
                |r| r.get(0),
            )
            .optional()?;
        let mut value: Value = serde_json::from_str(&text.ok_or_else(ApiError::missing)?).unwrap();
        if value["revision"] != body["revision"] {
            return Err(ApiError::conflict(json!({"revision":value["revision"]})));
        }
        value["ack"] = json!({"revision":body["revision"],"status":body["status"],"detail":body["detail"].as_str().unwrap_or("").chars().take(500).collect::<String>(),"at":now_ms(),"actor":actor});
        db.execute(
            "UPDATE records SET value=?1,updated_at=?2 WHERE collection='views' AND id=?3",
            params![value.to_string(), now_ms(), id],
        )?;
        value
    };
    state.event(
        "view.ack",
        json!({"id":id,"revision":body["revision"],"status":body["status"],"actor":actor}),
    )?;
    Ok(Json(value))
}
