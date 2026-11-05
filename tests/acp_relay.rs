use agent_instance::{
    acp::relay,
    config::{BackendConfig, ConfigDocument, InstanceConfig, private_dir, write_private},
    state::AppState,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tempfile::TempDir;
use tokio::{sync::mpsc, task::JoinHandle};

struct Installation {
    _dir: TempDir,
    state: Arc<AppState>,
    work: PathBuf,
}
impl Installation {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for name in ["work", "profiles", "secrets", "fixture-state"] {
            private_dir(&dir.path().join(name)).unwrap();
        }
        write_private(
            &dir.path().join("secrets/provider-test"),
            b"fixture-secret-canary",
        )
        .unwrap();
        write_private(
            &dir.path().join("profiles/working-style.md"),
            b"Verify the result before declaring it complete.",
        )
        .unwrap();
        let mut config = InstanceConfig::blank("Fixture individual".into(), "test-owner".into());
        config.purpose = "Test interoperability".into();
        config.instructions = "Be precise".into();
        config.setup_complete = true;
        config.enabled = true;
        config.backend = Some(BackendConfig {
            command: agent_instance::platform::python().into(),
            args: vec![
                format!(
                    "{}/tests/fixtures/acp_backend.py",
                    env!("CARGO_MANIFEST_DIR")
                ),
                "${AGENT_HOME}/fixture-state".into(),
            ],
            env: BTreeMap::new(),
            secret_env: BTreeMap::from([("FIXTURE_SECRET".into(), "provider-test".into())]),
            working_directory: "${AGENT_HOME}/work".into(),
            allowed_working_roots: vec!["${AGENT_HOME}/work".into()],
        });
        write_private(
            &dir.path().join("config.json"),
            &serde_json::to_vec(&ConfigDocument {
                revision: 1,
                config,
            })
            .unwrap(),
        )
        .unwrap();
        let state = Arc::new(AppState::open(dir.path().to_owned()).unwrap());
        let work = dir.path().join("work");
        Self {
            _dir: dir,
            state,
            work,
        }
    }
    fn connect(&self) -> Client {
        let (tx, incoming) = mpsc::channel(32);
        let (outgoing, rx) = mpsc::channel(32);
        let state = self.state.clone();
        let task =
            tokio::spawn(
                async move { relay(state, "test-principal".into(), incoming, outgoing).await },
            );
        Client { tx, rx, task }
    }
}

struct Client {
    tx: mpsc::Sender<Value>,
    rx: mpsc::Receiver<Value>,
    task: JoinHandle<anyhow::Result<()>>,
}

#[tokio::test]
async fn early_launch_failures_have_actionable_secret_free_observations() {
    for (failure, expected_stage) in [
        ("directory", "working_directory"),
        ("secret", "backend_secrets"),
        ("command", "backend_process"),
    ] {
        let install = Installation::new();
        {
            let mut document = install.state.config.write().unwrap();
            let backend = document.config.backend.as_mut().unwrap();
            match failure {
                "directory" => backend.working_directory = "/absent-private-canary/work".into(),
                "secret" => {
                    backend
                        .secret_env
                        .insert("TEST_SECRET".into(), "absent-private-canary".into());
                }
                "command" => {
                    backend.command = "/absent-private-canary/program".into();
                    backend.args = vec!["argument-private-canary".into()];
                }
                _ => unreachable!(),
            }
        }
        let client = install.connect();
        client.tx.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}})).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(4), client.task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        let data: String = install
            .state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT data FROM events WHERE kind='acp.launch_failed' ORDER BY seq DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let event: Value = serde_json::from_str(&data).unwrap();
        assert_eq!(event["stage"], expected_stage);
        assert!(event["action"].as_str().unwrap().len() > 20);
        let all_events: String = install
            .state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT group_concat(data) FROM events", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(!all_events.contains("private-canary"));
        assert!(!all_events.contains("fixture-secret-canary"));
        assert!(install.state.connections.lock().unwrap().is_empty());
    }
}
impl Client {
    async fn send(&self, id: u32, method: &str, params: Value) {
        self.tx
            .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await
            .unwrap();
    }
    async fn recv(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(8), self.rx.recv())
            .await
            .expect("ACP frame timeout")
            .expect("ACP closed early")
    }
    async fn response(&mut self, id: u32) -> (Value, Vec<Value>) {
        let mut notifications = vec![];
        loop {
            let msg = self.recv().await;
            if msg.get("method").is_none() && msg["id"] == id {
                return (msg, notifications);
            }
            notifications.push(msg);
        }
    }
    async fn initialize(&mut self) -> Value {
        self.send(100, "initialize", json!({"protocolVersion":1,"clientInfo":{"name":"independent-test-client","version":"1"},"clientCapabilities":{"fs":{"readTextFile":true}}})).await;
        self.response(100).await.0
    }
    async fn new_session(&mut self, work: &std::path::Path) -> String {
        self.send(101, "session/new", json!({"cwd":work,"mcpServers":[{"name":"example-tools","command":"/fixture/mcp","args":[],"env":[{"name":"SECRET","value":"mcp-secret-canary"}]}]})).await;
        self.response(101).await.0["result"]["sessionId"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    async fn disconnect(self) {
        drop(self.tx);
        let result = tokio::time::timeout(Duration::from_secs(4), self.task)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_ok(), "{result:?}");
    }
}

fn detail(updates: &[Value]) -> Value {
    let text = updates
        .iter()
        .rev()
        .find_map(|v| v["params"]["update"]["content"]["text"].as_str())
        .unwrap();
    serde_json::from_str(text).unwrap()
}

