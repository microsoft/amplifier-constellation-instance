//! Saved experience connections. Only the installation-owned host worker executes commands.
use crate::{
    auth,
    error::{ApiError, ApiResult},
    state::{AppState, now_ms},
};
use axum::{Json, extract::State, http::HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn enrolled_default() -> bool {
    true
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Service {
    #[serde(default = "enrolled_default")]
    pub enrolled: bool,
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub setup_url: Option<String>,
    #[serde(default)]
    pub add: Option<Add>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub value: String,
    pub label: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: String,
    pub label: String,
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    pub max_length: Option<usize>,
    pub min: Option<i64>,
    pub max: Option<i64>,
    #[serde(default)]
    pub choices: Vec<Choice>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Add {
    pub revision: String,
    pub fields: Vec<Field>,
}
impl Add {
    fn valid(&self) -> bool {
        let mut names = BTreeSet::new();
        !self.revision.is_empty()
            && self.revision.len() <= 128
            && self.fields.len() <= 16
            && self.fields.iter().all(|f| {
                !f.name.is_empty()
                    && f.name.len() <= 64
                    && f.name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && ![
                        "password",
                        "token",
                        "secret",
                        "authorization",
                        "__proto__",
                        "constructor",
                        "prototype",
                    ]
                    .contains(&f.name.to_ascii_lowercase().as_str())
                    && label(&f.label)
                    && names.insert(&f.name)
                    && match f.kind.as_str() {
                        "text" => f.max_length.is_some_and(|n| n > 0 && n <= 1000),
                        "integer" => matches!((f.min,f.max),(Some(a),Some(b)) if a<=b),
                        "boolean" => true,
                        "choice" => {
                            !f.choices.is_empty()
                                && f.choices.len() <= 32
                                && f.choices.iter().all(|c| {
                                    label(&c.label) && !c.value.is_empty() && c.value.len() <= 1000
                                })
                        }
                        _ => false,
                    }
            })
    }
    fn accepts(&self, values: &BTreeMap<String, Value>) -> bool {
        self.valid()
            && values
                .keys()
                .all(|k| self.fields.iter().any(|f| &f.name == k))
            && self.fields.iter().all(|f| match values.get(&f.name) {
                None => !f.required,
                Some(v) => match f.kind.as_str() {
                    "text" => v.as_str().is_some_and(|s| {
                        s.len() <= f.max_length.unwrap_or(0)
                            && (!f.required || !s.trim().is_empty())
                    }),
                    "integer" => v
                        .as_i64()
                        .is_some_and(|n| n >= f.min.unwrap() && n <= f.max.unwrap()),
                    "boolean" => v.is_boolean(),
                    "choice" => v
                        .as_str()
                        .is_some_and(|s| f.choices.iter().any(|c| c.value == s)),
                    _ => false,
                },
            })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    #[serde(default)]
    pub settings_url: Option<String>,
    #[serde(default = "enrolled_default")]
    pub enrolled: bool,
    pub id: String,
    pub service: String,
    pub label: String,
    pub revision: String,
    pub context_provider_ids: Vec<String>,
    pub state: String,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub settled: bool,
    #[serde(default)]
    pub context_ready: bool,
    #[serde(default)]
    pub context_quiesced: bool,
    #[serde(default)]
    pub failed: bool,
    #[serde(default)]
    pub message: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Command {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connection_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, Value>,
    pub revision: String,
    pub action: String,
    pub request_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Operation {
    pub command: Command,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    #[serde(default)]
    pub result: Option<Value>,
}
#[derive(Default, Serialize, Deserialize)]
struct Registry {
    services: Vec<Service>,
    connections: Vec<Connection>,
    observed_at: i64,
    // Gates survive absent enrollment and worker restarts; removal is not activation.
    gates: BTreeMap<String, bool>,
    requests: BTreeMap<String, Operation>,
    pending: Option<String>,
}
fn read(s: &AppState) -> ApiResult<Registry> {
    Ok(s.get_record("service_connections", "registry")?
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| ApiError::bad("Invalid saved connections registry"))?
        .unwrap_or_default())
}
fn write(s: &AppState, r: &Registry) -> ApiResult<()> {
    s.set_record("service_connections", "registry", json!(r))?;
    Ok(())
}
fn name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c))
}
fn label(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 200
}
pub fn setup_url(s: &str) -> bool {
    reqwest::Url::parse(s).is_ok_and(|u| {
        (u.scheme() == "https"
            || (u.scheme() == "http"
                && u.host_str().is_some_and(|h| {
                    h.trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|i| i.is_loopback())
                })))
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.fragment().is_none()
    })
}
pub fn context_enabled(s: &AppState, id: &str) -> anyhow::Result<bool> {
    Ok(read(s)
        .map_err(|e| anyhow::anyhow!(e.1))?
        .gates
        .get(id)
        .copied()
        .unwrap_or(true))
}
fn public(r: &Registry) -> Value {
    let connections: Vec<Value> = r
        .connections
        .iter()
        .map(|c| {
            let mut v = json!(c);
            v["context_active"] = json!(
                c.context_provider_ids
                    .iter()
                    .all(|id| r.gates.get(id).copied().unwrap_or(true))
            );
            v["desired_state"] = r
                .requests
                .values()
                .filter(|op| op.command.connection_id == c.id)
                .max_by_key(|op| op.created_at)
                .map(|op| {
                    json!(if op.command.action == "connect" {
                        "connected"
                    } else {
                        "disconnected"
                    })
                })
                .unwrap_or(Value::Null);
            v
        })
        .collect();
    json!({"schema":1,"services":r.services,"connections":connections,"observed_at":r.observed_at,
        "helper_connected":r.observed_at>0 && now_ms()-r.observed_at<90000,
        "request":r.pending.as_ref().and_then(|id| r.requests.get(id)),
        "last_operation":r.requests.values().filter(|op|op.finished_at.is_some()).max_by_key(|op|op.finished_at)})
}
pub async fn get(State(s): State<Arc<AppState>>, h: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "admin", false)?;
    Ok(Json(public(&read(&s)?)))
}
pub async fn command(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Json(c): Json<Command>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "admin", true)?;
    if !(if c.action == "add" {
        c.connection_id.is_empty() && c.service_id.as_deref().is_some_and(name)
    } else {
        name(&c.connection_id) && c.service_id.is_none() && c.values.is_empty()
    }) || !["add", "connect", "disconnect"].contains(&c.action.as_str())
        || uuid::Uuid::parse_str(&c.request_id).is_err()
    {
        return Err(ApiError::bad(
            "Choose Add, Connect or Disconnect for a saved connection with a UUID request_id",
        ));
    }
    let _lock = s.service_commands.lock().unwrap();
    let mut r = read(&s)?;
    if let Some(old) = r.requests.get(&c.request_id) {
        return if old.command == c {
            Ok(Json(json!(old)))
        } else {
            Err(ApiError::conflict(json!({"error":"request_id_reused"})))
        };
    }
    if r.pending.is_some() {
        return Err(ApiError::conflict(public(&r)));
    }
    if now_ms() - r.observed_at >= 90000 {
        return Err(ApiError::bad(
            "The installation connection helper is offline",
        ));
    }
    if c.action == "add" {
        let service = r
            .services
            .iter()
            .find(|e| e.enrolled && Some(&e.id) == c.service_id.as_ref())
            .and_then(|e| e.add.as_ref())
            .filter(|a| a.revision == c.revision)
            .ok_or_else(|| ApiError::conflict(json!({"error":"service_changed"})))?;
        if !service.accepts(&c.values) {
            return Err(ApiError::bad("Review the connection setup fields"));
        }
    } else {
        let entry = r
            .connections
            .iter()
            .find(|e| e.enrolled && e.id == c.connection_id && e.revision == c.revision)
            .ok_or_else(|| ApiError::conflict(json!({"error":"connection_changed"})))?;
        // The queue is not execution. The serialized host worker closes context
        // through its matching report before dispatch, after any update finishes.
        let _ = entry;
    }
    let op = Operation {
        command: c.clone(),
        created_at: now_ms(),
        finished_at: None,
        result: None,
    };
    r.pending = Some(c.request_id.clone());
    r.requests.insert(c.request_id.clone(), op.clone());
    write(&s, &r)?;
    s.event(
        "service_connection.requested",
        json!({"connection_id":c.connection_id,"action":c.action,"request_id":c.request_id}),
    )?;
    Ok(Json(json!(op)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub schema: u8,
    pub services: Vec<Service>,
    pub connections: Vec<Connection>,
    #[serde(default)]
    pub add_result: Option<AddResult>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddResult {
    pub service_id: String,
    pub request_id: String,
    pub state: String,
    pub settled: bool,
    pub connection_id: Option<String>,
    #[serde(default)]
    pub message: String,
}
pub async fn report(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Json(b): Json<Report>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "admin", true)?;
    let mut services = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut providers = BTreeSet::new();
    if b.schema != 1
        || b.services.len() > 32
        || b.connections.len() > 64
        || b.services.iter().any(|v| {
            !name(&v.id)
                || !label(&v.label)
                || v.setup_url.as_deref().is_some_and(|u| !setup_url(u))
                || v.add.as_ref().is_some_and(|a| !a.valid())
                || !services.insert(v.id.clone())
        })
        || b.connections.iter().any(|c| {
            !name(&c.id)
                || !services.contains(&c.service)
                || !label(&c.label)
                || c.settings_url.as_deref().is_some_and(|u| !setup_url(u))
                || c.revision.is_empty()
                || c.revision.len() > 128
                || !ids.insert(c.id.clone())
                || !["connected", "disconnected", "busy", "unknown"].contains(&c.state.as_str())
                || c.message.len() > 1000
                || c.context_provider_ids.len() > 16
                || c.context_provider_ids
                    .iter()
                    .any(|id| !name(id) || !providers.insert(id.clone()))
                || c.request_id
                    .as_ref()
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
        })
    {
        return Err(ApiError::bad("Invalid installation connection report"));
    }
    let _lock = s.service_commands.lock().unwrap();
    let mut r = read(&s)?;
    // Do not lose a pending connection or change its immutable command context.
    if let Some(op) = r.pending.as_ref().and_then(|id| r.requests.get(id)) {
        if op.command.action == "add" {
            let old = r
                .services
                .iter()
                .find(|s| Some(&s.id) == op.command.service_id.as_ref())
                .unwrap();
            if !b
                .services
                .iter()
                .any(|s| s.id == old.id && s.add == old.add)
            {
                return Err(ApiError::conflict(
                    json!({"error":"pending_service_changed"}),
                ));
            }
        } else {
            let old = r
                .connections
                .iter()
                .find(|c| c.id == op.command.connection_id)
                .unwrap();
            if !b.connections.iter().any(|c| {
                c.id == old.id
                    && c.revision == old.revision
                    && c.context_provider_ids == old.context_provider_ids
            }) {
                return Err(ApiError::conflict(
                    json!({"error":"pending_connection_changed"}),
                ));
            }
        }
    }
    for c in &b.connections {
        for id in &c.context_provider_ids {
            r.gates.entry(id.clone()).or_insert(c.state == "connected");
            if c.state == "disconnected" {
                r.gates.insert(id.clone(), false);
            }
        }
    }
    if let Some(id) = r.pending.clone() {
        let op = r.requests.get_mut(&id).unwrap();
        if op.command.action == "add" {
            if let Some(result) = b.add_result.as_ref().filter(|a| {
                a.request_id == id
                    && Some(&a.service_id) == op.command.service_id.as_ref()
                    && a.message.len() <= 1000
                    && a.settled
                    && (a.state == "failed"
                        || (a.state == "saved"
                            && b.connections.iter().any(|c| {
                                Some(&c.id) == a.connection_id.as_ref() && c.service == a.service_id
                            })))
            }) {
                op.finished_at = Some(now_ms());
                op.result = Some(json!(result));
                r.pending = None;
            }
        } else if let Some(c) = b
            .connections
            .iter()
            .find(|c| c.id == op.command.connection_id && c.request_id.as_deref() == Some(&id))
        {
            let success = c.settled
                && !c.failed
                && c.state
                    == if op.command.action == "connect" {
                        "connected"
                    } else {
                        "disconnected"
                    };
            for provider in &c.context_provider_ids {
                if op.command.action == "connect" {
                    r.gates
                        .insert(provider.clone(), !c.failed && (success || c.context_ready));
                } else if c.context_quiesced || success || c.failed {
                    // Preserve active-turn grants during domain admission/drain. Once
                    // quiesced, no subsequent busy report may reopen this gate.
                    r.gates.insert(provider.clone(), false);
                }
            }
            if success || (c.settled && c.failed && c.state != "unknown" && c.state != "busy") {
                op.finished_at = Some(now_ms());
                op.result = Some(json!({"state":c.state,"failed":c.failed,"message":c.message}));
                r.pending = None;
            }
        }
    }
    // Omitted connections remain saved and unavailable; removal never deletes history or opens context.
    for old in &r.connections {
        if !ids.contains(&old.id) {
            for id in &old.context_provider_ids {
                r.gates.insert(id.clone(), false);
            }
        }
    }
    let mut connections = b.connections;
    for c in &mut connections {
        c.enrolled = true;
    }
    for mut old in r.connections {
        if !ids.contains(&old.id) {
            old.enrolled = false;
            old.state = "unknown".into();
            old.message = "Connection is not currently enrolled with the host helper".into();
            connections.push(old);
        }
    }
    r.connections = connections;
    let mut services = b.services;
    for s in &mut services {
        s.enrolled = true;
    }
    for mut old in r.services {
        if !services.iter().any(|s| s.id == old.id) {
            old.enrolled = false;
            services.push(old);
        }
    }
    r.services = services;
    r.observed_at = now_ms();
    write(&s, &r)?;
    Ok(Json(public(&r)))
}
