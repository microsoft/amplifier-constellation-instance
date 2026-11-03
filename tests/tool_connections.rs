use agent_instance::{
    acp, auth,
    config::{BackendConfig, ConfigDocument, InstanceConfig, private_dir, write_private},
    state::AppState,
    tool_connections::{self, Connection},
};
use axum::http::HeaderMap;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tempfile::TempDir;
use tokio::{sync::mpsc, task::JoinHandle};

struct Fixture {
    dir: TempDir,
    state: Arc<AppState>,
}

#[test]
fn private_token_file_is_bounded_and_never_accepts_shared_or_linked_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credential");
    write_private(&path, b"test-scoped-credential\n").unwrap();
    assert_eq!(
        acp::read_token_file(&path).unwrap(),
        "test-scoped-credential"
    );
    assert!(acp::read_token_file(dir.path()).is_err());
    for bytes in [b" \n".to_vec(), b"one\ntwo".to_vec(), vec![b'x'; 8193]] {
        write_private(&path, &bytes).unwrap();
        assert!(acp::read_token_file(&path).is_err());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        write_private(&path, b"test-scoped-credential").unwrap();
        let link = dir.path().join("linked");
        symlink(&path, &link).unwrap();
        assert!(acp::read_token_file(&link).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(acp::read_token_file(&path).is_err());
    }
}
fn connection() -> Connection {
    Connection {
        name: "project-tools".into(),
        url: "https://tools.example/mcp".into(),
        header_secrets: BTreeMap::from([("Authorization".into(), "project-auth".into())]),
    }
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for name in ["work", "profiles", "secrets", "fixture"] {
            private_dir(&dir.path().join(name)).unwrap();
        }
        write_private(
            &dir.path().join("secrets/project-auth"),
            b"Bearer private-mcp-canary",
        )
        .unwrap();
        let mut config = InstanceConfig::blank("Fixture".into(), "owner".into());
        config.setup_complete = true;
        config.enabled = true;
        config.tool_connections = vec![connection()];
        config.backend = Some(BackendConfig {
            command: agent_instance::platform::python().into(),
            args: vec![
                format!(
                    "{}/tests/fixtures/tool_backend.py",
                    env!("CARGO_MANIFEST_DIR")
                ),
                "${AGENT_HOME}/fixture".into(),
            ],
            env: BTreeMap::new(),
            secret_env: BTreeMap::new(),
            working_directory: "${AGENT_HOME}/work".into(),
            allowed_working_roots: vec![],
        });
        config.validate().unwrap();
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
        Self { dir, state }
    }
    fn token(&self, scopes: &[&str]) -> (HeaderMap, String) {
        let token = auth::issue_token(
            &self.state,
            "fixture",
            &scopes.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            3600,
        )
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            format!("Bearer {}", token["token"].as_str().unwrap())
                .parse()
                .unwrap(),
        );
        (headers, token["id"].as_str().unwrap().to_owned())
    }
    fn connect(&self, headers: HeaderMap) -> Client {
        let (tx, incoming) = mpsc::channel(16);
        let (outgoing, rx) = mpsc::channel(16);
        let state = self.state.clone();
        let task = tokio::spawn(async move {
            acp::relay_authenticated(state, "fixture-client".into(), headers, incoming, outgoing)
                .await
        });
        Client {
            tx,
            rx,
            task,
            next: 0,
        }
    }
    fn params(&self, sid: Option<&str>) -> Value {
        let mut p = json!({"cwd":self.dir.path().join("work"),"mcpServers":[]});
        if let Some(sid) = sid {
            p["sessionId"] = json!(sid);
        }
        p
    }
    fn no_leaks(&self) {
        for (table, column) in [("events", "data"), ("records", "value")] {
            let sql = format!("SELECT {column} FROM {table}");
            let db = self.state.db.lock().unwrap();
            let mut query = db.prepare(&sql).unwrap();
            for data in query.query_map([], |r| r.get::<_, String>(0)).unwrap() {
                assert!(!data.unwrap().contains("private-mcp-canary"));
            }
        }
        assert!(
            !serde_json::to_string(&*self.state.config.read().unwrap())
                .unwrap()
                .contains("private-mcp-canary")
        );
    }
}
struct Client {
    tx: mpsc::Sender<Value>,
    rx: mpsc::Receiver<Value>,
    task: JoinHandle<anyhow::Result<()>>,
    next: u64,
}
impl Client {
    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        self.tx
            .send(json!({"jsonrpc":"2.0","id":self.next,"method":method,"params":params}))
            .await
            .unwrap();
        loop {
            let v = tokio::time::timeout(Duration::from_secs(5), self.rx.recv())
                .await
                .unwrap()
                .unwrap();
            if v["id"] == self.next {
                assert!(!v.to_string().contains("private-mcp-canary"));
                return v;
            }
        }
    }
    async fn init(&mut self) {
        assert!(
            self.request("initialize", json!({"protocolVersion":1}))
                .await
                .get("error")
                .is_none()
        );
    }
    async fn close(self) {
        drop(self.tx);
        assert!(self.task.await.unwrap().is_ok());
    }
    async fn revoked(self) {
        assert!(
            tokio::time::timeout(Duration::from_secs(3), self.task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
    }
}

#[tokio::test]
async fn explicit_scope_injects_on_new_load_resume_without_history_secrets() {
    let f = Fixture::new();
    let (headers, _) = f.token(&["acp", "tools:use"]);
    let mut c = f.connect(headers.clone());
    c.init().await;
    let r = c.request("session/new", f.params(None)).await;
    assert_eq!(
        r["result"]["_meta"]["fixture"],
        json!([{"name":"project-tools","type":"http","url":"https://tools.example/mcp","headers":["Authorization"],"secret_matches":true}])
    );
    let sid = r["result"]["sessionId"].as_str().unwrap().to_owned();
    c.close().await;
    for method in ["session/load", "session/resume"] {
        let mut c = f.connect(headers.clone());
        c.init().await;
        let r = c.request(method, f.params(Some(&sid))).await;
        assert_eq!(r["result"]["_meta"]["fixture"][0]["secret_matches"], true);
        c.close().await;
    }
    f.no_leaks();
}

#[tokio::test]
async fn context_owner_and_admin_do_not_imply_tool_authority_or_load_access() {
    let f = Fixture::new();
    let (tools, _) = f.token(&["acp", "tools:use"]);
    let mut c = f.connect(tools);
    c.init().await;
    let sid = c.request("session/new", f.params(None)).await["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    c.close().await;
    for scopes in [vec!["acp"], vec!["acp", "context:owner"], vec!["admin"]] {
        let (headers, _) = f.token(&scopes);
        let mut c = f.connect(headers);
        c.init().await;
        let r = c.request("session/new", f.params(None)).await;
        assert_eq!(r["result"]["_meta"]["fixture"], json!([]));
        assert_eq!(
            c.request("session/load", f.params(Some(&sid))).await["error"]["code"],
            -32002
        );
        c.close().await;
    }
    f.no_leaks();
}

#[tokio::test]
async fn collisions_unsupported_backends_missing_credentials_and_budgets_fail_closed() {
    let f = Fixture::new();
    let (headers, _) = f.token(&["acp", "tools:use"]);
    let mut c = f.connect(headers.clone());
    c.init().await;
    for entry in [
        json!({"name":"project-tools","type":"http","url":"https://other.example/mcp","headers":[]}),
        json!({"name":"different","type":"http","url":"https://tools.example:443/mcp","headers":[]}),
    ] {
        let mut params = f.params(None);
        params["mcpServers"] = json!([entry]);
        assert!(
            c.request("session/new", params)
                .await
                .get("error")
                .is_some()
        );
    }
    let mut params = f.params(None);
    params["mcpServers"] = json!(
        (0..8)
            .map(|n| json!({"name":format!("client-{n}"),"command":"fixture","args":[],"env":[]}))
            .collect::<Vec<_>>()
    );
    assert!(
        c.request("session/new", params)
            .await
            .get("error")
            .is_some()
    );
    std::fs::remove_file(f.dir.path().join("secrets/project-auth")).unwrap();
    assert!(
        c.request("session/new", f.params(None))
            .await
            .get("error")
            .is_some()
    );
    c.close().await;
    f.state
        .config
        .write()
        .unwrap()
        .config
        .backend
        .as_mut()
        .unwrap()
        .env
        .insert("FIXTURE_HTTP".into(), "no".into());
    let mut c = f.connect(headers);
    c.init().await;
    assert!(
        c.request("session/new", f.params(None))
            .await
            .get("error")
            .is_some()
    );
    c.close().await;
    f.no_leaks();
}

#[tokio::test]
async fn token_expiry_revocation_tool_config_and_secret_changes_end_active_connections() {
    for mutation in ["expiry", "revoke", "config", "replace", "secret"] {
        let f = Fixture::new();
        let (headers, id) = f.token(&["acp", "tools:use"]);
        let mut c = f.connect(headers);
        c.init().await;
        assert!(
            c.request("session/new", f.params(None))
                .await
                .get("error")
                .is_none()
        );
        match mutation {
            "expiry" => {
                f.state
                    .db
                    .lock()
                    .unwrap()
                    .execute("UPDATE tokens SET expires_at=0 WHERE id=?1", [&id])
                    .unwrap();
            }
            "revoke" => {
                f.state
                    .db
                    .lock()
                    .unwrap()
                    .execute("DELETE FROM tokens WHERE id=?1", [&id])
                    .unwrap();
            }
            "config" => f
                .state
                .config
                .write()
                .unwrap()
                .config
                .tool_connections
                .clear(),
            "replace" => {
                f.state.config.write().unwrap().config.tool_connections[0].url =
                    "https://replacement.example/mcp".into()
            }
            "secret" => write_private(
                &f.dir.path().join("secrets/project-auth"),
                b"changed-private-canary",
            )
            .unwrap(),
            _ => unreachable!(),
        }
        c.revoked().await;
        f.no_leaks();
    }
}

#[test]
fn endpoint_and_header_validation_is_bounded() {
    let valid = connection();
    for url in [
        "http://127.0.0.1:8123/mcp",
        "http://[::1]:8123/mcp",
        "https://tools.example/mcp",
    ] {
        let mut c = valid.clone();
        c.url = url.into();
        assert!(tool_connections::validate(&[c]).is_ok());
    }
    for url in [
        "http://localhost/mcp",
        "http://192.0.2.1/mcp",
        "file:///mcp",
        "https://user:password@tools.example/mcp",
        "https://tools.example/mcp?token=x",
        "https://tools.example/mcp#fragment",
    ] {
        let mut c = valid.clone();
        c.url = url.into();
        assert!(tool_connections::validate(&[c]).is_err());
    }
    for header in ["Host", "Content-Length", "MCP-Session-ID", "bad\r\nheader"] {
        let mut c = valid.clone();
        c.header_secrets = BTreeMap::from([(header.into(), "secret".into())]);
        assert!(tool_connections::validate(&[c]).is_err());
    }
    let mut duplicate = valid.clone();
    duplicate
        .header_secrets
        .insert("authorization".into(), "different".into());
    assert!(tool_connections::validate(&[duplicate]).is_err());
    assert!(tool_connections::validate(&[valid.clone(), valid.clone()]).is_err());
    let mut reserved = valid;
    reserved
        .header_secrets
        .insert("X-Key".into(), "control-token".into());
    assert!(tool_connections::validate(&[reserved]).is_err());
}

#[tokio::test]
async fn resumed_tool_sessions_keep_revocation_even_when_connections_are_removed() {
    let f = Fixture::new();
    let (headers, id) = f.token(&["acp", "tools:use"]);
    let mut c = f.connect(headers.clone());
    c.init().await;
    let sid = c.request("session/new", f.params(None)).await["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    c.close().await;
    f.state
        .config
        .write()
        .unwrap()
        .config
        .tool_connections
        .clear();
    let mut c = f.connect(headers);
    c.init().await;
    assert_eq!(
        c.request("session/resume", f.params(Some(&sid))).await["result"]["_meta"]["fixture"],
        json!([])
    );
    f.state
        .db
        .lock()
        .unwrap()
        .execute("DELETE FROM tokens WHERE id=?1", [id])
        .unwrap();
    c.revoked().await;
    f.no_leaks();
}

#[tokio::test]
async fn installed_tools_preserve_client_servers_and_reject_unsafe_secret_values() {
    let f = Fixture::new();
    let (headers, _) = f.token(&["acp", "tools:use"]);
    let mut c = f.connect(headers.clone());
    c.init().await;
    let mut params = f.params(None);
    params["mcpServers"] = json!([{"type":"http","name":"client-tools","url":"https://client.example/mcp","headers":[]}]);
    let r = c.request("session/new", params).await;
    assert_eq!(r["result"]["_meta"]["fixture"][0]["name"], "client-tools");
    assert_eq!(r["result"]["_meta"]["fixture"][1]["secret_matches"], true);
    c.close().await;
    for secret in [
        "Bearer secret\r\nInjected: bad".to_string(),
        "x".repeat(8193),
    ] {
        write_private(
            &f.dir.path().join("secrets/project-auth"),
            secret.as_bytes(),
        )
        .unwrap();
        let mut c = f.connect(headers.clone());
        c.init().await;
        let result = c.request("session/new", f.params(None)).await;
        assert!(result.get("error").is_some());
        assert!(!result.to_string().contains("Injected"));
        c.close().await;
    }
    f.no_leaks();
}