#[tokio::test]
async fn initialization_discovers_optional_management_without_changing_acp_capabilities() {
    for (public_url, addresses, expected_descriptor) in [
        (
            Some("https://agents.example/instance/"),
            json!({"urls":["https://192.168.1.20:4567"],"base_url":"https://127.0.0.1:4567"}),
            Some("https://agents.example/instance/api/v1/descriptor"),
        ),
        (
            None,
            json!({"urls":["https://localhost:4567","https://127.0.0.1:4567","https://[::1]:4567","https://192.168.1.20:4567"],"base_url":"https://127.0.0.1:4567"}),
            Some("https://192.168.1.20:4567/api/v1/descriptor"),
        ),
        (
            None,
            json!({"urls":["https://127.0.0.1:4567"],"base_url":"https://127.0.0.1:4567"}),
            Some("https://127.0.0.1:4567/api/v1/descriptor"),
        ),
        (None, json!({}), None),
    ] {
        let install = Installation::new();
        install
            .state
            .config
            .write()
            .unwrap()
            .config
            .server
            .public_url = public_url.map(str::to_owned);
        *install.state.addresses.write().unwrap() = addresses;
        let mut client = install.connect();
        let response = client.initialize().await;
        let management =
            &response["result"]["_meta"]["io.github.bkrabach.agent-instance"]["management"];
        if let Some(descriptor) = expected_descriptor {
            assert_eq!(
                management,
                &json!({
                    "profile":"agent-instance.management/1",
                    "descriptorUrl":descriptor,
                    "authentication":{"required":true,"separateFromAcp":true}
                })
            );
        } else {
            assert!(management.is_null());
        }
        assert_eq!(
            response["result"]["agentCapabilities"],
            json!({
                "loadSession":true,"sessionCapabilities":{"list":{},"resume":{},"close":{}}
            })
        );
        assert_eq!(response["result"]["_meta"]["fixture-preserved"], true);
        client.disconnect().await;
    }
}

#[tokio::test]
async fn work_observations_follow_prompt_results_cancellation_and_disconnect_without_content() {
    let install = Installation::new();
    let mut client = install.connect();
    client.initialize().await;
    let sid = client.new_session(&install.work).await;
    client
        .send(
            170,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"private-work-canary"}]}),
        )
        .await;
    assert_eq!(
        client.response(170).await.0["result"]["stopReason"],
        "end_turn"
    );
    client.send(171, "session/prompt", json!({"sessionId":"unbound-session","prompt":[{"type":"text","text":"private-error-canary"}]})).await;
    assert!(client.response(171).await.0["error"].is_object());
    client.send(172, "session/prompt", json!({"sessionId":sid,"prompt":[{"type":"text","text":"WAIT_FOR_CANCEL private-cancel-canary"}]})).await;
    assert_eq!(client.recv().await["method"], "session/update");
    client
        .tx
        .send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":sid}}))
        .await
        .unwrap();
    assert_eq!(
        client.response(172).await.0["result"]["stopReason"],
        "cancelled"
    );
    client.send(173, "session/prompt", json!({"sessionId":sid,"prompt":[{"type":"text","text":"WAIT_FOR_CANCEL private-disconnect-canary"}]})).await;
    assert_eq!(client.recv().await["method"], "session/update");
    client.disconnect().await;

    let db = install.state.db.lock().unwrap();
    let rows = db
        .prepare("SELECT kind,data FROM events WHERE kind LIKE 'acp.prompt.%' ORDER BY seq")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        rows.len(),
        8,
        "Four starts, each with exactly one terminal outcome"
    );
    let mut active = std::collections::BTreeSet::new();
    for (kind, encoded) in &rows {
        let data: Value = serde_json::from_str(encoded).unwrap();
        let work_id = data["work_id"].as_str().unwrap().to_owned();
        assert!(!encoded.contains("canary"));
        assert!(!encoded.contains("Configured agent instance context"));
        if kind == "acp.prompt.started" {
            assert!(active.insert(work_id));
        } else {
            assert!(active.remove(&work_id));
            assert!(data["duration_ms"].as_i64().unwrap() >= 0);
        }
    }
    assert!(active.is_empty());
    assert_eq!(
        rows.iter()
            .filter(|(kind, _)| kind == "acp.prompt.finished")
            .count(),
        1
    );
    assert_eq!(
        rows.iter()
            .filter(|(kind, _)| kind == "acp.prompt.cancelled")
            .count(),
        1
    );
    assert_eq!(
        rows.iter()
            .filter(|(kind, _)| kind == "acp.prompt.failed")
            .count(),
        2
    );
}

#[tokio::test]
async fn observations_distinguish_fixed_backend_revision_from_current_profile_context() {
    let install = Installation::new();
    let mut client = install.connect();
    client.initialize().await;
    let sid = client.new_session(&install.work).await;
    client
        .send(
            160,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"First profile"}]}),
        )
        .await;
    client.response(160).await;
    let before = install.state.get_record("sessions", &sid).unwrap().unwrap();
    assert_eq!(before["last_prompt_config_revision"], 1);
    assert_eq!(
        before["last_profile_sha256"],
        format!(
            "{:x}",
            Sha256::digest(install.state.profile_text().unwrap().as_bytes())
        )
    );

    {
        let mut document = install.state.config.write().unwrap();
        document.revision = 2;
        document.config.purpose = "Updated purpose for the next prompt".into();
        let backend = document.config.backend.as_mut().unwrap();
        backend.secret_env.clear();
        backend
            .env
            .insert("FIXTURE_SECRET".into(), "new-backend-setting".into());
    }
    agent_instance::profiles::write(
        &install.state,
        "working-style",
        "A newly edited working style.",
    )
    .unwrap();
    client
        .send(
            161,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"Updated profile"}]}),
        )
        .await;
    let (_, updates) = client.response(161).await;
    let response = detail(&updates);
    assert!(
        response["echo"]
            .as_str()
            .unwrap()
            .contains("Updated purpose for the next prompt")
    );
    assert!(
        response["echo"]
            .as_str()
            .unwrap()
            .contains("A newly edited working style.")
    );
    assert_eq!(
        response["secret_present"], true,
        "Existing backend retains the original environment"
    );

    let connection = install
        .state
        .connections
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    assert_eq!(connection["config_revision"], 1);
    let after = install.state.get_record("sessions", &sid).unwrap().unwrap();
    assert_eq!(after["last_prompt_config_revision"], 2);
    assert_eq!(before["last_profile_package"]["revision"], 0);
    assert_eq!(after["last_profile_package"]["revision"], 1);
    assert_eq!(
        after["last_profile_package"]["id"],
        agent_instance::profiles::status(&install.state).unwrap()["active"]["id"]
    );
    assert_eq!(
        after["last_profile_sha256"],
        format!(
            "{:x}",
            Sha256::digest(install.state.profile_text().unwrap().as_bytes())
        )
    );
    assert_ne!(after["last_profile_sha256"], before["last_profile_sha256"]);
    assert!(
        after["last_profile_supplied_at"].as_i64().unwrap()
            >= before["last_profile_supplied_at"].as_i64().unwrap()
    );
    let delivery =
        agent_instance::profile_use::inspect(&install.state, &Default::default()).unwrap();
    assert_eq!(delivery["status"], "supplied_current");
    assert_eq!(delivery["relevant_turn"]["session_id"], sid);
    assert_eq!(
        delivery["relevant_turn"]["profile_sha256"],
        after["last_profile_sha256"]
    );
    assert_eq!(
        delivery["relevant_turn"]["profile_package"],
        after["last_profile_package"]
    );
    assert_eq!(delivery["behavior_quality"], "not_evaluated");

    client.disconnect().await;
}

