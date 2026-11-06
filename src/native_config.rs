//! Optional management of the separately versioned native engine. Generic ACP is unchanged.
use crate::{
    auth,
    config::{expand, write_private},
    error::{ApiError, ApiResult},
    scheduler,
    state::AppState,
};
use axum::{Json, extract::State, http::HeaderMap};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub expected_revision: String,
    pub expected_backend_sha256: String,
    pub config: Value,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    pub expected_backend_sha256: String,
    pub value: String,
}

/// Store a provider credential and connect its environment reference in one
/// owner operation. The secret is never returned, logged, or stored in UI state.
pub async fn credential(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Credential>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "admin", true)?.actor;
    if body.expected_backend_sha256 != scheduler::backend_hash(&state) {
        return Err(ApiError::conflict(
            json!({"reason":"configured_backend_changed"}),
        ));
    }
    if body.value.trim().is_empty() || body.value.len() > 64 * 1024 {
        return Err(ApiError::bad("Enter a provider API key"));
    }
    let mut document = state.config.read().unwrap().clone();
    let native = invoke(&state, "config-inspect", None).await?;
    let key = native["config"]["provider"]["api_key_env"]
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        })
        .ok_or_else(|| ApiError::bad("Choose a provider with API-key authentication first"))?;
    let secret_name = format!("native-model-key-{}", uuid::Uuid::new_v4());
    let path = state.home.join("secrets").join(&secret_name);
    write_private(&path, body.value.trim().as_bytes())?;
    document
        .config
        .backend
        .as_mut()
        .ok_or_else(|| ApiError::bad("No engine configured"))?
        .secret_env
        .insert(key.into(), secret_name);
    let applied = crate::server::replace_config(&state, document.revision, document.config, &actor);
    if let Err(error) = applied {
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    state.event("native.credential_configured", json!({"actor":actor}))?;
    Ok(Json(json!({"configured":true,"value_returned":false})))
}
pub(crate) fn installation(state: &AppState) -> ApiResult<(PathBuf, PathBuf)> {
    let cfg = state.instance_config();
    let b = cfg
        .backend
        .as_ref()
        .ok_or_else(|| ApiError::bad("No engine configured"))?;
    if ![Some("agent-runtime"), Some("agent-runtime.exe")]
        .contains(&Path::new(&b.command).file_name().and_then(|n| n.to_str()))
    {
        return Err(ApiError::bad(
            "This optional editor requires the native Agent Runtime; other ACP engines retain their own configuration interface",
        ));
    }
    let cwd = PathBuf::from(expand(&state.home, &b.working_directory));
    let absolute = |v: &str| {
        let p = PathBuf::from(expand(&state.home, v));
        if p.is_absolute() { p } else { cwd.join(p) }
    };
    let engine = absolute(&b.command);
    let home = b
        .args
        .iter()
        .enumerate()
        .find_map(|(i, a)| {
            if a == "--home" {
                b.args.get(i + 1).map(|v| absolute(v))
            } else {
                a.strip_prefix("--home=").map(absolute)
            }
        })
        .ok_or_else(|| ApiError::bad("The native engine needs an explicit --home argument"))?;
    if !engine.is_file() || !home.is_dir() {
        return Err(ApiError::bad(
            "The configured native engine installation is unavailable",
        ));
    }
    Ok((engine, home))
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
pub(crate) async fn invoke(
    state: &AppState,
    operation: &str,
    input: Option<&Update>,
) -> ApiResult<Value> {
    run(
        state,
        operation,
        input.map(|i| &i.config),
        input.map(|i| i.expected_revision.as_str()),
    )
    .await
}
/// Private local management call. This never forwards model/connector credentials.
pub(crate) async fn history_query(state: &AppState, query: &Value) -> ApiResult<Value> {
    run(state, "history-query", Some(query), None).await
}
async fn run(
    state: &AppState,
    operation: &str,
    input: Option<&Value>,
    expected_revision: Option<&str>,
) -> ApiResult<Value> {
    let (engine, home) = installation(state)?;
    let mut cmd = Command::new(engine);
    cmd.arg("--home")
        .arg(&home)
        .arg(operation)
        .env_clear()
        .current_dir(&home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for key in ["PATH", "LANG", "SYSTEMROOT", "WINDIR", "TEMP", "TMP"] {
        if let Ok(v) = std::env::var(key) {
            cmd.env(key, v);
        }
    }
    // Configuration commands do not need model credentials or the owner's provider accounts.
    cmd.env("HOME", &home).env("USERPROFILE", &home);
    let temporary = if let Some(body) = input {
        if body.to_string().len() > 1024 * 1024 {
            return Err(ApiError::bad("Engine configuration is too large"));
        }
        let path = state
            .home
            .join(format!(".native-config-{}.json", uuid::Uuid::new_v4()));
        write_private(&path, &serde_json::to_vec(body).unwrap())?;
        cmd.arg(&path);
        if operation == "configure" {
            cmd.arg("--expected-revision")
                .arg(expected_revision.unwrap_or_default());
        }
        Some(Temporary(path))
    } else {
        None
    };
    let mut child = cmd
        .spawn()
        .map_err(|_| ApiError::bad("Could not start the native configuration command"))?;
    let mut output = child.stdout.take().unwrap().take(1024 * 1024 + 1);
    let mut errors = child.stderr.take().unwrap().take(8193);
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            output.read_to_end(&mut stdout),
            errors.read_to_end(&mut stderr)
        )?;
        Ok::<_, std::io::Error>((status, stdout, stderr))
    })
    .await;
    drop(temporary);
    let (status, stdout, stderr) = result
        .map_err(|_| ApiError::bad("The engine configuration command timed out"))?
        .map_err(|_| ApiError::bad("Could not read the engine configuration result"))?;
    if stdout.len() > 1024 * 1024 || stderr.len() > 8192 {
        return Err(ApiError::bad(
            "The engine returned too much configuration output",
        ));
    }
    if !status.success() {
        // Runtime's structured validation describes the field, never credential values.
        let message = String::from_utf8_lossy(&stderr).trim().to_owned();
        if (message.to_lowercase().contains("revision")
            && message.to_lowercase().contains("conflict"))
            || message.to_lowercase().contains("configuration changed")
        {
            return Err(ApiError::conflict(
                json!({"reason":"engine_configuration_changed"}),
            ));
        }
        return Err(ApiError::bad(if message.is_empty() {
            "The engine does not support this configuration operation. Update its version first."
                .to_owned()
        } else {
            message
        }));
    }
    serde_json::from_slice(&stdout)
        .map_err(|_| ApiError::bad("The engine returned an unsupported configuration response"))
}
pub async fn inspect(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", false)?;
    let mut result = invoke(&state, "config-inspect", None).await?;
    result["backend_sha256"] = json!(scheduler::backend_hash(&state));
    result["management_profile"] = json!("agent-instance.native-config/1");
    result["credential_mapping"] = json!(
        state
            .instance_config()
            .backend
            .as_ref()
            .map(|b| &b.secret_env)
    );
    Ok(Json(result))
}
pub async fn schema(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", false)?;
    Ok(Json(invoke(&state, "config-schema", None).await?))
}
fn check_revision(state: &AppState, body: &Update) -> ApiResult<()> {
    if body.expected_revision.len() != 64
        || !body
            .expected_revision
            .chars()
            .all(|c| c.is_ascii_hexdigit())
    {
        return Err(ApiError::bad(
            "Use the revision returned by the engine inspector",
        ));
    }
    if body.expected_backend_sha256 != scheduler::backend_hash(state) {
        return Err(ApiError::conflict(
            json!({"reason":"configured_backend_changed"}),
        ));
    }
    Ok(())
}
pub async fn validate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Update>,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", true)?;
    check_revision(&state, &body)?;
    Ok(Json(invoke(&state, "config-validate", Some(&body)).await?))
}
pub async fn save(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Update>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "admin", true)?.actor;
    check_revision(&state, &body)?;
    if crate::native_account::pending(&state) {
        return Err(ApiError::bad(
            "Finish or cancel account sign-in before changing the engine",
        ));
    }
    // Avoid changing the semantics beneath already-authorized background jobs.
    if scheduler::inspect(&state)?["runs"]
        .as_array()
        .is_some_and(|runs| {
            runs.iter().any(|r| {
                ["queued", "starting", "running", "cancelling"]
                    .contains(&r["state"].as_str().unwrap_or(""))
            })
        })
    {
        return Err(ApiError::bad(
            "Wait for or cancel pending background runs before changing the engine",
        ));
    }
    let mut result = invoke(&state, "configure", Some(&body)).await?;
    result["restart_required"] = json!(true);
    result["active_connections_unchanged"] = json!(true);
    state.event(
        "native.configured",
        json!({"actor":actor,"revision":result["revision"],"restart_required":true}),
    )?;
    Ok(Json(result))
}

/// Content-free execution evidence, exposed independently of this UI.
pub async fn observations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "observe", false)?;
    Ok(Json(invoke(&state, "diagnostics", None).await?))
}
