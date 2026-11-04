//! Owner setup for development tools. Execution environments remain independent.
use crate::{
    auth,
    config::write_private,
    error::{ApiError, ApiResult},
    state::{AppState, now_ms},
    tool_connections::Connection,
};
use axum::{Json, extract::State, http::HeaderMap};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, process::Stdio, sync::Arc, time::Duration};
use tokio::io::AsyncReadExt;

pub const ENDPOINT: &str = "https://api.githubcopilot.com/mcp/";
pub const TOOLS: &str = "get_me,get_file_contents,list_branches,list_commits,get_commit,search_code,search_pull_requests,pull_request_read,create_branch,push_files,create_pull_request";

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Connect {
    pub expected_revision: u64,
    /// Explicitly authorize this installation's local ACP attach clients.
    #[serde(default)]
    pub enable_local_clients: bool,
    /// Omit to use the owner's already signed-in GitHub CLI account.
    #[serde(default)]
    pub credential: Option<String>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Disconnect {
    pub expected_revision: u64,
}

fn github(state: &AppState) -> Option<Connection> {
    state
        .instance_config()
        .tool_connections
        .into_iter()
        .find(|c| c.url == ENDPOINT)
}
fn local_access(state: &AppState) -> bool {
    let Ok(token) = crate::acp::read_token_file(&state.home.join("secrets/acp-token")) else {
        return false;
    };
    let mut headers = HeaderMap::new();
    let Ok(value) = format!("Bearer {token}").parse() else {
        return false;
    };
    headers.insert("authorization", value);
    auth::explicit_scope(state, &headers, "tools:use")
        && auth::check(state, &headers, "acp", false).is_ok()
}

pub fn summary(state: &AppState) -> ApiResult<Value> {
    let configured = github(state).is_some();
    let stored = state.get_record("development", "github")?;
    let verification = stored
        .filter(|v| {
            github(state).is_some_and(|c| {
                c.header_secrets.get("Authorization").is_some_and(|s| {
                    state
                        .read_secret(s)
                        .is_ok_and(|secret| v["credential_digest"] == auth::digest(&secret))
                })
            })
        })
        .map(|mut v| {
            v.as_object_mut().unwrap().remove("credential_digest");
            v
        });
    let native = crate::native_config::installation(state)
        .ok()
        .and_then(|(_, home)| {
            std::fs::read(home.join("runtime.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        });
    let environment = if let Some(c) = native {
        json!({"kind":"native_isolated_workspace","build_verified":false,"network":c.get("command_network").and_then(Value::as_str).unwrap_or("isolated"), "command_seconds":c["limits"]["command_seconds"],
            "message":"Local files and isolated commands are available. Repository checkout, dependency installation, and a project build have not been verified."})
    } else {
        json!({"kind":"external_acp_backend","build_verified":false,"message":"Execution is provided by the connected agent. A project build has not been verified."})
    };
    Ok(
        json!({"config_revision":state.config.read().unwrap().revision,"github":{"configured":configured,"verification":verification,
        "local_clients_enabled":local_access(state),"credential_boundary":"GitHub access follows the connected credential. Tool selection does not restrict repositories."},
        "environment":environment,"ready_to_develop":false}),
    )
}
pub async fn status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", false)?;
    Ok(Json(summary(&state)?))
}

async fn host_credential() -> ApiResult<String> {
    // No shell, no model-selected executable, no inherited token overrides, and
    // no credential in process arguments or errors. The owner opts in explicitly.
    let mut cmd = tokio::process::Command::new(if cfg!(windows) { "gh.exe" } else { "gh" });
    cmd.args(["auth", "token", "--hostname", "github.com"])
        .env_clear()
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_HOST", "github.com")
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in [
        "HOME",
        "USERPROFILE",
        "PATH",
        "XDG_CONFIG_HOME",
        "SYSTEMROOT",
        "LOCALAPPDATA",
        "DBUS_SESSION_BUS_ADDRESS",
    ] {
        if let Ok(v) = std::env::var(key) {
            cmd.env(key, v);
        }
    }
    let error = || {
        ApiError::bad(
            "Sign in to GitHub on this computer with gh auth login, or use a GitHub token in the private field.",
        )
    };
    let mut child = cmd.spawn().map_err(|_| error())?;
    let mut output = child.stdout.take().unwrap().take(8193);
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let mut bytes = Vec::new();
        output.read_to_end(&mut bytes).await?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status.success(), bytes))
    })
    .await
    .map_err(|_| error())?
    .map_err(|_| error())?;
    if !result.0 || result.1.len() > 8192 {
        return Err(error());
    }
    String::from_utf8(result.1).map_err(|_| error())
}

#[derive(Serialize, Deserialize)]
struct Verified {
    checked_at: i64,
    tool_names: Vec<String>,
}