#[tokio::test]
async fn preserves_acp_capabilities_content_mcp_auth_and_configuration() {
    let install = Installation::new();
    let mut client = install.connect();
    let initialize = client.initialize().await;
    assert_eq!(
        initialize["result"]["agentInfo"]["title"],
        "Fixture individual"
    );
    assert_eq!(
        initialize["result"]["agentCapabilities"]["loadSession"],
        true
    );
    assert!(initialize["result"]["agentCapabilities"]["promptCapabilities"].is_null());
    assert_eq!(initialize["result"]["_meta"]["fixture-preserved"], true);
    assert_eq!(
        initialize["result"]["_meta"]["io.github.bkrabach.agent-instance"]["backendAgentInfo"]["name"],
        "independent-python-fixture"
    );
    client
        .send(102, "authenticate", json!({"methodId":"fixture-login"}))
        .await;
    assert_eq!(client.response(102).await.0["result"], json!({}));
    let sid = client.new_session(&install.work).await;
    client.send(103,"session/prompt",json!({"sessionId":sid,"prompt":[{"type":"text","text":"Hello"},{"type":"resource_link","uri":"file:///fixture/context","name":"Context"}]})).await;
    let (response, updates) = client.response(103).await;
    assert_eq!(response["result"]["stopReason"], "end_turn");
    let detail = detail(&updates);
    assert_eq!(detail["types"], json!(["text", "text", "resource_link"]));
    assert_eq!(detail["configured_context"], true);
    assert_eq!(
        detail["block_meta"][0]["com.microsoft.amplifier.context-snapshot"]["version"],
        1
    );
    assert!(
        detail["echo"]
            .as_str()
            .unwrap()
            .contains("Verify the result")
    );
    assert_eq!(detail["mcp_servers"], json!(["example-tools"]));
    assert_eq!(detail["secret_present"], true);
    assert_eq!(detail["slack_env_present"], false);
    assert_eq!(
        detail["backend_home"],
        install
            .state
            .home
            .join("backend")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(
        detail["xdg_config_home"],
        install
            .state
            .home
            .join("backend")
            .join("config")
            .to_string_lossy()
            .as_ref()
    );
    client
        .send(
            104,
            "session/set_config_option",
            json!({"sessionId":sid,"configId":"model","value":"fixture"}),
        )
        .await;
    assert_eq!(
        client.response(104).await.0["result"]["configOptions"][0]["currentValue"],
        "fixture"
    );
    client.send(105, "_unknown/extension", json!({})).await;
    assert_eq!(client.response(105).await.0["error"]["code"], -32601);
    client.disconnect().await;
    assert!(install.state.connections.lock().unwrap().is_empty());
    assert_eq!(install.state.list_records("sessions").unwrap().len(), 1);
    let events: String = install
        .state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT group_concat(data) FROM events", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(
        !events.contains("fixture-secret-canary")
            && !events.contains("mcp-secret-canary")
            && !events.contains("Verify the result")
    );
}

#[tokio::test]
async fn forwards_bidirectional_permission_and_filesystem_callbacks() {
    let install = Installation::new();
    let mut client = install.connect();
    client.initialize().await;
    let sid = client.new_session(&install.work).await;
    // Same ID in both directions is legal and must not collide.
    client.send(1,"session/prompt",json!({"sessionId":sid,"prompt":[{"type":"text","text":"REQUEST_PERMISSION REQUEST_FILE"}]})).await;
    let permission = client.recv().await;
    assert_eq!(permission["method"], "session/request_permission");
    assert_eq!(permission["id"], 1);
    client.tx.send(json!({"jsonrpc":"2.0","id":1,"result":{"outcome":{"outcome":"selected","optionId":"permit"}}})).await.unwrap();
    let file = client.recv().await;
    assert_eq!(file["method"], "fs/read_text_file");
    client.tx.send(json!({"jsonrpc":"2.0","id":"file-callback","result":{"content":"callback-secret-canary"}})).await.unwrap();
    let (_, updates) = client.response(1).await;
    let detail = detail(&updates);
    assert_eq!(detail["permission"]["outcome"]["optionId"], "permit");
    assert_eq!(detail["file_result"]["content"], "callback-secret-canary");
    client.disconnect().await;
    let events: String = install
        .state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT group_concat(data) FROM events", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(!events.contains("callback-secret-canary"));
}

#[tokio::test]
async fn persisted_backend_sessions_replay_on_a_new_connection() {
    let install = Installation::new();
    let mut first = install.connect();
    first.initialize().await;
    let sid = first.new_session(&install.work).await;
    first.send(110,"session/prompt",json!({"sessionId":sid,"prompt":[{"type":"text","text":"Remember this marker: violet-cabin"}]})).await;
    first.response(110).await;
    first.disconnect().await;
    let mut second = install.connect();
    second.initialize().await;
    second
        .send(
            111,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"Not loaded yet"}]}),
        )
        .await;
    assert_eq!(second.response(111).await.0["error"]["code"], -32002);
    second.send(112, "session/list", json!({})).await;
    assert_eq!(
        second.response(112).await.0["result"]["sessions"][0]["sessionId"],
        sid
    );
    second
        .send(
            113,
            "session/load",
            json!({"sessionId":sid,"cwd":install.work,"mcpServers":[]}),
        )
        .await;
    let (response, replay) = second.response(113).await;
    assert_eq!(response["result"], json!({}));
    assert_eq!(replay.len(), 2);
    assert_eq!(
        replay[0]["params"]["update"]["content"]["text"]
            .as_str()
            .unwrap()
            .trim(),
        "Remember this marker: violet-cabin"
    );
    second.disconnect().await;
}

