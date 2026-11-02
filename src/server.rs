use crate::{
    acp, activity, auth,
    config::{InstanceConfig, valid_name, write_private},
    context,
    error::{ApiError, ApiResult},
    memory, native_account, native_config, profile_use, profiles, provision, scheduler,
    state::{AppState, now_ms},
    views, welcome, work,
};
use axum::{
    Json, Router,
    body::Body,
    extract::{
        DefaultBodyLimit, Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::{get, post, put},
};
use futures_util::{SinkExt, StreamExt};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    convert::Infallible,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(welcome::routes())
        .route("/", get(index))
        .route("/app.js", get(js))
        .route("/app.css", get(css))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/session", get(auth::session))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/status", get(status))
        .route("/api/v1/descriptor", get(descriptor))
        .route("/api/v1/schema", get(schema))
        .route("/api/v1/openapi.json", get(openapi))
        .route(
            "/api/v1/organization",
            get(work::organization_get).put(work::organization_put),
        )
        .route("/api/v1/scheduler", get(scheduler::get).put(scheduler::put))
        .route(
            "/api/v1/work/{id}/execution",
            get(scheduler::get_execution).post(scheduler::post_execution),
        )
        .route(
            "/api/v1/memory",
            get(memory::list_api).post(memory::remember_api),
        )
        .route("/api/v1/memory/{id}", get(memory::get_api))
        .route("/api/v1/memory/{id}/commands", post(memory::command_api))
        .route("/api/v1/work", get(work::list).post(work::create))
        .route("/api/v1/work/{id}", get(work::get))
        .route("/api/v1/work/{id}/commands", post(work::command))
        .route("/api/v1/work/{id}/export", post(work::export))
        .route("/api/v1/work/import", post(work::import))
        .route("/api/v1/profile-use", get(profile_use::get))
        .route("/api/v1/config", get(config_get).put(config_put))
        .route("/api/v1/config/validate", post(config_validate))
        .route(
            "/api/v1/native-config",
            get(native_config::inspect).put(native_config::save),
        )
        .route("/api/v1/native-config/schema", get(native_config::schema))
        .route(
            "/api/v1/native-account",
            get(native_account::status).delete(native_account::disconnect),
        )
        .route(
            "/api/v1/native-account/login",
            get(native_account::login_status)
                .post(native_account::start)
                .delete(native_account::cancel),
        )
        .route(
            "/api/v1/native-account/models",
            post(native_account::models),
        )
        .route(
            "/api/v1/native-config/validate",
            post(native_config::validate),
        )
        .route("/api/v1/backend", get(backend_status))
        .route("/api/v1/backend/install", post(backend_install))
        .route(
            "/api/v1/native-config/credential",
            post(crate::native_config::credential),
        )
        .route("/api/v1/backend/account", get(backend_account))
        .route(
            "/api/v1/backend/account/login",
            get(backend_login_status)
                .post(backend_login_start)
                .delete(backend_login_cancel),
        )
        .route("/api/v1/lifecycle", post(lifecycle))
        .route("/api/v1/connections", get(connections))
        .route("/api/v1/sessions", get(sessions))
        .route("/api/v1/sessions/{id}", get(session))
        .route(
            "/api/v1/profile-package",
            get(profiles::get).post(profiles::apply),
        )
        .route("/api/v1/profile-package/restore", post(profiles::restore))
        .route("/api/v1/profiles/{name}", get(profile_get).put(profile_put))
        .route("/api/v1/secrets", get(secrets))
        .route(
            "/api/v1/secrets/{name}",
            put(secret_put).delete(secret_delete),
        )
        .route("/api/v1/tokens", get(tokens).post(token_create))
        .route("/api/v1/tokens/{id}", axum::routing::delete(token_delete))
        .route("/api/v1/activity", get(activity::list))
        .route("/api/v1/events", get(events))
        .route("/api/v1/events/stream", get(event_stream))
        .route("/api/v1/views", get(views::list).post(views::create))
        .route("/api/v1/views/{id}", get(views::get).patch(views::patch))
        .route("/api/v1/views/{id}/commands", post(views::command))
        .route("/api/v1/views/{id}/ack", post(views::ack))
        .route(
            "/api/v1/context/providers",
            get(context::get).put(context::put),
        )
        .route("/context/mcp", post(context::mcp).get(context::mcp_get))
        .route("/api/v1/acp", get(acp_upgrade))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security_headers,
        ))
        .with_state(state)
}
async fn backend_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "observe", false)?;
    Ok(Json(provision::status(
        &state.home,
        &state.instance_config(),
    )))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallBackend {
    #[serde(default = "native_engine_default")]
    engine: String,
    #[serde(default = "reuse_account_default")]
    reuse_owner_account: bool,
}
fn native_engine_default() -> String {
    "native".into()
}
fn reuse_account_default() -> bool {
    true
}
async fn backend_install(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<InstallBackend>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "admin", true)?.actor;
    let mut document = state.config.read().unwrap().clone();
    document.config.backend = Some(
        match body.engine.as_str() {
            "native" => provision::install_native(&state.home, None).await,
            "codex" => provision::install_codex(&state.home, body.reuse_owner_account).await,
            _ => return Err(ApiError::bad("Choose native or codex")),
        }
        .map_err(|e| ApiError::bad(e.to_string()))?,
    );
    replace_config(&state, document.revision, document.config, &actor)?;
    state.event(
        "backend.installed",
        json!({"actor":actor,"runtime":if body.engine == "native" {"Constellation"} else {"Codex"}}),
    )?;
    Ok(Json(provision::status(
        &state.home,
        &state.instance_config(),
    )))
}
async fn backend_account(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", false)?;
    Ok(Json(
        provision::account_read(&state.home)
            .await
            .map_err(|e| ApiError::bad(e.to_string()))?,
    ))
}
async fn backend_login_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", true)?;
    Ok(Json(
        provision::login_start(&state.home)
            .await
            .map_err(|e| ApiError::bad(e.to_string()))?,
    ))
}
async fn backend_login_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", false)?;
    Ok(Json(
        provision::login_status(&state.home)
            .await
            .map_err(|e| ApiError::bad(e.to_string()))?,
    ))
}
async fn backend_login_cancel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", true)?;
    Ok(Json(
        provision::login_cancel(&state.home)
            .await
            .map_err(|e| ApiError::bad(e.to_string()))?,
    ))
}
async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../ui/index.html"),
    )
}
async fn js() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        include_str!("../ui/app.js"),
    )
}
async fn css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../ui/app.css"),
    )
}
async fn security_headers(
    State(state): State<Arc<AppState>>,
    mut request: axum::http::Request<Body>,
    next: Next,
) -> Response {
    // HTTP/2 carries :authority in the URI instead of a Host header.
    // Normalize that protocol field before cookie-origin validation.
    if !request.headers().contains_key(header::HOST)
        && let Some(authority) = request.uri().authority().map(|a| a.as_str().to_owned())
        && let Ok(value) = authority.parse()
    {
        request.headers_mut().insert(header::HOST, value);
    }
    let is_welcome = request.uri().path().starts_with("/welcome");
    let mut response = next.run(request).await;
    let origins = state.instance_config().server.embed_origins.join(" ");
    let connect = if is_welcome {
        "'self' https:"
    } else {
        "'self'"
    };
    let csp = format!(
        "default-src 'self'; script-src 'self'; style-src 'self'; connect-src {connect}; img-src 'self' data:; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'self' {origins}"
    );
    let h = response.headers_mut();
    h.insert("content-security-policy", csp.parse().unwrap());
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
    h.insert("referrer-policy", "no-referrer".parse().unwrap());
    h.insert("cache-control", "no-store".parse().unwrap());
    response
}
async fn config_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", false)?;
    Ok(Json(json!(*state.config.read().unwrap())))
}
async fn schema(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "observe", false)?;
    Ok(Json(json!(schemars::schema_for!(InstanceConfig))))
}
async fn openapi(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "observe", false)?;
    Ok(Json(crate::discovery::openapi()))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigUpdate {
    expected_revision: u64,
    config: InstanceConfig,
}
pub fn replace_config(
    state: &AppState,
    expected: u64,
    config: InstanceConfig,
    actor: &str,
) -> ApiResult<Value> {
    config
        .validate()
        .map_err(|e| ApiError::bad(e.to_string()))?;
    let document = {
        let mut current = state.config.write().unwrap();
        if current.revision != expected {
            return Err(ApiError::conflict(json!({"revision":current.revision})));
        }
        if current.config.instance_id != config.instance_id || current.config.owner != config.owner
        {
            return Err(ApiError::bad(
                "Instance ID and owner are fixed at installation",
            ));
        }
        let next = crate::config::ConfigDocument {
            revision: expected + 1,
            config,
        };
        write_private(
            &state.home.join("config.json"),
            &serde_json::to_vec_pretty(&next).unwrap(),
        )?;
        let enabled = next.config.enabled;
        *current = next.clone();
        state.runtime_enabled.store(enabled, Ordering::SeqCst);
        if !enabled {
            let _ = state.runtime_stop.send(());
        }
        next
    };
    state.event("config.changed",json!({"actor":actor,"revision":document.revision,"applies":"new ACP connections; server settings after service restart"}))?;
    Ok(json!(document))
}
async fn config_put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ConfigUpdate>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "admin", true)?.actor;
    Ok(Json(replace_config(
        &state,
        body.expected_revision,
        body.config,
        &actor,
    )?))
}
async fn config_validate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<InstanceConfig>,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", true)?;
    body.validate().map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(json!({"valid":true,"config":body})))
}
pub fn status_value(state: &AppState) -> ApiResult<Value> {
    let document = state.config.read().unwrap().clone();
    let config = &document.config;
    let connections: Vec<Value> = state
        .connections
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect();
    let sessions = state.list_records("sessions")?;
    let addresses = state.addresses.read().unwrap().clone();
    Ok(
        json!({"instance_id":config.instance_id,"name":config.name,"setup_complete":config.setup_complete,"enabled":state.enabled(),"version":env!("CARGO_PKG_VERSION"),"platform":std::env::consts::OS,"authentication":crate::platform::auth_method(),"uptime_ms":now_ms()-state.started_at,"connections":connections,"sessions":sessions,"config_revision":document.revision,"addresses":addresses,"backend":{"configured":config.backend.is_some(),"command":config.backend.as_ref().map(|b|&b.command)},"counts":{"connections":connections.len(),"sessions":sessions.len()},"restart_required":addresses.get("effective_server").is_some_and(|s|*s!=json!(config.server)),"profile_activation":"Profiles are supplied on each new prompt; in-flight turns retain their context"}),
    )
}
async fn status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "observe", false)?;
    Ok(Json(status_value(&state)?))
}
async fn descriptor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "observe", false)?;
    let mut document = json!({"api_version":"1","instance_id":state.instance_config().instance_id,"implementation":{"name":"agent-instance","version":env!("CARGO_PKG_VERSION")},"schema":"api/v1/schema","profile_packages":{"accepted_schemas":["twin-profile/1"],"activation":"next_prompt","rollback":true,"path":"api/v1/profile-package"},"capabilities":["config","profiles","profile-packages","secrets","runtime-control","observations","ui-state","runtime-setup","certificate-help","conversation-continuity","agent-instance.organization/1","agent-instance.work/1","agent-instance.delegation/1","agent-instance.profile-use/1","agent-instance.scheduler/1","agent-instance.memory/1","agent-instance.native-config/1"],"acp":{"transport":"stdio","attach":state.addresses.read().unwrap()["acp_attach"],"remote_bridge":{"profile":"agent-instance.wss/1","path":"api/v1/acp","authentication":"Bearer token with acp scope","standard_acp_transport":false}},"ui":{"entry":"./","embed_protocol":"agent-instance.ui/1","embed_origins":state.instance_config().server.embed_origins},"operations":[{"path":"native-config","methods":["GET","PUT"],"scope":"admin"},{"path":"native-config/schema","methods":["GET"],"scope":"admin"},{"path":"native-config/validate","methods":["POST"],"scope":"admin"},{"path":"memory","methods":["GET"],"scope":"memory:read"},{"path":"memory","methods":["POST"],"scope":"memory"},{"path":"memory/{id}","methods":["GET"],"scope":"memory:read"},{"path":"memory/{id}/commands","methods":["POST"],"scope":"memory"},{"path":"scheduler","methods":["GET"],"scope":"work:read"},{"path":"scheduler","methods":["PUT"],"scope":"work:execute"},{"path":"work/{id}/execution","methods":["GET"],"scope":"work:read"},{"path":"work/{id}/execution","methods":["POST"],"scope":"work:execute"},{"path":"organization","methods":["GET"],"scope":"work:read"},{"path":"organization","methods":["PUT"],"scope":"admin"},{"path":"work","methods":["GET"],"scope":"work:read"},{"path":"work","methods":["POST"],"scope":"work"},{"path":"work/{id}","methods":["GET"],"scope":"work:read"},{"path":"work/{id}/commands","methods":["POST"],"scope":"work"},{"path":"work/{id}/export","methods":["POST"],"scope":"work"},{"path":"work/import","methods":["POST"],"scope":"work"},{"path":"profile-use","methods":["GET"],"scope":"observe"},{"path":"profile-package","methods":["GET","POST"],"scope":"profiles"},{"path":"profile-package/restore","methods":["POST"],"scope":"profiles"},{"path":"context/providers","methods":["GET","PUT"],"scope":"admin"},{"path":"activity","methods":["GET"],"scope":"observe"},{"path":"backend","methods":["GET"],"scope":"observe"},{"path":"backend/install","methods":["POST"],"scope":"admin"},{"path":"backend/account","methods":["GET"],"scope":"admin"},{"path":"backend/account/login","methods":["GET","POST","DELETE"],"scope":"admin"},{"path":"config","methods":["GET","PUT"],"scope":"admin"},{"path":"config/validate","methods":["POST"],"scope":"admin"},{"path":"lifecycle","methods":["POST"],"scope":"admin"},{"path":"profiles/{name}","methods":["GET","PUT"],"scope":"admin"},{"path":"secrets/{name}","methods":["PUT","DELETE"],"scope":"admin"},{"path":"tokens","methods":["GET","POST"],"scope":"admin"},{"path":"connections","methods":["GET"],"scope":"observe"},{"path":"sessions","methods":["GET"],"scope":"observe"},{"path":"events","methods":["GET"],"scope":"observe"},{"path":"views","methods":["GET","POST"],"scope":"ui"},{"path":"views/{id}","methods":["GET","PATCH"],"scope":"ui"},{"path":"views/{id}/commands","methods":["POST"],"scope":"ui"},{"path":"views/{id}/ack","methods":["POST"],"scope":"ui"}]});
    document["operations"]
        .as_array_mut()
        .unwrap()
        .push(json!({"path":"native-config/credential","methods":["POST"],"scope":"admin"}));
    if native_config::installation(&state).is_ok() {
        document["capabilities"]
            .as_array_mut()
            .unwrap()
            .push(json!("agent-instance.native-account/1"));
        document["operations"].as_array_mut().unwrap().extend([
            json!({"path":"native-account","methods":["GET","DELETE"],"scope":"admin"}),
            json!({"path":"native-account/login","methods":["GET","POST","DELETE"],"scope":"admin"}),
            json!({"path":"native-account/models","methods":["POST"],"scope":"admin"})]);
    } else {
        document["capabilities"]
            .as_array_mut()
            .unwrap()
            .retain(|v| v != "agent-instance.native-config/1");
        document["operations"]
            .as_array_mut()
            .unwrap()
            .retain(|v| !v["path"].as_str().unwrap_or("").starts_with("native-"));
    }
    Ok(Json(document))
}
async fn lifecycle(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "admin", true)?.actor;
    let mut document = state.config.read().unwrap().clone();
    document.config.enabled = match body["action"].as_str() {
        Some("start") => true,
        Some("stop") => false,
        _ => return Err(ApiError::bad("Action must be start or stop")),
    };
    replace_config(&state, document.revision, document.config, &actor)?;
    state.event(
        "runtime.changed",
        json!({"actor":actor,"enabled":state.enabled()}),
    )?;
    Ok(Json(
        json!({"enabled":state.enabled(),"active_connections":state.connections.lock().unwrap().len()}),
    ))
}
async fn connections(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "observe", false)?;
    Ok(Json(json!(
        state
            .connections
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>()
    )))
}
async fn sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "observe", false)?;
    Ok(Json(json!(state.list_records("sessions")?)))
}
async fn session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "observe", false)?;
    Ok(Json(
        state
            .get_record("sessions", &id)?
            .ok_or_else(ApiError::missing)?,
    ))
}
fn profile_path(state: &AppState, name: &str) -> ApiResult<std::path::PathBuf> {
    if !["communication", "working-style", "identity"].contains(&name) {
        return Err(ApiError::missing());
    }
    Ok(state.home.join("profiles").join(format!("{name}.md")))
}
async fn profile_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", false)?;
    profile_path(&state, &name)?;
    let text = profiles::read(&state, &name)?;
    let sha256 = format!("{:x}", Sha256::digest(text.as_bytes()));
    Ok(Json(json!({"name":name,"text":text,"sha256":sha256})))
}
async fn profile_put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "admin", true)?.actor;
    profile_path(&state, &name)?;
    let text = body["text"]
        .as_str()
        .filter(|s| s.len() <= 128 * 1024)
        .ok_or_else(|| ApiError::bad("Profile must be text, at most 128 KiB"))?;
    profiles::write(&state, &name, text)?;
    let sha256 = format!("{:x}", Sha256::digest(text.as_bytes()));
    state.event(
        "profile.changed",
        json!({"actor":actor,"name":name,"sha256":sha256}),
    )?;
    Ok(Json(json!({"name":name,"text":text,"sha256":sha256})))
}
async fn secrets(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", false)?;
    let names: Vec<Value> = std::fs::read_dir(state.home.join("secrets"))?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (valid_name(&name) && name != "control-token")
                .then(|| json!({"name":name,"configured":true}))
        })
        .collect();
    Ok(Json(json!(names)))
}
fn secret_path(state: &AppState, name: &str) -> ApiResult<std::path::PathBuf> {
    if !valid_name(name) || name == "control-token" {
        return Err(ApiError::bad(
            "Choose a non-reserved secret name (letters, digits, hyphen, underscore)",
        ));
    }
    Ok(state.home.join("secrets").join(name))
}
async fn secret_put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "admin", true)?.actor;
    let path = secret_path(&state, &name)?;
    let value = body["value"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 64 * 1024 && !s.contains('\0'))
        .ok_or_else(|| ApiError::bad("Secret must contain 1–65536 bytes without NUL"))?;
    write_private(&path, value.as_bytes())?;
    state.event("secret.changed", json!({"actor":actor,"name":name}))?;
    Ok(Json(json!({"name":name,"configured":true})))
}
async fn secret_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "admin", true)?.actor;
    let path = secret_path(&state, &name)?;
    if state
        .instance_config()
        .backend
        .as_ref()
        .is_some_and(|b| b.secret_env.values().any(|v| v == &name))
    {
        return Err(ApiError::bad(
            "Remove this secret's configuration reference before deleting it",
        ));
    }
    if context::registry(&state)?
        .providers
        .iter()
        .any(|provider| provider.secret_ref == name)
    {
        return Err(ApiError::bad(
            "Remove this secret's context provider reference before deleting it",
        ));
    }
    match std::fs::remove_file(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    state.event("secret.deleted", json!({"actor":actor,"name":name}))?;
    Ok(Json(json!({"ok":true})))
}
async fn tokens(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", false)?;
    let db = state.db.lock().unwrap();
    let mut query = db.prepare(
        "SELECT id,name,scopes,expires_at,created_at FROM tokens ORDER BY created_at DESC",
    )?;
    let rows=query.query_map([],|r|{let scopes:String=r.get(2)?;Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"scopes":serde_json::from_str::<Value>(&scopes).unwrap_or(json!([])),"expires_at":r.get::<_,i64>(3)?,"created_at":r.get::<_,i64>(4)?}))})?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Json(json!(rows)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenRequest {
    name: String,
    scopes: Vec<String>,
    expires_in_seconds: u64,
}
async fn token_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<TokenRequest>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "admin", true)?.actor;
    let value = auth::issue_token(&state, &body.name, &body.scopes, body.expires_in_seconds)?;
    state.event(
        "token.created",
        json!({"actor":actor,"id":value["id"],"scopes":value["scopes"]}),
    )?;
    Ok(Json(value))
}
async fn token_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "admin", true)?.actor;
    state
        .db
        .lock()
        .unwrap()
        .execute("DELETE FROM tokens WHERE id=?1", [&id])?;
    state.event("token.revoked", json!({"actor":actor,"id":id}))?;
    Ok(Json(
        json!({"ok":true,"existing_connections":"Revocation closes authenticated streams within 15 seconds"}),
    ))
}
#[derive(Deserialize, Default)]
struct EventQuery {
    #[serde(default)]
    after: i64,
}
fn event_access(state: &AppState, headers: &HeaderMap) -> ApiResult<bool> {
    if auth::check(state, headers, "observe", false).is_ok() {
        Ok(true)
    } else {
        auth::check(state, headers, "ui", false)?;
        Ok(false)
    }
}
fn events_after(state: &AppState, after: i64, observe: bool) -> ApiResult<Vec<Value>> {
    let db = state.db.lock().unwrap();
    let mut query=db.prepare("SELECT seq,at,kind,data FROM events WHERE seq>?1 AND (?2 OR kind LIKE 'view.%') ORDER BY seq LIMIT 500")?;
    let rows=query.query_map(params![after,observe],|r|{let data:String=r.get(3)?;Ok(json!({"seq":r.get::<_,i64>(0)?,"at":r.get::<_,i64>(1)?,"kind":r.get::<_,String>(2)?,"data":serde_json::from_str::<Value>(&data).unwrap_or(Value::Null)}))})?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
async fn events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<EventQuery>,
) -> ApiResult<Json<Value>> {
    let observe = event_access(&state, &headers)?;
    let events = events_after(&state, query.after, observe)?;
    let next = events
        .last()
        .and_then(|v| v["seq"].as_i64())
        .unwrap_or(query.after);
    Ok(Json(json!({"events":events,"next_after":next})))
}
async fn event_stream(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<EventQuery>,
) -> ApiResult<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>> {
    event_access(&state, &headers)?;
    let after = headers
        .get("last-event-id")
        .and_then(|s| s.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(query.after);
    let receiver = state.events.subscribe();
    let stream = futures_util::stream::unfold(
        (state, headers, receiver, after, VecDeque::<Value>::new()),
        |(state, headers, mut receiver, mut after, mut queue)| async move {
            let observe = event_access(&state, &headers).ok()?;
            if queue.is_empty() {
                queue.extend(events_after(&state, after, observe).ok()?);
            }
            if queue.is_empty() {
                let _ = tokio::time::timeout(Duration::from_secs(10), receiver.recv()).await;
                queue.extend(events_after(&state, after, observe).ok()?);
            }
            // Recheck grants after waiting, before yielding any data.
            event_access(&state, &headers).ok()?;
            let event = if let Some(value) = queue.pop_front() {
                after = value["seq"].as_i64().unwrap_or(after);
                Event::default()
                    .id(after.to_string())
                    .data(value.to_string())
            } else {
                Event::default().comment("keep-alive")
            };
            Some((Ok(event), (state, headers, receiver, after, queue)))
        },
    );
    Ok(Sse::new(stream))
}
async fn acp_upgrade(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    if headers.get(header::AUTHORIZATION).is_none() {
        return Err(ApiError::unauthorized());
    }
    let principal = auth::check(&state, &headers, "acp", false)?;
    if !state.enabled() {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Complete setup and start the agent before connecting".into(),
            None,
        ));
    }
    let maximum = state.instance_config().limits.max_frame_bytes;
    Ok(ws
        .max_message_size(maximum)
        .max_frame_size(maximum)
        .on_upgrade(move |socket| acp_socket(state, headers, principal.actor, socket)))
}
async fn acp_socket(state: Arc<AppState>, headers: HeaderMap, actor: String, socket: WebSocket) {
    let (mut sink, mut source) = socket.split();
    let (input_tx, input_rx) = tokio::sync::mpsc::channel(32);
    let (output_tx, mut output_rx) = tokio::sync::mpsc::channel(32);
    let relay = tokio::spawn(acp::relay_authenticated(
        state.clone(),
        actor,
        headers.clone(),
        input_rx,
        output_tx,
    ));
    {
        let input = async {
            while let Some(Ok(message)) = source.next().await {
                match message {
                    Message::Text(text) => {
                        let Ok(value) = serde_json::from_str::<Value>(&text) else {
                            break;
                        };
                        if input_tx.send(value).await.is_err() {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    Message::Binary(_) => break,
                    _ => (),
                }
            }
        };
        let output = async {
            while let Some(value) = output_rx.recv().await {
                if sink
                    .send(Message::Text(value.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        };
        let granted = async {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                if auth::check(&state, &headers, "acp", false).is_err() {
                    break;
                }
            }
        };
        tokio::select! {_=input=>(),_=output=>(),_=granted=>()}
    }
    drop(input_tx);
    drop(output_rx);
    // Closing either queue asks the relay to terminate its child process group.
    let _ = tokio::time::timeout(Duration::from_secs(10), relay).await;
}
