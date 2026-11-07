//! A versioned default ACP runtime, and the account connection owned by one instance.
//! Custom ACP backends remain valid; this module never rewrites configuration itself.
use crate::config::{BackendConfig, InstanceConfig, private_dir, write_private};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Mutex, oneshot},
};

const LOCK: &str = include_str!("../scripts/backend-lock.json");
const INSTALLER: &str = include_str!("../scripts/provision-backend.py");
const BUNDLE: &str = "codex-0.165.0-acp-2.5.0-node-22.23.2";
const LOGIN_SECONDS: u64 = 15 * 60;

/// Downloads only the lock's checksummed packages. Caller applies the returned
/// backend through its normal revision/active-connection checks.
pub async fn install_codex(home: &Path, reuse_owner_account: bool) -> Result<BackendConfig> {
    ensure!(
        home.join("config.json").is_file(),
        "Initialize the instance first"
    );
    let mut command = Command::new(crate::platform::python());
    command
        .args(["-c", INSTALLER, "--home"])
        .arg(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if !reuse_owner_account {
        command.arg("--no-reuse-account");
    }
    let mut child = command
        .spawn()
        .context("Python 3 is required to install the default runtime")?;
    child
        .stdin
        .take()
        .context("Runtime installer input unavailable")?
        .write_all(LOCK.as_bytes())
        .await?;
    let output = tokio::time::timeout(Duration::from_secs(300), child.wait_with_output())
        .await
        .context("Runtime download timed out after five minutes; retry installation")??;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        bail!("{}", message.chars().take(2000).collect::<String>());
    }
    let result: Value =
        serde_json::from_slice(&output.stdout).context("Invalid runtime installation result")?;
    Ok(serde_json::from_value(result["backend"].clone())?)
}