#[tokio::test]
async fn cancellation_and_concurrent_connections_stay_separate() {
    let install = Installation::new();
    let mut first = install.connect();
    let mut second = install.connect();
    first.initialize().await;
    second.initialize().await;
    let a = first.new_session(&install.work).await;
    let b = second.new_session(&install.work).await;
    first
        .send(
            120,
            "session/prompt",
            json!({"sessionId":a,"prompt":[{"type":"text","text":"WAIT_FOR_CANCEL"}]}),
        )
        .await;
    assert_eq!(first.recv().await["method"], "session/update");
    second
        .send(
            120,
            "session/prompt",
            json!({"sessionId":b,"prompt":[{"type":"text","text":"Second independent context"}]}),
        )
        .await;
    assert_eq!(
        second.response(120).await.0["result"]["stopReason"],
        "end_turn"
    );
    first
        .tx
        .send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":a}}))
        .await
        .unwrap();
    assert_eq!(
        first.response(120).await.0["result"]["stopReason"],
        "cancelled"
    );
    first.disconnect().await;
    assert_eq!(install.state.connections.lock().unwrap().len(), 1);
    second.disconnect().await;
}

#[tokio::test]
async fn refuses_cwd_outside_roots_and_symlink_escape_without_substitution() {
    let install = Installation::new();
    let mut client = install.connect();
    client
        .send(
            129,
            "session/new",
            json!({"cwd":"relative","mcpServers":[]}),
        )
        .await;
    assert_eq!(client.response(129).await.0["error"]["code"], -32600);
    client.initialize().await;
    for cwd in [
        "relative".to_owned(),
        install.state.home.to_string_lossy().into_owned(),
    ] {
        client
            .send(130, "session/new", json!({"cwd":cwd,"mcpServers":[]}))
            .await;
        assert_eq!(client.response(130).await.0["error"]["code"], -32602);
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&install.state.home, install.work.join("escape")).unwrap();
        client
            .send(
                130,
                "session/new",
                json!({"cwd":install.work.join("escape"),"mcpServers":[]}),
            )
            .await;
        assert_eq!(client.response(130).await.0["error"]["code"], -32602);
    }
    let sid = client.new_session(&install.work).await;
    assert_eq!(
        install.state.get_record("sessions", &sid).unwrap().unwrap()["cwd"],
        install.work.to_string_lossy().as_ref()
    );
    client.disconnect().await;
}

#[tokio::test]
async fn oversized_backend_output_closes_connection_without_unbounded_buffering() {
    let install = Installation::new();
    install
        .state
        .config
        .write()
        .unwrap()
        .config
        .limits
        .max_frame_bytes = 4096;
    let mut client = install.connect();
    client.initialize().await;
    let sid = client.new_session(&install.work).await;
    client
        .send(
            150,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"OVERSIZE_BACKEND_FRAME"}]}),
        )
        .await;
    assert_eq!(client.response(150).await.0["error"]["code"], -32603);
    assert!(client.task.await.unwrap().is_err());
    assert!(install.state.connections.lock().unwrap().is_empty());
}

#[tokio::test]
async fn runtime_stop_kills_backend_and_its_descendant() {
    let install = Installation::new();
    let mut client = install.connect();
    client.initialize().await;
    let sid = client.new_session(&install.work).await;
    client
        .send(
            140,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"SPAWN_CHILD"}]}),
        )
        .await;
    assert_eq!(client.recv().await["method"], "session/update");
    let descendant: u32 =
        std::fs::read_to_string(install.state.home.join("fixture-state/descendant.pid"))
            .unwrap()
            .parse()
            .unwrap();
    install.state.runtime_stop.send(()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(4), client.task)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err());
    let category: String = install.state.db.lock().unwrap().query_row(
        "SELECT activity_category FROM events WHERE kind='acp.closed_with_error' ORDER BY seq DESC LIMIT 1",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(
        category, "runtime",
        "An intentional runtime stop is not an unexpected connection failure"
    );
    for _ in 0..20 {
        #[cfg(unix)]
        let stopped = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &descendant.to_string()])
            .output()
            .map(|o| {
                !o.status.success() || String::from_utf8_lossy(&o.stdout).trim().starts_with('Z')
            })
            .unwrap_or(false);
        #[cfg(windows)]
        let stopped = unsafe {
            use windows_sys::Win32::{
                Foundation::CloseHandle,
                System::Threading::{
                    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
                },
            };
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, descendant);
            if handle.is_null() {
                true
            } else {
                let mut code = 259;
                let ok = GetExitCodeProcess(handle, &mut code);
                CloseHandle(handle);
                ok != 0 && code != 259
            }
        };
        if stopped {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("Backend descendant survived runtime stop");
}