/// Verify the actual tool endpoint, without writing to GitHub. Never return a
/// remote error body: it can contain a credential or organization sign-in data.
async fn verify(authorization: &str) -> ApiResult<Verified> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| ApiError::bad("Unable to check GitHub"))?;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "authorization",
        authorization
            .parse()
            .map_err(|_| ApiError::bad("Invalid GitHub credential"))?,
    );
    headers.insert("x-mcp-tools", TOOLS.parse().unwrap());
    headers.insert(
        "accept",
        "application/json, text/event-stream".parse().unwrap(),
    );
    async fn rpc(
        client: &reqwest::Client,
        headers: &mut reqwest::header::HeaderMap,
        body: Value,
    ) -> ApiResult<Value> {
        let mut r = client
            .post(ENDPOINT)
            .headers(headers.clone())
            .json(&body)
            .send()
            .await
            .map_err(|_| {
                ApiError::bad("GitHub could not be reached. Check the connection and try again.")
            })?;
        if !r.status().is_success() {
            return Err(ApiError::bad(
                "GitHub did not accept this connection. Check account access and organization authorization, then reconnect.",
            ));
        }
        if let Some(id) = r.headers().get("mcp-session-id") {
            headers.insert("mcp-session-id", id.clone());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = r
            .chunk()
            .await
            .map_err(|_| ApiError::bad("GitHub check was interrupted"))?
        {
            if bytes.len() + chunk.len() > 1024 * 1024 {
                return Err(ApiError::bad("GitHub returned an oversized response"));
            }
            bytes.extend_from_slice(&chunk);
        }
        if body.get("id").is_none() {
            return Ok(Value::Null);
        }
        let raw = String::from_utf8(bytes).map_err(|_| ApiError::bad("Invalid GitHub response"))?;
        let value = serde_json::from_str::<Value>(&raw)
            .ok()
            .or_else(|| {
                raw.lines()
                    .filter_map(|l| l.strip_prefix("data:"))
                    .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
                    .find(|v| v["id"] == body["id"])
            })
            .ok_or_else(|| ApiError::bad("Invalid GitHub tool response"))?;
        if value.get("error").is_some() || value["result"]["isError"] == true {
            return Err(ApiError::bad(
                "GitHub tools could not verify this account. Reconnect after checking access.",
            ));
        }
        Ok(value["result"].clone())
    }
    let init = rpc(&client, &mut headers, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"constellation-development-setup","version":"1"}}})).await?;
    if let Some(v) = init["protocolVersion"].as_str() {
        headers.insert(
            "mcp-protocol-version",
            v.parse()
                .map_err(|_| ApiError::bad("Invalid tool protocol"))?,
        );
    }
    rpc(
        &client,
        &mut headers,
        json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
    )
    .await?;
    let list = rpc(
        &client,
        &mut headers,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    )
    .await?;
    let names: Vec<String> = list["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect();
    if ![
        "get_me",
        "get_file_contents",
        "create_branch",
        "push_files",
        "create_pull_request",
    ]
    .iter()
    .all(|n| names.iter().any(|x| x == n))
    {
        return Err(ApiError::bad(
            "GitHub did not offer the repository and pull-request tools needed for development.",
        ));
    }
    rpc(&client, &mut headers, json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_me","arguments":{}}})).await?;
    Ok(Verified {
        checked_at: now_ms(),
        tool_names: names,
    })
}

/// Add tools to the explicit local attach credential, preserving its identity,
/// audience grants, expiry, and remote client credentials. Never modify admin.
pub fn enable_local(state: &AppState) -> ApiResult<()> {
    let path = state.home.join("secrets/acp-token");
    if path.symlink_metadata().is_ok() {
        let token = crate::acp::read_token_file(&path).map_err(|_| {
            ApiError::bad("The local agent credential needs repair before enabling tools")
        })?;
        let db = state.db.lock().unwrap();
        let raw: Option<String> = db
            .query_row(
                "SELECT scopes FROM tokens WHERE hash=?1 AND expires_at>?2",
                params![auth::digest(&token), now_ms()],
                |r| r.get(0),
            )
            .optional()?;
        let mut scopes: Vec<String> =
            raw.and_then(|s| serde_json::from_str(&s).ok())
                .ok_or_else(|| {
                    ApiError::bad("The local agent credential expired; reconnect the local client")
                })?;
        if !scopes.iter().any(|s| s == "acp") || scopes.iter().any(|s| s == "admin") {
            return Err(ApiError::bad(
                "Use a scoped ACP credential for local agent tools",
            ));
        }
        if !scopes.iter().any(|s| s == "tools:use") {
            scopes.push("tools:use".into());
            db.execute(
                "UPDATE tokens SET scopes=?1 WHERE hash=?2",
                params![
                    serde_json::to_string(&scopes).unwrap(),
                    auth::digest(&token)
                ],
            )?;
        }
    } else {
        let v = auth::issue_token(
            state,
            "Local development tools",
            &["acp".into(), "context:owner".into(), "tools:use".into()],
            366 * 86400,
        )?;
        if let Err(e) = write_private(&path, v["token"].as_str().unwrap().as_bytes()) {
            state.db.lock().unwrap().execute(
                "DELETE FROM tokens WHERE id=?1",
                [v["id"].as_str().unwrap()],
            )?;
            return Err(e.into());
        }
    }
    Ok(())
}

pub fn install(state: &AppState, expected: u64, authorization: &str, actor: &str) -> ApiResult<()> {
    let mut doc = state.config.read().unwrap().clone();
    if expected != doc.revision {
        return Err(ApiError::conflict(json!({"revision":doc.revision})));
    }
    let suffix = uuid::Uuid::new_v4();
    let auth_name = format!("development-github-auth-{suffix}");
    let tools_name = format!("development-github-tools-{suffix}");
    let name = github(state)
        .map(|c| c.name)
        .unwrap_or_else(|| "project-github".into());
    // Keep unrelated connections, including project/world tools.
    doc.config.tool_connections.retain(|c| c.url != ENDPOINT);
    doc.config.tool_connections.push(Connection {
        name,
        url: ENDPOINT.into(),
        header_secrets: BTreeMap::from([
            ("Authorization".into(), auth_name.clone()),
            ("X-MCP-Tools".into(), tools_name.clone()),
        ]),
    });
    doc.config
        .validate()
        .map_err(|e| ApiError::bad(e.to_string()))?;
    write_private(
        &state.home.join("secrets").join(&auth_name),
        authorization.as_bytes(),
    )?;
    if let Err(e) = write_private(
        &state.home.join("secrets").join(&tools_name),
        TOOLS.as_bytes(),
    ) {
        let _ = std::fs::remove_file(state.home.join("secrets").join(&auth_name));
        return Err(e.into());
    }
    if let Err(e) = crate::server::replace_config(state, expected, doc.config, actor) {
        let _ = std::fs::remove_file(state.home.join("secrets").join(auth_name));
        let _ = std::fs::remove_file(state.home.join("secrets").join(tools_name));
        return Err(e);
    }
    Ok(())
}

pub async fn connect(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Connect>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "admin", true)?.actor;
    if state.config.read().unwrap().revision != body.expected_revision {
        return Err(ApiError::conflict(
            json!({"reason":"configuration_changed"}),
        ));
    }
    let credential = match body.credential {
        Some(s) => s,
        None => host_credential().await?,
    };
    let credential = credential.trim();
    if credential.is_empty() || credential.len() > 8192 || credential.contains(['\r', '\n', '\0']) {
        return Err(ApiError::bad("Enter a valid GitHub token"));
    }
    let authorization = format!("Bearer {credential}");
    let verified = verify(&authorization).await?;
    install(&state, body.expected_revision, &authorization, &actor)?;
    let mut receipt = serde_json::to_value(verified).unwrap();
    receipt["credential_digest"] = json!(auth::digest(&authorization));
    state.set_record("development", "github", receipt)?;
    let local_error = if body.enable_local_clients {
        enable_local(&state).err().map(|e| e.1)
    } else {
        None
    };
    state.event(
        "development.github_connected",
        json!({"actor":actor,"local_clients_enabled":local_access(&state)}),
    )?;
    let mut result = summary(&state)?;
    result["local_clients_error"] = json!(local_error);
    Ok(Json(result))
}
pub async fn check(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", true)?;
    let c = github(&state).ok_or_else(|| ApiError::bad("Connect GitHub first"))?;
    let secret = c
        .header_secrets
        .get("Authorization")
        .ok_or_else(|| ApiError::bad("Reconnect GitHub to restore its credential"))?;
    let authorization = state
        .read_secret(secret)
        .map_err(|_| ApiError::bad("Reconnect GitHub to restore its credential"))?;
    let verified = verify(&authorization).await;
    match verified {
        Ok(v) => {
            let mut receipt = serde_json::to_value(v).unwrap();
            receipt["credential_digest"] = json!(auth::digest(&authorization));
            state.set_record("development", "github", receipt)?;
        }
        Err(e) => {
            state.set_record("development", "github", Value::Null)?;
            return Err(e);
        }
    }
    Ok(Json(summary(&state)?))
}
pub async fn disconnect(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Disconnect>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "admin", true)?.actor;
    let mut doc = state.config.read().unwrap().clone();
    doc.config.tool_connections.retain(|c| c.url != ENDPOINT);
    crate::server::replace_config(&state, body.expected_revision, doc.config, &actor)?;
    state.set_record("development", "github", Value::Null)?;
    Ok(Json(summary(&state)?))
}
