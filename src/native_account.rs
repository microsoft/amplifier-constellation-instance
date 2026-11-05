//! This installation's native OAuth job. Credentials remain owned by the engine.
use crate::{
    auth,
    error::{ApiError, ApiResult},
    native_config, scheduler,
    state::{AppState, now_ms},
};
use axum::{Json, extract::State, http::HeaderMap};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
    sync::watch,
};

pub struct Login {
    id: String,
    value: Mutex<Value>,
    cancel: watch::Sender<bool>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Start {
    pub expected_backend_sha256: String,
    #[serde(default)]
    pub callback_port: u16,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cancel {
    pub job_id: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Disconnect {
    pub confirm: bool,
}

fn in_progress(value: &Value) -> bool {
    matches!(
        value["state"].as_str(),
        Some("starting" | "pending" | "cancelling")
    )
}
pub fn pending(state: &AppState) -> bool {
    state
        .native_login
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|job| in_progress(&job.value.lock().unwrap()))
}
fn public_account(value: &Value) -> Value {
    let mut result =
        json!({"connected":value["connected"]==true,"credential_values_included":false});
    for key in [
        "email",
        "sharing",
        "expires_at",
        "client_id",
        "subject",
        "usage_url",
        "revoked",
        "registration_preserved",
    ] {
        if let Some(v) = value.get(key)
            && (v.is_boolean() || v.is_number() || v.as_str().is_some_and(|s| s.len() <= 2048))
        {
            result[key] = v.clone();
        }
    }
    result
}
fn publish(state: &AppState, job: &Login, value: Value) -> ApiResult<()> {
    // The authorization URL contains short-lived state. It is never durable or observable.
    let mut durable = value.clone();
    if let Some(obj) = durable.as_object_mut() {
        obj.remove("authorization_url");
    }
    state.set_record("native_account", "login", durable)?;
    *job.value.lock().unwrap() = value;
    Ok(())
}
fn current(state: &AppState) -> ApiResult<Value> {
    if let Some(job) = state.native_login.lock().unwrap().as_ref() {
        return Ok(job.value.lock().unwrap().clone());
    }
    let mut value = state
        .get_record("native_account", "login")?
        .unwrap_or(json!({"state":"idle"}));
    if in_progress(&value) {
        value["state"] = json!("interrupted");
        value["message"] = json!(
            "The service restarted before sign-in finished. Check the account, then start again if needed."
        );
        state.set_record("native_account", "login", value.clone())?;
    }
    Ok(value)
}
fn pending_details(value: &Value, requested: u16) -> ApiResult<Value> {
    let invalid = || ApiError::bad("The native engine returned an unsupported sign-in address");
    let url = reqwest::Url::parse(value["authorization_url"].as_str().ok_or_else(invalid)?)
        .map_err(|_| invalid())?;
    let callback = reqwest::Url::parse(value["callback"].as_str().ok_or_else(invalid)?)
        .map_err(|_| invalid())?;
    let port = callback.port().ok_or_else(invalid)?;
    if url.scheme() != "https"
        || url.host_str() != Some("auth.openai.com")
        || url.path() != "/api/accounts/authorize"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || callback.scheme() != "http"
        || callback.host_str() != Some("127.0.0.1")
        || callback.path() != "/auth/callback"
        || !callback.username().is_empty()
        || callback.password().is_some()
        || callback.query().is_some()
        || callback.fragment().is_some()
        || port < 1024
        || (requested != 0 && port != requested)
    {
        return Err(invalid());
    }
    let redirects: Vec<_> = url
        .query_pairs()
        .filter(|(k, _)| k == "redirect_uri")
        .collect();
    if redirects.len() != 1 || redirects[0].1 != callback.as_str() {
        return Err(invalid());
    }
    let seconds = value["expires_in_seconds"]
        .as_u64()
        .filter(|n| (1..=300).contains(n))
        .ok_or_else(invalid)?;
    Ok(
        json!({"authorization_url":url.as_str(),"callback":callback.as_str(),"callback_port":port,"expires_at":now_ms()+seconds as i64*1000,"expires_in_seconds":seconds}),
    )
}
fn tunnel(state: &AppState, port: u16) -> Option<String> {
    let cfg = state.instance_config();
    let base = cfg.server.public_url.as_deref()?;
    let url = reqwest::Url::parse(base).ok()?;
    let host = url.host_str()?;
    let destination = format!("{}@{}", cfg.owner, host);
    if !destination
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"@._-:[]".contains(&b))
    {
        return None;
    }
    Some(format!(
        "ssh -N -o ExitOnForwardFailure=yes -L 127.0.0.1:{port}:127.0.0.1:{port} '{destination}'"
    ))
}
async fn run(
    state: Arc<AppState>,
    job: Arc<Login>,
    body: Start,
    mut cancel: watch::Receiver<bool>,
) {
    let mut stopped = state.runtime_stop.subscribe();
    let outcome=async {
        let (engine,home)=native_config::installation(&state)?;
        let mut command=Command::new(engine);
        command.arg("--home").arg(&home).args(["chatgpt-sign-in","--no-browser","--port"]).arg(body.callback_port.to_string())
            .env_clear().env("HOME",&home).env("USERPROFILE",&home).current_dir(&home)
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
        for key in ["PATH","LANG","SYSTEMROOT","WINDIR","TEMP","TMP"] {if let Ok(v)=std::env::var(key){command.env(key,v);}}
        #[cfg(target_os="linux")]
        unsafe {
            let parent=std::process::id() as libc::pid_t;
            command.pre_exec(move|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG,libc::SIGKILL)!=0 {return Err(std::io::Error::last_os_error());}
                if libc::getppid()!=parent {return Err(std::io::Error::other("Sign-in owner stopped"));}
                Ok(())
            });
        }
        let mut child=command.spawn().map_err(|_|ApiError::bad("The native engine could not start sign-in"))?;
        #[cfg(windows)]
        let _job=crate::platform::windows::ProcessJob::attach(child.id().ok_or_else(||ApiError::bad("Sign-in process stopped"))?).map_err(|_|ApiError::bad("Could not supervise the sign-in process"))?;
        let mut output=BufReader::new(child.stdout.take().unwrap());
        let mut first=Vec::new();
        let mut bounded=(&mut output).take(16385);
        let read=tokio::time::timeout(Duration::from_secs(15),bounded.read_until(b'\n',&mut first));
        tokio::select! {r=read=>{r.map_err(|_|ApiError::bad("The native engine did not start sign-in in time"))?.map_err(|_|ApiError::bad("Could not read the sign-in address"))?;},_=cancel.changed()=>{let _=child.kill().await;return Ok(("cancelled",None));},_=stopped.recv()=>{let _=child.kill().await;return Ok(("interrupted",None));}}
        if first.len()>16384{return Err(ApiError::bad("The native engine returned too much sign-in data"));}
        let first:Value=serde_json::from_slice(&first).map_err(|_|ApiError::bad("The installed engine could not start ChatGPT sign-in. Update the engine or choose another callback port."))?;
        let details=pending_details(&first,body.callback_port)?;
        if *cancel.borrow() {let _=child.kill().await;return Ok(("cancelled",None));}
        let mut value=json!({"id":job.id,"state":"pending","started_at":job.value.lock().unwrap()["started_at"],"local_browser_only":true,"provider_configuration_unchanged":true});
        for (k,v) in details.as_object().unwrap(){value[k]=v.clone();}
        value["tunnel_command"]=json!(tunnel(&state,details["callback_port"].as_u64().unwrap() as u16));
        publish(&state,&job,value)?;
        let seconds=details["expires_in_seconds"].as_u64().unwrap();
        let finish=async {
            let mut bytes=Vec::new();
            output.take(32769).read_to_end(&mut bytes).await.map_err(|_|ApiError::bad("Could not read the sign-in result"))?;
            if bytes.len()>32768{return Err(ApiError::bad("The engine returned too much sign-in data"));}
            let status=child.wait().await.map_err(|_|ApiError::bad("Could not read sign-in completion"))?;
            if !status.success(){return Err(ApiError::bad("ChatGPT sign-in did not finish. It may have been declined, expired, or the callback connection was unavailable."));}
            let result:Value=serde_json::from_slice(&bytes).map_err(|_|ApiError::bad("The native engine returned an unsupported sign-in result"))?;
            if result["connected"]!=true{return Err(ApiError::bad("The native engine did not confirm an account connection"));}
            if scheduler::backend_hash(&state)!=body.expected_backend_sha256{return Err(ApiError::bad("The engine configuration changed during sign-in. Inspect the account in its original engine home before continuing."));}
            Ok(public_account(&result))
        };
        tokio::select! {
            result=finish=>Ok(("connected",Some(result?))),
            _=tokio::time::sleep(Duration::from_secs(seconds))=>{let _=child.kill().await;Ok(("expired",None))},
            _=cancel.changed()=>{let _=child.kill().await;Ok(("cancelled",None))},
            _=stopped.recv()=>{let _=child.kill().await;Ok(("interrupted",None))}
        }
    }.await;
    let mut value = job.value.lock().unwrap().clone();
    if let Some(o) = value.as_object_mut() {
        o.remove("authorization_url");
    }
    value["finished_at"] = json!(now_ms());
    match outcome {
        Ok((status, account)) => {
            value["state"] = json!(status);
            if let Some(a) = account {
                value["account"] = a;
            }
        }
        Err(e) => {
            value["state"] = json!("failed");
            value["message"] = json!(e.1);
        }
    }
    let _ = publish(&state, &job, value.clone());
    let _ = state.event(
        "native.account",
        json!({"job_id":job.id,"state":value["state"]}),
    );
}
pub async fn start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Start>,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", true)?;
    native_config::installation(&state)?;
    if body.expected_backend_sha256 != scheduler::backend_hash(&state) {
        return Err(ApiError::conflict(
            json!({"reason":"configured_backend_changed"}),
        ));
    }
    if body.callback_port != 0 && body.callback_port < 1024 {
        return Err(ApiError::bad(
            "Use callback port 0 or a port from 1024 through 65535",
        ));
    }
    let mut guard = state.native_login.lock().unwrap();
    if let Some(job) = guard.as_ref() {
        let value = job.value.lock().unwrap();
        if in_progress(&value) {
            return Ok(Json(value.clone()));
        }
    }
    let (cancel, receiver) = watch::channel(false);
    let id = uuid::Uuid::new_v4().to_string();
    let value = json!({"id":id,"state":"starting","started_at":now_ms(),"provider_configuration_unchanged":true});
    let job = Arc::new(Login {
        id,
        value: Mutex::new(value.clone()),
        cancel,
    });
    publish(&state, &job, value.clone())?;
    *guard = Some(job.clone());
    drop(guard);
    tokio::spawn(run(state, job, body, receiver));
    Ok(Json(value))
}
pub async fn login_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", false)?;
    Ok(Json(current(&state)?))
}
pub async fn cancel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Cancel>,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", true)?;
    let guard = state.native_login.lock().unwrap();
    if let Some(job) = guard.as_ref() {
        if job.id != body.job_id {
            return Err(ApiError::conflict(json!({"reason":"sign_in_changed"})));
        }
        let mut value = job.value.lock().unwrap().clone();
        if in_progress(&value) {
            value["state"] = json!("cancelling");
            if let Some(o) = value.as_object_mut() {
                o.remove("authorization_url");
            }
            publish(&state, job, value.clone())?;
            let _ = job.cancel.send(true);
        }
        return Ok(Json(value));
    }
    drop(guard);
    Ok(Json(current(&state)?))
}
pub async fn status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", false)?;
    let (_, home) = native_config::installation(&state)?;
    let mut account = public_account(&native_config::invoke(&state, "chatgpt-status", None).await?);
    account["credentials_file"] = json!(home.join("accounts/chatgpt/credentials.json"));
    Ok(Json(json!({"account":account,"login":current(&state)?})))
}
pub async fn models(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", true)?;
    let value = native_config::invoke(&state, "chatgpt-models", None).await?;
    let schema = native_config::invoke(&state, "config-schema", None).await?;
    let catalog = schema["x-agent-runtime"].get("model_catalog");
    let supported = catalog.and_then(|value| value.get("openai"));
    if catalog.is_some() && supported.is_none_or(|value| !value.is_array()) {
        return Err(ApiError::bad(
            "The engine returned an unsupported model support catalog",
        ));
    }
    let models = value["models"]
        .as_array()
        .ok_or_else(|| ApiError::bad("The engine returned an unsupported model catalog"))?;
    Ok(Json(
        json!({"runtime_catalog_available":supported.is_some(),"models":models.iter().take(200).filter_map(|m|{let slug=m["slug"].as_str()?;let label=m["display_name"].as_str().unwrap_or(slug);let allowed=supported.is_none_or(|rows|rows.as_array().unwrap().iter().any(|row|row["model"]==slug));(allowed&&slug.len()<=256&&label.len()<=256).then(||json!({"slug":slug,"display_name":label}))}).collect::<Vec<_>>()}),
    ))
}
pub async fn disconnect(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Disconnect>,
) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", true)?;
    if !body.confirm || pending(&state) {
        return Err(ApiError::bad(
            "Finish or cancel sign-in, then explicitly confirm disconnection",
        ));
    }
    let result = public_account(&native_config::invoke(&state, "chatgpt-sign-out", None).await?);
    state.event("native.account", json!({"state":"disconnected"}))?;
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loopback_and_authorization_address_must_match() {
        let base = json!({"authorization_url":"https://auth.openai.com/api/accounts/authorize?redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fauth%2Fcallback","callback":"http://127.0.0.1:1455/auth/callback","expires_in_seconds":300});
        assert!(pending_details(&base, 1455).is_ok());
        assert!(pending_details(&base, 1555).is_err());
        for (key, value) in [
            ("callback", json!("http://192.168.1.2:1455/auth/callback")),
            ("authorization_url", json!("https://example.com/sign-in")),
            ("expires_in_seconds", json!(301)),
        ] {
            let mut candidate = base.clone();
            candidate[key] = value;
            assert!(pending_details(&candidate, 0).is_err());
        }
    }
    #[test]
    fn only_public_account_fields_leave_the_engine() {
        let result = public_account(
            &json!({"connected":true,"email":"fixture@example.invalid","access_token":"secret-canary","refresh_token":"private","credentials_file":"/untrusted/path","unexpected":{"secret":"hidden"}}),
        );
        assert!(result["connected"].as_bool().unwrap());
        assert!(!result.to_string().contains("canary"));
        assert!(result.get("credentials_file").is_none());
        assert!(result.get("unexpected").is_none());
    }
}