#[tokio::test]
async fn optional_attention_is_intercepted_but_plain_acp_and_streaming_still_work() {
    use agent_instance::{attention, state::now_ms};
    let i = Installation::new();
    // Offline provider exercises the separately-labeled, bounded engage preference.
    i.state.config.write().unwrap().config.attention = Some(attention::Config {
        endpoint: "http://127.0.0.1:9/".into(),
        model: "fixture".into(),
        protocol: "system_one".into(),
        auth: "bearer".into(),
        credential_secret: "provider-test".into(),
        decisions_per_hour: 20,
        turns_per_hour: 5,
        uncertain_turns_per_hour: 1,
    });
    let mut c = i.connect();
    let init = c.initialize().await;
    assert_eq!(init["result"]["_meta"][attention::META]["enabled"], true);
    let sid = c.new_session(&i.work).await;
    let event = json!({"sessionId":sid,"occurredAt":now_ms(),"context":{"conversation_id":"room","agent":{"id":"a","name":"Agent"},"participants":[{"id":"a","name":"Agent"},{"id":"b","name":"Human"}],"messages":[{"id":"event","sender":"b","text":"What are you working on?"}],"trigger_id":"event"}});
    c.send(200, attention::METHOD, event.clone()).await;
    let offer = c.response(200).await.0["result"].clone();
    assert_eq!(offer["action"], "engage");
    c.send(201,"session/prompt",json!({"sessionId":sid,"prompt":[{"type":"text","text":"One admitted response"}],"_meta":{attention::META:{"id":offer["id"],"ticket":offer["ticket"]}}})).await;
    let (r, updates) = c.response(201).await;
    assert_eq!(r["result"]["stopReason"], "end_turn");
    assert!(
        detail(&updates)["echo"]
            .as_str()
            .unwrap()
            .contains("One admitted response")
    );
    c.send(202, attention::METHOD, event).await;
    assert_eq!(c.response(202).await.0["result"]["reason"], "duplicate");
    c.send(203,"session/prompt",json!({"sessionId":sid,"prompt":[{"type":"text","text":"Direct ordinary ACP still works"}]})).await;
    assert_eq!(c.response(203).await.0["result"]["stopReason"], "end_turn");
    assert_eq!(
        attention::report(&i.state).unwrap()["counts"]["status:admitted"],
        1
    );
    c.disconnect().await;
}

fn steering(sid: &str, input_id: &str) -> Value {
    json!({"sessionId":sid,"prompt":[{"type":"text","text":"New participant input private-steering-canary","_meta":{"com.microsoft.amplifier.context-snapshot":{"version":1,"key":"source:room:revision-2","retainedText":"Earlier source"},"source":{"conversation":"room","revision":2,"trigger":"message-2"}}}],"_meta":{"steering":{"inputId":input_id,"idleBehavior":"promptRequired"},"client-preserved":true}})
}

fn awareness(sid: &str, prompt_id: Value) -> Value {
    let mut input = steering(sid, "peer-awareness");
    input["_meta"]["steering"]["intent"] = json!("awareness");
    input["_meta"]["steering"]["activePromptId"] = prompt_id;
    input
}

#[tokio::test]
async fn awareness_is_bound_to_the_active_prompt_and_preserves_approval() {
    let install = Installation::new();
    let mut client = install.connect();
    let initialized = client.initialize().await;
    assert_eq!(
        initialized["result"]["_meta"]["io.github.bkrabach.agent-instance"]["inputContinuity"]["awareness"],
        json!({"version":1,"activeOnly":true,"promptIdRequired":true})
    );
    let sid = client.new_session(&install.work).await;
    client
        .send(
            200,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"REQUEST_PERMISSION"}]}),
        )
        .await;
    let permission = client.recv().await;
    assert_eq!(permission["method"], "session/request_permission");

    // Even with a turn running, stale/wrong prompt references are not forwarded.
    client
        .send(201, STEERING_METHOD, awareness(&sid, json!(199)))
        .await;
    let rejected = client.response(201).await.0;
    assert_eq!(rejected["result"]["outcome"], "promptRequired");
    assert!(rejected["result"]["fixtureParams"].is_null());

    client
        .send(202, STEERING_METHOD, awareness(&sid, json!(200)))
        .await;
    let accepted = client.response(202).await.0;
    assert_eq!(accepted["result"]["outcome"], "injected");
    assert_eq!(
        accepted["result"]["fixtureParams"]["_meta"]["steering"]["intent"],
        "awareness"
    );
    client.tx.send(json!({"jsonrpc":"2.0","id":permission["id"],"result":{"outcome":{"outcome":"selected","optionId":"permit"}}})).await.unwrap();
    let (done, updates) = client.response(200).await;
    assert_eq!(done["result"]["stopReason"], "end_turn");
    assert_eq!(
        detail(&updates)["permission"]["outcome"]["optionId"],
        "permit"
    );

    client
        .send(203, STEERING_METHOD, awareness(&sid, json!(200)))
        .await;
    let idle = client.response(203).await.0;
    assert_eq!(idle["result"]["outcome"], "promptRequired");
    assert!(idle["result"]["fixtureParams"].is_null());
    let starts: i64 = install
        .state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM events WHERE kind='acp.prompt.started'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(starts, 1);
    client.disconnect().await;
}

const STEERING_METHOD: &str = "_session/steering";

#[tokio::test]
async fn awareness_cannot_start_work_or_carry_a_new_social_offer() {
    let install = Installation::new();
    let mut client = install.connect();
    client.initialize().await;
    let sid = client.new_session(&install.work).await;
    client
        .send(200, "session/prompt", awareness(&sid, json!(199)))
        .await;
    assert_eq!(client.response(200).await.0["error"]["code"], -32602);
    let mut input = awareness(&sid, Value::Null);
    client.send(201, STEERING_METHOD, input.clone()).await;
    assert_eq!(client.response(201).await.0["error"]["code"], -32602);
    input["_meta"]["steering"]["activePromptId"] = json!(199);
    input["_meta"]["com.microsoft.amplifier.attention"] =
        json!({"id":"unconsumed-offer","ticket":"private-ticket-canary"});
    client.send(202, STEERING_METHOD, input).await;
    assert_eq!(client.response(202).await.0["error"]["code"], -32602);
    let starts: i64 = install
        .state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM events WHERE kind='acp.prompt.started'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(starts, 0);
    client.disconnect().await;
}