/// Native Constellation is the default; no Codex account or runtime is involved.
pub async fn install_native(home: &Path, binary: Option<&Path>) -> Result<BackendConfig> {
    ensure!(
        home.join("config.json").is_file(),
        "Initialize the instance first"
    );
    let mut command = Command::new(crate::platform::python());
    command
        .args([
            "-c",
            include_str!("../scripts/provision-native.py"),
            "--home",
        ])
        .arg(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(binary) = binary {
        command.arg("--runtime-binary").arg(binary);
    }
    let mut child = command
        .spawn()
        .context("Python 3 is required to install Constellation")?;
    child
        .stdin
        .take()
        .context("Installer input unavailable")?
        .write_all(include_str!("../scripts/native-runtime-lock.json").as_bytes())
        .await?;
    let output = tokio::time::timeout(Duration::from_secs(300), child.wait_with_output())
        .await
        .context("Constellation download timed out; retry installation")??;
    if !output.status.success() {
        bail!(
            "{}",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(2000)
                .collect::<String>()
        );
    }
    let result: Value =
        serde_json::from_slice(&output.stdout).context("Invalid native installation result")?;
    Ok(serde_json::from_value(result["backend"].clone())?)
}

fn runtime(home: &Path) -> PathBuf {
    home.join("backend/runtime").join(BUNDLE)
}
fn configured_default(config: &InstanceConfig) -> bool {
    config.backend.as_ref().is_some_and(|b| {
        b.command
            == format!(
                "${{AGENT_HOME}}/backend/runtime/{BUNDLE}/{}",
                runtime_files()[0]
            )
    })
}
fn runtime_files() -> Vec<String> {
    vec![
        format!("node/bin/{}", crate::platform::executable("node")),
        format!("codex/bin/{}", crate::platform::executable("codex")),
        format!(
            "codex/bin/{}",
            crate::platform::executable("codex-code-mode-host")
        ),
        "acp/dist/index.js".into(),
        "runtime.json".into(),
    ]
}
fn installed(home: &Path) -> bool {
    runtime_files()
        .iter()
        .all(|name| runtime(home).join(name).is_file())
}
fn account_present(home: &Path) -> bool {
    let path = home.join("backend/provider/auth.json");
    let Ok(metadata) = std::fs::symlink_metadata(&path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return false;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    [
        value.get("OPENAI_API_KEY"),
        value.pointer("/tokens/access_token"),
    ]
    .into_iter()
    .flatten()
    .any(|v| v.as_str().is_some_and(|s| !s.trim().is_empty()))
}
fn source(home: &Path) -> &'static str {
    if !account_present(home) {
        return "none";
    }
    let v = std::fs::read(home.join("backend/provider/account-source.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    if v.as_ref().and_then(|v| v["source"].as_str()) == Some("owner_account") {
        "owner_account"
    } else {
        "instance_sign_in"
    }
}

/// Recognize the independently installed native engine without executing it or opening a key.
/// Its private state location is explicit in the standard backend command's arguments.
fn native_status(home: &Path, config: &InstanceConfig) -> Option<Value> {
    let backend = config.backend.as_ref()?;
    let name = Path::new(&backend.command).file_name()?.to_str()?;
    if !["agent-runtime", "agent-runtime.exe"].contains(&name) {
        return None;
    }
    let cwd = PathBuf::from(crate::config::expand(home, &backend.working_directory));
    let absolute = |raw: &str| {
        let p = PathBuf::from(crate::config::expand(home, raw));
        if p.is_absolute() { p } else { cwd.join(p) }
    };
    let command = absolute(&backend.command);
    let installed = command.is_file();
    let runtime_home = backend.args.iter().enumerate().find_map(|(i, arg)| {
        if arg == "--home" {
            backend.args.get(i + 1).map(|v| absolute(v))
        } else {
            arg.strip_prefix("--home=").map(absolute)
        }
    });
    let document = runtime_home.as_ref().and_then(|home| {
        let path = home.join("runtime.json");
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > 1024 * 1024 {
            return None;
        }
        serde_json::from_slice::<Value>(&std::fs::read(path).ok()?).ok()
    });
    let key_env = document
        .as_ref()
        .and_then(|v| v["provider"]["api_key_env"].as_str())
        .filter(|name| {
            !name.is_empty()
                && name.len() <= 128
                && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        });
    let secret_configured = key_env
        .and_then(|name| backend.secret_env.get(name))
        .filter(|name| crate::config::valid_name(name))
        .is_some_and(|name| {
            std::fs::symlink_metadata(home.join("secrets").join(name))
                .is_ok_and(|m| m.is_file() && m.len() > 0)
        });
    let chatgpt = document
        .as_ref()
        .is_some_and(|v| v["provider"]["auth"] == "chatgpt");
    let account_present = chatgpt
        && document
            .as_ref()
            .and_then(|v| v["provider"]["chatgpt_credentials_file"].as_str())
            .is_some_and(|p| {
                let path = Path::new(p);
                path.is_absolute()
                    && std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0)
            });
    let ready = if chatgpt {
        account_present
    } else {
        secret_configured
    };
    let status = if !installed {
        "missing_runtime"
    } else if document.is_none() {
        "needs_configuration"
    } else if chatgpt && !ready {
        "needs_account"
    } else if !ready {
        "needs_key"
    } else {
        "configured"
    };
    let problem = match status {
        "missing_runtime" => Some("The configured native runtime executable is not present."),
        "needs_configuration" => Some(
            "The native runtime needs an explicit --home argument and a readable runtime.json.",
        ),
        "needs_account" => {
            Some("Connect this native engine's ChatGPT account in Model & capabilities.")
        }
        "needs_key" => Some(
            "Store an API key and map the runtime's configured key environment variable to that named secret.",
        ),
        _ => None,
    };
    Some(
        json!({"kind":"native","configured":true,"installed":installed,"name":"Constellation","version":null,"acp_version":null,"node_version":null,"auth_ready":installed&&ready,"auth_mode":if chatgpt{"chatgpt"}else{"api_key"},"auth_validated":false,"credential_observation":"configured credential file metadata only; value not read","account_source":if account_present{"native_chatgpt"}else if secret_configured{"instance_secret"}else{"none"},"status":status,"problem":problem,"help":if chatgpt{"This native engine uses its own ChatGPT authorization. Model access requires an account model selection and a real task."}else if secret_configured{"An API key is configured for this instance. A real task must verify provider access."}else{"Configure a provider key or connect ChatGPT in Model & capabilities."},"actions":[],"isolation":"Native runtime state, credentials, workspace and history belong to this installation."}),
    )
}

/// Cheap local readiness, intentionally not a claim that a model call succeeded.
pub fn status(home: &Path, config: &InstanceConfig) -> Value {
    if let Some(status) = native_status(home, config) {
        return status;
    }
    let is_default = configured_default(config);
    let installed = installed(home);
    let auth_ready = is_default && installed && account_present(home);
    let kind = if is_default {
        "codex"
    } else if config.backend.is_some() {
        "custom"
    } else {
        "none"
    };
    let state = if kind == "custom" {
        "custom"
    } else if !installed || !is_default {
        "missing_runtime"
    } else if auth_ready {
        "ready"
    } else {
        "needs_account"
    };
    let problem = match state {
        "missing_runtime" => {
            Some("The default agent runtime has not been installed and configured.")
        }
        "needs_account" => Some("Connect a ChatGPT account to let this agent work."),
        _ => None,
    };
    let actions = match state {
        "missing_runtime" => vec!["install"],
        "needs_account" => vec!["connect_account"],
        "ready" => vec!["connect_account"],
        _ => vec![],
    };
    json!({"kind":kind,"configured":config.backend.is_some(),"installed":installed,
        "name":if kind=="custom" {"Custom ACP agent"} else {"Codex"},
        "version": if kind=="custom" {None} else {Some("0.165.0")},
        "acp_version":if kind=="custom" {None} else {Some("2.5.0")},
        "node_version":if kind=="custom" {None} else {Some("22.23.2")},
        "auth_ready":auth_ready,"auth_validated":false,"account_source":if is_default {source(home)} else {"none"},
        "status":state,"problem":problem,"help": if state=="needs_account" {Some("Sign in once for this agent. Device-code login must be enabled in your ChatGPT security settings or workspace permissions.")} else {None::<&str>},
        "actions":actions,
        "isolation":"Runtime files, provider account cache, working folders and history belong to this instance."})
}

struct AccountProcess {
    child: Child,
    input: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
}
impl AccountProcess {
    async fn open(home: &Path) -> Result<Self> {
        ensure!(
            installed(home),
            "Install the default runtime before connecting an account"
        );
        let backend = home.join("backend");
        let provider = backend.join("provider");
        private_dir(&provider)?;
        let mut command = Command::new(runtime(home).join(&runtime_files()[1]));
        command
            .args(["app-server", "-c", "cli_auth_credentials_store=\"file\""])
            .current_dir(home.join("workspace"))
            .env_clear()
            .env("HOME", &backend)
            .env("CODEX_HOME", &provider)
            .env("NO_BROWSER", "1")
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.env("USERPROFILE", &backend);
        for (key, folder) in [
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
        ] {
            let path = backend.join(folder);
            private_dir(&path)?;
            command.env(key, &path);
            #[cfg(windows)]
            if key == "XDG_CONFIG_HOME" {
                command.env("APPDATA", &path);
            }
            #[cfg(windows)]
            if key == "XDG_DATA_HOME" {
                command.env("LOCALAPPDATA", &path);
            }
        }
        for key in [
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "CODEX_CA_CERTIFICATE",
            "LANG",
            "SYSTEMROOT",
            "WINDIR",
            "TEMP",
            "TMP",
        ] {
            if let Ok(value) = std::env::var(key) {
                command.env(key, value);
            }
        }
        let mut child = command
            .spawn()
            .context("Could not start this agent's account connection")?;
        let input = child.stdin.take().context("Account input unavailable")?;
        let lines =
            BufReader::new(child.stdout.take().context("Account output unavailable")?).lines();
        let mut process = Self {
            child,
            input,
            lines,
        };
        process
            .request(
                1,
                "initialize",
                json!({"clientInfo":{"name":"agent-instance","version":env!("CARGO_PKG_VERSION")}}),
            )
            .await?;
        process
            .write(json!({"method":"initialized","params":{}}))
            .await?;
        Ok(process)
    }
    async fn write(&mut self, value: Value) -> Result<()> {
        let mut data = serde_json::to_vec(&value)?;
        data.push(b'\n');
        self.input.write_all(&data).await?;
        self.input.flush().await?;
        Ok(())
    }
    async fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value> {
        self.write(json!({"id":id,"method":method,"params":params}))
            .await?;
        tokio::time::timeout(Duration::from_secs(25), async {
            while let Some(line)=self.lines.next_line().await? {
                ensure!(line.len() < 2*1024*1024,"Account response exceeded its size limit");
                let value:Value=serde_json::from_str(&line).context("Invalid account response")?;
                if value["id"]==json!(id) {
                    // Provider failures may contain diagnostics. Never relay raw auth payloads.
                    ensure!(value.get("error").is_none(),"ChatGPT could not start this request. Check connectivity and enable device-code login in your ChatGPT security settings or workspace permissions, then retry.");
                    return Ok(value["result"].clone());
                }
            }
            bail!("Account connection closed before replying")
        }).await.context("Account request timed out; retry when this machine can reach ChatGPT")?
    }
}

/// Read the provider's account surface without returning account identifiers or tokens.
pub async fn account_read(home: &Path) -> Result<Value> {
    let mut process = AccountProcess::open(home).await?;
    let result = process
        .request(2, "account/read", json!({"refreshToken":false}))
        .await?;
    let account_type = result
        .pointer("/account/type")
        .and_then(Value::as_str)
        .map(str::to_string);
    let ready = account_type.is_some();
    let _ = process.child.kill().await;
    Ok(
        json!({"auth_ready":ready,"auth_validated":false,"account_type":account_type,"account_source":source(home)}),
    )
}

struct LoginAttempt {
    result: Arc<Mutex<Value>>,
    cancel: Option<oneshot::Sender<()>>,
}
type LoginMap = Mutex<HashMap<PathBuf, LoginAttempt>>;
fn logins() -> &'static LoginMap {
    static LOGINS: OnceLock<LoginMap> = OnceLock::new();
    LOGINS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn login_details(result: &Value) -> Result<Value> {
    let url = result["verificationUrl"]
        .as_str()
        .context("ChatGPT did not return a device sign-in address")?;
    let parsed = reqwest::Url::parse(url)?;
    ensure!(
        parsed.scheme() == "https"
            && parsed.host_str() == Some("auth.openai.com")
            && parsed.path() == "/codex/device"
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none()
            && parsed.port().is_none(),
        "ChatGPT returned an unexpected device sign-in address"
    );
    let code = result["userCode"]
        .as_str()
        .context("ChatGPT did not return a device code")?;
    ensure!(
        !code.is_empty()
            && code.len() <= 64
            && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "ChatGPT returned an unexpected device code"
    );
    let id = result["loginId"]
        .as_str()
        .context("ChatGPT did not return a login identifier")?;
    ensure!(
        !id.is_empty() && id.len() <= 128,
        "Unexpected login identifier"
    );
    Ok(
        json!({"state":"pending","login_id":id,"verification_url":url,"user_code":code,
        "expires_at":crate::state::now_ms()+LOGIN_SECONDS as i64*1000}),
    )
}

pub async fn login_start(home: &Path) -> Result<Value> {
    let home = dunce::canonicalize(home)?;
    let mut map = logins().lock().await;
    if let Some(attempt) = map.get(&home) {
        let value = attempt.result.lock().await.clone();
        if value["state"] == "pending" {
            return Ok(value);
        }
    }
    let mut process = tokio::time::timeout(Duration::from_secs(30), async {
        let mut process = AccountProcess::open(&home).await?;
        let result = process
            .request(
                2,
                "account/login/start",
                json!({"type":"chatgptDeviceCode"}),
            )
            .await?;
        let details = login_details(&result)?;
        Ok::<_, anyhow::Error>((process, details))
    })
    .await
    .context("Account connection timed out; retry")??;
    let details = process.1.clone();
    let result = Arc::new(Mutex::new(details.clone()));
    let pending = result.clone();
    let (cancel_tx, mut cancel_rx) = oneshot::channel();
    map.insert(
        home.clone(),
        LoginAttempt {
            result,
            cancel: Some(cancel_tx),
        },
    );
    tokio::spawn(async move {
        // Keep stdin alive throughout the ceremony. Capturing only stdout and
        // the child would otherwise close stdin when login_start returns and
        // cause app-server to exit before the browser login completes.
        let _stdin = process.0.input;
        let completion = tokio::select! {
            _=&mut cancel_rx => json!({"state":"cancelled"}),
            _=tokio::time::sleep(Duration::from_secs(LOGIN_SECONDS)) => json!({"state":"failed","message":"Sign-in expired. Connect your account again for a fresh code."}),
            result=async {
                while let Some(line)=process.0.lines.next_line().await? {
                    ensure!(line.len()<2*1024*1024,"Account message too large");
                    let value:Value=serde_json::from_str(&line)?;
                    if value["method"]=="account/login/completed" {
                        if value["params"]["success"]==true && account_present(&home) {
                            write_private(&home.join("backend/provider/account-source.json"),&serde_json::to_vec(&json!({"source":"instance_sign_in","at":crate::state::now_ms()}))?)?;
                            return Ok::<_,anyhow::Error>(json!({"state":"connected","auth_ready":true}));
                        }
                        return Ok(json!({"state":"failed","message":"ChatGPT sign-in did not complete. Check that device-code login is enabled for your account, then try again."}));
                    }
                }
                bail!("Account connection closed")
            } => result.unwrap_or_else(|_|json!({"state":"failed","message":"The account connection ended. Try connecting again."})),
        };
        let _ = process.0.child.kill().await;
        *pending.lock().await = completion;
    });
    Ok(details)
}

pub async fn login_status(home: &Path) -> Result<Value> {
    let home = dunce::canonicalize(home)?;
    let map = logins().lock().await;
    if let Some(attempt) = map.get(&home) {
        return Ok(attempt.result.lock().await.clone());
    }
    Ok(
        json!({"state":if account_present(&home){"connected"}else{"idle"},"auth_ready":account_present(&home)}),
    )
}
pub async fn login_cancel(home: &Path) -> Result<Value> {
    let home = dunce::canonicalize(home)?;
    let mut map = logins().lock().await;
    if let Some(mut attempt) = map.remove(&home) {
        if let Some(cancel) = attempt.cancel.take() {
            let _ = cancel.send(());
        }
        *attempt.result.lock().await = json!({"state":"cancelled"});
    }
    Ok(json!({"state":"cancelled"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    fn account_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for file in runtime_files() {
            let path = runtime(dir.path()).join(file);
            private_dir(path.parent().unwrap()).unwrap();
            write_private(&path, b"fixture").unwrap();
        }
        for path in ["workspace", "backend/provider"] {
            private_dir(&dir.path().join(path)).unwrap();
        }
        let codex = runtime(dir.path()).join("codex/bin/codex");
        write_private(
            &codex,
            include_bytes!("../tests/fixtures/account_backend.py"),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(codex, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        dir
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn account_login_is_idempotent_and_completion_drops_device_code() {
        let dir = account_fixture();
        let started = login_start(dir.path()).await.unwrap();
        assert_eq!(started["state"], "pending");
        assert_eq!(started["user_code"], "ABCD-1234");
        assert_eq!(login_start(dir.path()).await.unwrap(), started);
        write_private(
            &dir.path().join("backend/provider/fixture-complete"),
            b"yes",
        )
        .unwrap();
        let complete = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let state = login_status(dir.path()).await.unwrap();
                if state["state"] != "pending" {
                    break state;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(complete["state"], "connected");
        assert!(complete.get("user_code").is_none());
        assert!(!complete.to_string().contains("fixture-only-private-token"));
        assert_eq!(source(dir.path()), "instance_sign_in");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("backend/provider/fixture-starts")).unwrap(),
            "start\n"
        );
        login_cancel(dir.path()).await.unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn account_cancel_stops_its_process_and_keeps_provider_disconnected() {
        let dir = account_fixture();
        assert_eq!(account_read(dir.path()).await.unwrap()["auth_ready"], false);
        assert_eq!(login_start(dir.path()).await.unwrap()["state"], "pending");
        let pid: i32 = std::fs::read_to_string(dir.path().join("backend/provider/fixture-pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            login_cancel(dir.path()).await.unwrap()["state"],
            "cancelled"
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                // The PID belongs only to this fixture's process.
                if unsafe { libc::kill(pid, 0) } != 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(login_status(dir.path()).await.unwrap()["state"], "idle");
        assert!(!account_present(dir.path()));
    }
    #[test]
    fn rejects_untrusted_verification_links_and_codes() {
        for address in [
            "http://auth.openai.com/codex/device",
            "https://auth.openai.com.evil.test/codex/device",
            "https://auth.openai.com/codex/device?secret=anything",
            "https://evil.test/",
        ] {
            assert!(
                login_details(
                    &json!({"verificationUrl":address,"userCode":"ABCD-1234","loginId":"id"})
                )
                .is_err()
            );
        }
        assert!(login_details(&json!({"verificationUrl":"https://auth.openai.com/codex/device","userCode":"<script>","loginId":"id"})).is_err());
        let valid=login_details(&json!({"verificationUrl":"https://auth.openai.com/codex/device","userCode":"ABCD-1234","loginId":"id"})).unwrap();
        assert_eq!(valid["state"], "pending");
    }
    #[test]
    fn native_status_reads_explicit_home_and_secret_metadata_without_provider_claims() {
        let dir = tempfile::tempdir().unwrap();
        private_dir(&dir.path().join("engine/bin")).unwrap();
        private_dir(&dir.path().join("secrets")).unwrap();
        let mut config = InstanceConfig::blank("Native".into(), "owner".into());
        config.backend = Some(BackendConfig {
            command: "${AGENT_HOME}/engine/bin/agent-runtime".into(),
            args: vec!["--home".into(), "${AGENT_HOME}/engine".into(), "acp".into()],
            env: Default::default(),
            secret_env: std::collections::BTreeMap::from([(
                "AGENT_MODEL_API_KEY".into(),
                "native-model-key".into(),
            )]),
            working_directory: "${AGENT_HOME}/workspace".into(),
            allowed_working_roots: vec![],
        });
        assert_eq!(status(dir.path(), &config)["kind"], "native");
        assert_eq!(status(dir.path(), &config)["status"], "missing_runtime");
        write_private(
            &dir.path().join("engine/bin/agent-runtime"),
            b"fixture; never execute",
        )
        .unwrap();
        assert_eq!(status(dir.path(), &config)["status"], "needs_configuration");
        write_private(
            &dir.path().join("engine/runtime.json"),
            br#"{"provider":{"api_key_env":"AGENT_MODEL_API_KEY"}}"#,
        )
        .unwrap();
        assert_eq!(status(dir.path(), &config)["status"], "needs_key");
        write_private(
            &dir.path().join("secrets/native-model-key"),
            b"private-native-canary",
        )
        .unwrap();
        let ready = status(dir.path(), &config);
        assert_eq!(ready["status"], "configured");
        assert_eq!(ready["name"], "Constellation");
        assert_eq!(ready["auth_ready"], true);
        assert_eq!(ready["auth_validated"], false);
        assert_eq!(ready["actions"], json!([]));
        assert!(ready["version"].is_null());
        assert!(!ready.to_string().contains("private-native-canary"));
        config.backend.as_mut().unwrap().args =
            vec!["--home=${AGENT_HOME}/engine".into(), "acp".into()];
        assert_eq!(status(dir.path(), &config)["auth_ready"], true);
        // A differently named key is not inferred from another configured secret.
        write_private(
            &dir.path().join("engine/runtime.json"),
            br#"{"provider":{"api_key_env":"OTHER_KEY"}}"#,
        )
        .unwrap();
        assert_eq!(status(dir.path(), &config)["status"], "needs_key");
        config.backend.as_mut().unwrap().args = vec!["acp".into()];
        assert_eq!(status(dir.path(), &config)["status"], "needs_configuration");
        config.backend.as_mut().unwrap().command =
            "${AGENT_HOME}/engine/bin/pretend-agent-runtime".into();
        assert_eq!(status(dir.path(), &config)["kind"], "custom");
    }

    #[test]
    fn status_distinguishes_missing_custom_and_private_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = InstanceConfig::blank("test".into(), "owner".into());
        assert_eq!(status(dir.path(), &config)["status"], "missing_runtime");
        config.backend = Some(BackendConfig {
            command: "external-acp".into(),
            args: vec![],
            env: Default::default(),
            secret_env: Default::default(),
            working_directory: "/tmp".into(),
            allowed_working_roots: vec![],
        });
        assert_eq!(status(dir.path(), &config)["status"], "custom");
        config.backend.as_mut().unwrap().command = format!(
            "${{AGENT_HOME}}/backend/runtime/{BUNDLE}/{}",
            runtime_files()[0]
        );
        for file in runtime_files() {
            let path = runtime(dir.path()).join(file);
            private_dir(path.parent().unwrap()).unwrap();
            write_private(&path, b"test").unwrap();
        }
        assert_eq!(status(dir.path(), &config)["status"], "needs_account");
        private_dir(&dir.path().join("backend/provider")).unwrap();
        write_private(
            &dir.path().join("backend/provider/auth.json"),
            br#"{"tokens":{"access_token":"private-canary"}}"#,
        )
        .unwrap();
        let status = status(dir.path(), &config);
        assert_eq!(status["status"], "ready");
        assert_eq!(status["auth_validated"], false);
        assert!(!status.to_string().contains("private-canary"));
    }
}