#[tokio::test]
async fn awareness_rejects_malformed_input_and_cancelled_string_prompt() {
    let install = Installation::new();
    let mut client = install.connect();
    client.initialize().await;
    let sid = client.new_session(&install.work).await;
    let mut malformed = awareness(&sid, json!("active-prompt"));
    malformed["prompt"] = json!([]);
    client.send(200, STEERING_METHOD, malformed).await;
    assert_eq!(client.response(200).await.0["error"]["code"], -32602);
    client.tx.send(json!({"jsonrpc":"2.0","id":"active-prompt","method":"session/prompt","params":{"sessionId":sid,"prompt":[{"type":"text","text":"REQUEST_PERMISSION"}]}})).await.unwrap();
    let permission = client.recv().await;
    client
        .send(
            201,
            STEERING_METHOD,
            awareness(&sid, json!("active-prompt")),
        )
        .await;
    assert_eq!(
        client.response(201).await.0["result"]["outcome"],
        "injected"
    );
    client
        .tx
        .send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":sid}}))
        .await
        .unwrap();
    // The fixture deliberately retains the pending callback. Cancel closes the
    // awareness window before the backend's final response can arrive.
    client
        .send(
            202,
            STEERING_METHOD,
            awareness(&sid, json!("active-prompt")),
        )
        .await;
    let rejected = client.response(202).await.0;
    assert_eq!(rejected["result"]["outcome"], "promptRequired");
    assert!(rejected["result"]["fixtureParams"].is_null());
    client.tx.send(json!({"jsonrpc":"2.0","id":permission["id"],"result":{"outcome":{"outcome":"selected","optionId":"reject"}}})).await.unwrap();
    loop {
        let value = client.recv().await;
        if value["id"] == "active-prompt" {
            break;
        }
    }
    client.disconnect().await;
}

#[tokio::test]
async fn awareness_cannot_target_a_different_loaded_session() {
    let install = Installation::new();
    let mut client = install.connect();
    client.initialize().await;
    let sid = client.new_session(&install.work).await;
    let other = client.new_session(&install.work).await;
    client
        .send(
            200,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"REQUEST_PERMISSION"}]}),
        )
        .await;
    let permission = client.recv().await;
    client
        .send(201, STEERING_METHOD, awareness(&other, json!(200)))
        .await;
    let response = client.response(201).await.0;
    assert_eq!(response["result"]["outcome"], "promptRequired");
    assert!(response["result"]["fixtureParams"].is_null());
    client.tx.send(json!({"jsonrpc":"2.0","id":permission["id"],"result":{"outcome":{"outcome":"selected","optionId":"permit"}}})).await.unwrap();
    assert_eq!(
        client.response(200).await.0["result"]["stopReason"],
        "end_turn"
    );
    client.disconnect().await;
}

#[tokio::test]
async fn steering_preserves_approval_context_receipts_and_one_work_lifecycle() {
    let i = Installation::new();
    let mut c = i.connect();
    let init = c.initialize().await;
    assert_eq!(
        init["result"]["_meta"]["steering"],
        json!({"supported":true,"version":1,"idleBehavior":["promptRequired"]})
    );
    let sid = c.new_session(&i.work).await;
    c.send(
        200,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"REQUEST_PERMISSION"}]}),
    )
    .await;
    let callback = c.recv().await;
    assert_eq!(callback["method"], "session/request_permission");
    let params = steering(&sid, "durable-input-1");
    c.send(201, "_session/steering", params.clone()).await;
    let (r, updates) = c.response(201).await;
    assert_eq!(r["result"]["outcome"], "injected", "{r}");
    let forwarded = &r["result"]["fixtureParams"];
    assert_eq!(forwarded["_meta"], params["_meta"]);
    assert_eq!(forwarded["prompt"][1], params["prompt"][0]);
    assert!(
        forwarded["prompt"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Name: Fixture individual")
    );
    assert_eq!(
        forwarded["prompt"][0]["_meta"]["com.microsoft.amplifier.context-snapshot"]["version"],
        1
    );
    let receipt =
        json!({"inputId":"durable-input-1","state":"incorporated","turnId":"fixture-turn-200"});
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["params"]["sessionId"], sid);
    assert_eq!(updates[0]["params"]["update"]["content"]["text"], "");
    assert_eq!(
        updates[0]["params"]["update"]["_meta"]["io.github.bkrabach.agent-runtime"]["inputReceipt"],
        receipt
    );
    assert_eq!(
        agent_instance::updates::status(&i.state).unwrap()["active_prompts"],
        1
    );
    c.tx.send(json!({"jsonrpc":"2.0","id":callback["id"],"result":{"outcome":{"outcome":"selected","optionId":"permit"}}})).await.unwrap();
    let (done, updates) = c.response(200).await;
    assert_eq!(done["result"]["stopReason"], "end_turn");
    assert_eq!(
        done["result"]["_meta"]["io.github.bkrabach.agent-runtime"]["inputReceipts"],
        json!([receipt])
    );
    assert_eq!(
        detail(&updates)["permission"]["outcome"]["optionId"],
        "permit"
    );
    assert_eq!(
        agent_instance::updates::status(&i.state).unwrap()["active_prompts"],
        0
    );
    {
        let db = i.state.db.lock().unwrap();
        let started: i64 = db
            .query_row(
                "SELECT count(*) FROM events WHERE kind='acp.prompt.started'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(started, 1);
        let events: String = db
            .query_row("SELECT group_concat(data) FROM events", [], |r| r.get(0))
            .unwrap();
        assert!(!events.contains("private-steering-canary"));
    }
    c.disconnect().await;
}

#[tokio::test]
async fn idle_steering_does_not_start_fallback_work_or_accept_unloaded_sessions() {
    let i = Installation::new();
    let mut c = i.connect();
    c.initialize().await;
    let sid = c.new_session(&i.work).await;
    c.send(200, "_session/steering", steering(&sid, "idle-input"))
        .await;
    assert_eq!(
        c.response(200).await.0["result"]["outcome"],
        "promptRequired"
    );
    assert_eq!(
        agent_instance::updates::status(&i.state).unwrap()["active_prompts"],
        0
    );
    for (id, params) in [
        (201, steering("unloaded-session", "wrong-session")),
        (202, {
            let mut p = steering(&sid, "unsafe-idle");
            p["_meta"]["steering"]["idleBehavior"] = json!("startNewTurn");
            p
        }),
    ] {
        c.send(id, "_session/steering", params).await;
        assert_eq!(c.response(id).await.0["error"]["code"], -32602);
    }
    let started: i64 = i
        .state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM events WHERE kind='acp.prompt.started'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(started, 0);
    c.disconnect().await;
}

#[tokio::test]
async fn steering_is_not_manufactured_for_backends_without_the_capability() {
    let i = Installation::new();
    i.state
        .config
        .write()
        .unwrap()
        .config
        .backend
        .as_mut()
        .unwrap()
        .env
        .insert("FIXTURE_NO_STEERING".into(), "1".into());
    let mut c = i.connect();
    assert_eq!(
        c.initialize().await["result"]["_meta"]["steering"]["supported"],
        false
    );
    let sid = c.new_session(&i.work).await;
    c.send(200, "_session/steering", steering(&sid, "unsupported"))
        .await;
    assert_eq!(c.response(200).await.0["error"]["code"], -32601);
    c.disconnect().await;
}

#[tokio::test]
async fn steering_requires_the_same_session_tool_authority_as_prompts() {
    let i = Installation::new();
    let mut c = i.connect();
    c.initialize().await;
    let sid = c.new_session(&i.work).await;
    i.state
        .set_record("session_tool_access", &sid, json!({"tools_required":true}))
        .unwrap();
    c.send(200, "_session/steering", steering(&sid, "tools-denied"))
        .await;
    assert_eq!(c.response(200).await.0["error"]["code"], -32002);
    c.disconnect().await;
}

#[tokio::test]
async fn busy_steering_consumes_attention_only_for_the_bound_session_and_current_ticket() {
    use agent_instance::{attention, state::now_ms};
    let i = Installation::new();
    i.state.config.write().unwrap().config.attention = Some(attention::Config {
        endpoint: "http://127.0.0.1:9/".into(),
        model: "fixture".into(),
        protocol: "system_one".into(),
        auth: "bearer".into(),
        credential_secret: "provider-test".into(),
        decisions_per_hour: 20,
        turns_per_hour: 5,
        uncertain_turns_per_hour: 5,
    });
    let mut c = i.connect();
    c.initialize().await;
    let sid = c.new_session(&i.work).await;
    let other = c.new_session(&i.work).await;
    c.send(
        200,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"REQUEST_PERMISSION"}]}),
    )
    .await;
    let callback = c.recv().await;
    assert_eq!(callback["method"], "session/request_permission");
    for (offset, case) in ["wrong-session", "expired", "revoked"]
        .into_iter()
        .enumerate()
    {
        let id = 210 + (offset as u32) * 10;
        let event = json!({"sessionId":sid,"occurredAt":now_ms(),"context":{"conversation_id":"room","agent":{"id":"a","name":"Agent"},"participants":[{"id":"a","name":"Agent"},{"id":"b","name":"Human"}],"messages":[{"id":case,"sender":"b","text":"An updated request"}],"trigger_id":case}});
        c.send(id, attention::METHOD, event).await;
        let offer = c.response(id).await.0["result"].clone();
        assert_eq!(offer["action"], "engage", "{offer}");
        let mut params = steering(
            if case == "wrong-session" {
                &other
            } else {
                &sid
            },
            case,
        );
        params["_meta"][attention::META] = json!({"id":offer["id"],"ticket":offer["ticket"]});
        if case == "expired" {
            let key = offer["id"].as_str().unwrap();
            let mut row = i
                .state
                .get_record("attention_events", key)
                .unwrap()
                .unwrap();
            row["expires_at"] = json!(0);
            i.state.set_record("attention_events", key, row).unwrap();
        } else if case == "revoked" {
            i.state.config.write().unwrap().config.purpose = "New attention policy identity".into();
        }
        c.send(id + 1, "_session/steering", params.clone()).await;
        assert_eq!(
            c.response(id + 1).await.0["error"]["code"],
            -32002,
            "{case}"
        );
        if case == "wrong-session" {
            params["sessionId"] = json!(sid);
            c.send(id + 2, "_session/steering", params.clone()).await;
            let (r, updates) = c.response(id + 2).await;
            assert_eq!(r["result"]["outcome"], "injected");
            assert!(
                r["result"]["fixtureParams"]["_meta"]
                    .get(attention::META)
                    .is_none()
            );
            assert_eq!(updates[0]["params"]["sessionId"], sid);
            c.send(id + 3, "_session/steering", params).await;
            assert_eq!(c.response(id + 3).await.0["error"]["code"], -32002);
        }
    }
    assert_eq!(
        attention::report(&i.state).unwrap()["counts"]["status:admitted"],
        1
    );
    assert_eq!(
        agent_instance::updates::status(&i.state).unwrap()["active_prompts"],
        1
    );
    c.tx.send(json!({"jsonrpc":"2.0","id":callback["id"],"result":{"outcome":{"outcome":"selected","optionId":"permit"}}})).await.unwrap();
    assert_eq!(c.response(200).await.0["result"]["stopReason"], "end_turn");
    c.disconnect().await;
}

fn attention_event(sid: &str, trigger: &str) -> Value {
    json!({"sessionId":sid,"occurredAt":agent_instance::state::now_ms(),"context":{"conversation_id":"room","agent":{"id":"a","name":"Agent"},"participants":[{"id":"a","name":"Agent"},{"id":"b","name":"Human"}],"messages":[{"id":trigger,"sender":"b","text":"An updated request"}],"trigger_id":trigger}})
}

fn one_attention_slot(i: &Installation) {
    i.state.config.write().unwrap().config.attention = Some(agent_instance::attention::Config {
        endpoint: "http://127.0.0.1:9/".into(),
        model: "fixture".into(),
        protocol: "system_one".into(),
        auth: "bearer".into(),
        credential_secret: "provider-test".into(),
        decisions_per_hour: 20,
        turns_per_hour: 1,
        uncertain_turns_per_hour: 1,
    });
}

#[tokio::test]
async fn prompt_completion_race_restores_same_attention_ticket_without_renewing_its_slot() {
    use agent_instance::attention;
    let i = Installation::new();
    one_attention_slot(&i);
    let mut c = i.connect();
    c.initialize().await;
    let sid = c.new_session(&i.work).await;
    c.send(
        200,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"REQUEST_PERMISSION"}]}),
    )
    .await;
    let callback = c.recv().await;
    assert_eq!(callback["method"], "session/request_permission");
    let event = attention_event(&sid, "race-input");
    c.send(201, attention::METHOD, event.clone()).await;
    let offer = c.response(201).await.0["result"].clone();
    assert_eq!(offer["action"], "engage");
    let mut input = steering(&sid, "race-input");
    input["prompt"][0]["text"] = json!("DEFER_STEERING_REPLY");
    input["_meta"][attention::META] = json!({"id":offer["id"],"ticket":offer["ticket"]});
    c.send(202, "_session/steering", input.clone()).await;
    assert_eq!(
        c.recv().await["params"]["update"]["content"]["text"],
        "Steering request held by fixture"
    );
    let key = offer["id"].as_str().unwrap();
    let before = i
        .state
        .get_record("attention_events", key)
        .unwrap()
        .unwrap();
    // The existing prompt finishes before the queued steering request gets its
    // definitive negative acknowledgment. No model input was incorporated.
    c.tx.send(json!({"jsonrpc":"2.0","id":callback["id"],"result":{"outcome":{"outcome":"selected","optionId":"permit"}}})).await.unwrap();
    assert_eq!(c.response(200).await.0["result"]["stopReason"], "end_turn");
    c.send(203, "_fixture/release_steering", json!({"sessionId":sid}))
        .await;
    let result = c.response(202).await.0;
    c.response(203).await;
    assert_eq!(result["result"]["outcome"], "promptRequired");
    assert_eq!(
        result["result"]["_meta"][attention::META],
        json!({"id":offer["id"],"ticketState":"restored","expiresAt":offer["expiresAt"]})
    );
    let restored = i
        .state
        .get_record("attention_events", key)
        .unwrap()
        .unwrap();
    assert_eq!(restored["status"], "offered");
    for field in [
        "occurred_at",
        "expires_at",
        "claimed_at",
        "requested_at",
        "ticket_hash",
    ] {
        assert_eq!(restored[field], before[field], "{field} must not renew");
    }
    assert_eq!(
        agent_instance::updates::status(&i.state).unwrap()["active_prompts"],
        0
    );
    input["prompt"][0]["text"] = json!("Deferred original input");
    c.send(204, "session/prompt", input.clone()).await;
    assert_eq!(c.response(204).await.0["result"]["stopReason"], "end_turn");
    let admitted = i
        .state
        .get_record("attention_events", key)
        .unwrap()
        .unwrap();
    assert_eq!(admitted["claimed_at"], before["claimed_at"]);
    assert_eq!(admitted["expires_at"], before["expires_at"]);
    c.send(205, "session/prompt", input).await;
    assert_eq!(c.response(205).await.0["error"]["code"], -32002);
    c.send(206, attention::METHOD, event).await;
    assert_eq!(c.response(206).await.0["result"]["reason"], "duplicate");
    c.send(
        207,
        attention::METHOD,
        attention_event(&sid, "another-input"),
    )
    .await;
    let next = c.response(207).await.0["result"].clone();
    let mut next_input = steering(&sid, "another-input");
    next_input["_meta"][attention::META] = json!({"id":next["id"],"ticket":next["ticket"]});
    c.send(208, "session/prompt", next_input).await;
    assert_eq!(
        c.response(208).await.0["error"]["code"],
        -32002,
        "The first admission still occupies its one budget slot"
    );
    c.disconnect().await;
}

#[tokio::test]
async fn steering_negative_ack_never_revives_expired_revoked_or_ambiguous_admissions() {
    use agent_instance::attention;
    for case in [
        "expired",
        "policy",
        "source-edit",
        "cancel",
        "failed",
        "lost-reply",
    ] {
        let i = Installation::new();
        one_attention_slot(&i);
        let mut c = i.connect();
        c.initialize().await;
        let sid = c.new_session(&i.work).await;
        let event = attention_event(&sid, case);
        c.send(200, attention::METHOD, event.clone()).await;
        let offer = c.response(200).await.0["result"].clone();
        assert_eq!(offer["action"], "engage");
        let mut input = steering(&sid, case);
        input["prompt"][0]["text"] = json!("DEFER_STEERING_REPLY");
        input["_meta"][attention::META] = json!({"id":offer["id"],"ticket":offer["ticket"]});
        c.send(201, "_session/steering", input.clone()).await;
        assert_eq!(
            c.recv().await["params"]["update"]["content"]["text"],
            "Steering request held by fixture"
        );
        let key = offer["id"].as_str().unwrap();
        let before = i
            .state
            .get_record("attention_events", key)
            .unwrap()
            .unwrap();
        match case {
            "expired" => {
                let mut row = before.clone();
                row["expires_at"] = json!(0);
                i.state.set_record("attention_events", key, row).unwrap();
            }
            "policy" => i.state.config.write().unwrap().config.purpose = "Changed policy".into(),
            "source-edit" => {
                let mut changed = event;
                changed["context"]["messages"][0]["text"] = json!("Edited input");
                c.send(202, attention::METHOD, changed).await;
                assert_eq!(c.response(202).await.0["result"]["reason"], "duplicate");
            }
            "cancel" => c
                .tx
                .send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":sid}}))
                .await
                .unwrap(),
            "lost-reply" => {
                c.disconnect().await;
                let row = i
                    .state
                    .get_record("attention_events", key)
                    .unwrap()
                    .unwrap();
                assert_eq!(row["status"], "admitted");
                assert_eq!(row["claimed_at"], before["claimed_at"]);
                continue;
            }
            _ => (),
        }
        c.send(
            203,
            "_fixture/release_steering",
            json!({"sessionId":sid,"outcome":if case=="failed"{"failed"}else{"promptRequired"}}),
        )
        .await;
        let result = c.response(201).await.0;
        c.response(203).await;
        assert_ne!(
            result["result"]["_meta"][attention::META]["ticketState"],
            "restored",
            "{case}"
        );
        let after = i
            .state
            .get_record("attention_events", key)
            .unwrap()
            .unwrap();
        assert_eq!(after["status"], "admitted", "{case}");
        assert_eq!(after["claimed_at"], before["claimed_at"]);
        c.send(204, "session/prompt", input).await;
        assert_eq!(c.response(204).await.0["error"]["code"], -32002, "{case}");
        c.disconnect().await;
    }
}
