use agent_instance::{
    acp, auth,
    config::{BackendConfig, ConfigDocument, InstanceConfig, private_dir, write_private},
    context, server,
    state::AppState,
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;

#[derive(Clone)]
struct Source {
    revoked: Arc<AtomicBool>,
    seen: Arc<std::sync::Mutex<Vec<Value>>>,
}
async fn source_query(
    State(source): State<Source>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    assert_eq!(headers["authorization"], "Bearer provider-secret-canary");
    source.seen.lock().unwrap().push(body.clone());
    let grant = body["access"]["grant"].as_str().unwrap_or_default();
    if body["access"]["mode"] != "owner"
        && (!matches!(
            grant,
            "owner-grant-canary"
                | "group-grant-canary"
                | "group-robin-grant-canary"
                | "other-grant-canary"
        ) || source.revoked.load(Ordering::SeqCst))
    {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"invalid"})));
    }
    let owner = body["access"]["mode"] == "owner" || grant == "owner-grant-canary";
    let conversation = if grant == "other-grant-canary" {
        "other-room"
    } else if owner {
        "owner-dm"
    } else {
        "team-room"
    };
    let actor = if grant == "group-robin-grant-canary" {
        "robin"
    } else {
        "alex"
    };
    let value = match body["operation"].as_str().unwrap() {
        "authorize" => {
            json!({"owner":owner,"conversation_id":conversation,"actor_id":actor,"thread_id":null,"surface":"fixture","label":"Source room","observed_at":1})
        }
        "orientation" => {
            json!({"conversation_id":conversation,"parent_excerpt":[{"text":"Parent conversation says keep the chosen blue design","id":"parent-1"}],"threads":[{"thread_id":"branch-1","label":"Follow up"}],"observed_at":1})
        }
        "list" => {
            json!({"items":[{"id":"room","conversation_id":"team-room","label":"Team room","surface":"fixture"},{"id":"dm","conversation_id":"owner-dm","label":"Private owner discussion","surface":"fixture"}],"complete":true})
        }
        "read" => {
            json!({"conversation_id":body["conversation_id"],"thread_id":body["thread_id"],"messages":[{"id":"msg-1","text":"The chosen design is blue","actor":"alex","at":1,"url":"https://source.example/message/1"}],"complete":true,"observed_at":1})
        }
        _ => json!({}),
    };
    (StatusCode::OK, Json(value))
}
struct Fixture {
    _dir: tempfile::TempDir,
    state: Arc<AppState>,
    source: Source,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    owner: String,
    acp_only: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for folder in ["work", "profiles", "secrets", "fixture-state"] {
            private_dir(&dir.path().join(folder)).unwrap();
        }
        write_private(
            &dir.path().join("secrets/context-source"),
            b"provider-secret-canary",
        )
        .unwrap();
        let mut config = InstanceConfig::blank("One Helper".into(), "alex".into());
        config.enabled = true;
        config.setup_complete = true;
        config.backend = Some(BackendConfig {
            command: agent_instance::platform::python().into(),
            args: vec![
                format!(
                    "{}/tests/fixtures/context_backend.py",
                    env!("CARGO_MANIFEST_DIR")
                ),
                "${AGENT_HOME}/fixture-state".into(),
            ],
            env: BTreeMap::new(),
            secret_env: BTreeMap::new(),
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
        let state = Arc::new(AppState::open(dir.path().into()).unwrap());
        let source = Source {
            revoked: Arc::new(AtomicBool::new(false)),
            seen: Arc::new(std::sync::Mutex::new(vec![])),
        };
        let source_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source_url = format!("http://{}", source_listener.local_addr().unwrap());
        let source_router = Router::new()
            .route("/context/v1/query", post(source_query))
            .with_state(source.clone());
        let source_task =
            tokio::spawn(async move { axum::serve(source_listener, source_router).await.unwrap() });
        let registry = json!({"providers":[{"id":"web:fixture","label":"Web fixture","url":source_url,"secret_ref":"context-source"},{"id":"slack:fixture","label":"Slack fixture","url":source_url,"secret_ref":"context-source"}]});
        write_private(
            &dir.path().join("context-providers.json"),
            registry.to_string().as_bytes(),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        state.config.write().unwrap().config.server.public_url =
            Some(format!("http://{}", listener.local_addr().unwrap()));
        let router = server::router(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let owner = auth::issue_token(&state, "Owner", &["admin".into()], 3600).unwrap()["token"]
            .as_str()
            .unwrap()
            .to_owned();
        let acp_only =
            auth::issue_token(&state, "Adapter", &["acp".into()], 3600).unwrap()["token"]
                .as_str()
                .unwrap()
                .to_owned();
        Self {
            _dir: dir,
            state,
            source,
            tasks: vec![task, source_task],
            owner,
            acp_only,
        }
    }
    fn headers(&self, owner: bool) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            "authorization",
            format!(
                "Bearer {}",
                if owner { &self.owner } else { &self.acp_only }
            )
            .parse()
            .unwrap(),
        );
        h
    }
    fn client(&self, owner: bool) -> Client {
        let (tx, rx_in) = mpsc::channel(32);
        let (tx_out, rx) = mpsc::channel(32);
        let state = self.state.clone();
        let headers = self.headers(owner);
        let task = tokio::spawn(async move {
            acp::relay_authenticated(state, "fixture".into(), headers, rx_in, tx_out).await
        });
        Client { tx, rx, task }
    }
    fn params(&self, grant: Option<&str>) -> Value {
        let mut value = json!({"cwd":self.state.home.join("work"),"mcpServers":[{"name":"client-tools","command":"fixture","args":[],"env":[]}]});
        if let Some(grant) = grant {
            value["_meta"] = json!({context::META:{"provider_id":"web:fixture","grant":grant},"preserved-client-metadata":true});
        }
        value
    }
}
struct Client {
    tx: mpsc::Sender<Value>,
    rx: mpsc::Receiver<Value>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

/// Run explicitly with AGENT_RUNTIME_TEST_BINARY pointing at the native runtime.
/// A separate release artifact keeps this test from coupling repository builds.
#[tokio::test]
#[ignore = "requires an explicit native runtime build"]
async fn native_history_survives_reconnect_and_stays_private_through_context_mcp() {
    let f = Fixture::new().await;
    let binary = std::path::PathBuf::from(
        std::env::var("AGENT_RUNTIME_TEST_BINARY").expect("Supply the native runtime binary"),
    );
    assert!(binary.is_absolute());
    let engine = f.state.home.join("engine");
    let output = tokio::process::Command::new(&binary)
        .args([
            "--home",
            engine.to_str().unwrap(),
            "init",
            "--workspace",
            f.state.home.join("work").to_str().unwrap(),
            "--provider",
            "mock",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    f.state.config.write().unwrap().config.backend = Some(BackendConfig {
        command: binary.to_string_lossy().into(),
        args: vec![
            "--home".into(),
            engine.to_string_lossy().into(),
            "acp".into(),
        ],
        env: BTreeMap::new(),
        secret_env: BTreeMap::new(),
        working_directory: "${AGENT_HOME}/work".into(),
        allowed_working_roots: vec!["${AGENT_HOME}/work".into()],
    });
    let params = |grant: Option<&str>| {
        let mut p = f.params(grant);
        p["mcpServers"] = json!([]);
        p
    };
    let mut owner = f.client(true);
    owner.init().await;
    let sid = owner.new_session(params(None)).await;
    let (response, _) = owner
        .request(
            3,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"owner-private-canary"}]}),
        )
        .await;
    assert!(response["error"].is_null(), "{response}");
    owner.close().await;
    let mut group = f.client(false);
    group.init().await;
    let group_sid = group.new_session(params(Some("group-grant-canary"))).await;
    group.close().await;
    let mut owner = f.client(true);
    owner.init().await;
    let second = owner.new_session(params(None)).await;
    let registered = f.state.list_records("native_history").unwrap();
    assert_eq!(registered.len(), 2);
    assert!(!json!(registered).to_string().contains(&group_sid));
    let listing = agent_instance::native_history::list(&f.state, true, 1, None)
        .await
        .unwrap();
    assert_eq!(listing["items"].as_array().unwrap().len(), 1);
    let next = listing["next_cursor"].as_str().unwrap();
    let page = agent_instance::native_history::list(&f.state, true, 1, Some(next))
        .await
        .unwrap();
    assert_ne!(
        listing["items"][0]["conversation_id"],
        page["items"][0]["conversation_id"]
    );
    assert!(
        agent_instance::native_history::read(&f.state, true, &group_sid, 20, None)
            .await
            .is_err()
    );
    let mut calls = vec![];
    for (name, grant) in [
        ("manual-owner", None),
        ("manual-group", Some("group-grant-canary")),
        ("manual-dm", Some("owner-grant-canary")),
    ] {
        f.state
            .connections
            .lock()
            .unwrap()
            .insert(name.into(), json!({"id":name}));
        let headers = f.headers(grant.is_none());
        let mut p = params(grant);
        context::prepare(
            &f.state,
            name,
            grant.is_none(),
            Some(&headers),
            &mut p,
            true,
        )
        .await
        .unwrap()
        .unwrap();
        let mcp = p["mcpServers"].as_array().unwrap().last().unwrap();
        let url = mcp["url"].as_str().unwrap().to_owned();
        let token = mcp["headers"][0]["value"].as_str().unwrap().to_owned();
        calls.push((url, token));
    }
    let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"context_read","arguments":{"provider_id":"native","conversation_id":sid}}});
    let http = reqwest::Client::new();
    for (i, (url, token)) in calls.iter().enumerate() {
        let result: Value = http
            .post(url)
            .header("authorization", token)
            .json(&request)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(result["result"]["isError"], i == 1, "{result}");
        assert_eq!(
            result.to_string().contains("owner-private-canary"),
            i != 1,
            "{result}"
        );
        assert!(
            !result
                .to_string()
                .contains("Configured agent instance context")
        );
    }
    f.source.revoked.store(true, Ordering::SeqCst);
    let result: Value = http
        .post(&calls[2].0)
        .header("authorization", &calls[2].1)
        .json(&request)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!result.to_string().contains("owner-private-canary"));
    f.state
        .db
        .lock()
        .unwrap()
        .execute("DELETE FROM tokens WHERE hash=?1", [auth::digest(&f.owner)])
        .unwrap();
    let denied = http
        .post(&calls[0].0)
        .header("authorization", &calls[0].1)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert!(!denied.status().is_success());
    assert!(
        !denied
            .text()
            .await
            .unwrap()
            .contains("owner-private-canary")
    );
    owner.close().await;
    // A replacement engine home has a distinct store identity and must never
    // inherit old registrations just because it is configured under this agent.
    let replacement = f.state.home.join("replacement");
    let output = tokio::process::Command::new(&binary)
        .args([
            "--home",
            replacement.to_str().unwrap(),
            "init",
            "--workspace",
            f.state.home.join("work").to_str().unwrap(),
            "--provider",
            "mock",
        ])
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    f.state
        .config
        .write()
        .unwrap()
        .config
        .backend
        .as_mut()
        .unwrap()
        .args[1] = replacement.to_string_lossy().into();
    assert!(
        agent_instance::native_history::list(&f.state, true, 40, None)
            .await
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        agent_instance::native_history::read(&f.state, true, &second, 20, None)
            .await
            .is_err()
    );
}
impl Client {
    async fn request(&mut self, id: u64, method: &str, params: Value) -> (Value, Vec<Value>) {
        self.tx
            .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await
            .unwrap();
        let mut updates = vec![];
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(8), self.rx.recv())
                .await
                .unwrap()
                .unwrap();
            if msg.get("method").is_none() && msg["id"] == id {
                return (msg, updates);
            }
            updates.push(msg);
        }
    }
    async fn init(&mut self) {
        assert!(
            self.request(
                1,
                "initialize",
                json!({"protocolVersion":1,"clientCapabilities":{}})
            )
            .await
            .0["result"]
                .is_object()
        );
    }
    async fn new_session(&mut self, params: Value) -> String {
        let result = self.request(2, "session/new", params).await.0;
        assert!(result["error"].is_null(), "{result}");
        result["result"]["sessionId"].as_str().unwrap().into()
    }
    async fn prompt(&mut self, sid: &str, tool: Value) -> Value {
        let(r,u)=self.request(3,"session/prompt",json!({"sessionId":sid,"prompt":[{"type":"text","text":format!("CONTEXT_TOOL {}",tool)}]})).await;
        assert!(r["error"].is_null(), "{r}");
        serde_json::from_str(
            u.last().unwrap()["params"]["update"]["content"]["text"]
                .as_str()
                .unwrap(),
        )
        .unwrap()
    }
    async fn close(self) {
        drop(self.tx);
        assert!(
            tokio::time::timeout(Duration::from_secs(4), self.task)
                .await
                .unwrap()
                .unwrap()
                .is_ok()
        );
    }
}

#[tokio::test]
async fn steering_revalidates_context_and_cannot_borrow_another_connections_audience() {
    let f = Fixture::new().await;
    let mut client = f.client(false);
    client.init().await;
    let sid = client
        .new_session(f.params(Some("group-grant-canary")))
        .await;
    let params = json!({"sessionId":sid,"prompt":[{"type":"text","text":"An ordinary follow-up"}],"_meta":{"steering":{"inputId":"context-update-1","idleBehavior":"promptRequired"}}});
    let (response, updates) = client
        .request(20, "_session/steering", params.clone())
        .await;
    assert_eq!(
        response["result"]["outcome"], "promptRequired",
        "{response}"
    );
    let detail: Value = serde_json::from_str(
        updates.last().unwrap()["params"]["update"]["content"]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let orientation = detail["orientation"].as_str().unwrap();
    assert!(orientation.contains("Name: One Helper"));
    assert!(orientation.contains("Parent conversation says keep the chosen blue design"));
    assert!(orientation.contains("team-room"));
    assert!(!orientation.contains("other-room"));
    let mut other = f.client(false);
    other.init().await;
    let denied = other
        .request(20, "_session/steering", params.clone())
        .await
        .0;
    assert_eq!(denied["error"]["code"], -32602);
    let mut wrong_load = f.params(Some("other-grant-canary"));
    wrong_load["sessionId"] = json!(sid);
    assert!(other.request(21, "session/load", wrong_load).await.0["error"].is_object());
    assert!(
        other
            .request(22, "_session/steering", params.clone())
            .await
            .0["error"]
            .is_object()
    );
    f.source.revoked.store(true, Ordering::SeqCst);
    let (denied, updates) = client.request(21, "_session/steering", params).await;
    assert_eq!(denied["error"]["code"], -32002);
    assert!(
        updates.is_empty(),
        "Revoked context must never reach the backend"
    );
    other.close().await;
    client.close().await;
}
#[tokio::test]
async fn owner_native_clients_get_merged_mcp_and_cross_surface_context_without_custom_client_tools()
{
    let f = Fixture::new().await;
    let mut client = f.client(true);
    client.init().await;
    let sid = client.new_session(f.params(None)).await;
    let result = client
        .prompt(&sid, json!({"name":"context_list","arguments":{}}))
        .await;
    assert_eq!(
        result["mcp_names"],
        json!(["client-tools", "agent-continuity"])
    );
    let items = &result["call"]["result"]["structuredContent"]["items"];
    assert_eq!(items.as_array().unwrap().len(), 4);
    assert_eq!(
        result["tools"]["result"]["tools"].as_array().unwrap().len(),
        13
    );
    assert_eq!(
        result["initialize"]["result"]["protocolVersion"],
        "2025-06-18"
    );
    let remembered=client.prompt(&sid,json!({"name":"memory_remember","arguments":{"key":"release-check","statement":"Check the artifact before shipping","source_note":"Owner requested this explicitly"}})).await;
    let memory = &remembered["call"]["result"]["structuredContent"];
    assert_eq!(memory["audience"]["kind"], "owner");
    assert_eq!(
        memory["candidates"][0]["statement"],
        "Check the artifact before shipping"
    );
    let proposed=client.prompt(&sid,json!({"name":"work_create","arguments":{"title":"Check the actual artifact","goal":"Run and record the agreed checks"}})).await;
    let work = &proposed["call"]["result"]["structuredContent"];
    assert_eq!(work["state"], "proposed");
    assert_eq!(work["audience"]["kind"], "owner");
    let accepted=client.prompt(&sid,json!({"name":"work_command","arguments":{"id":work["id"],"expected_revision":work["revision"],"command":{"type":"accept"}}})).await;
    assert_eq!(
        accepted["call"]["result"]["structuredContent"]["state"],
        "accepted"
    );

    let read=client.prompt(&sid,json!({"name":"context_read","arguments":{"provider_id":"slack:fixture","conversation_id":"team-room","thread_id":"branch-1"}})).await;
    assert_eq!(
        read["call"]["result"]["structuredContent"]["source"]["messages"][0]["text"],
        "The chosen design is blue"
    );
    assert!(
        read["orientation"]
            .as_str()
            .unwrap()
            .contains("same individual across experiences")
    );
    client.close().await;
    assert!(f.state.context_leases.lock().unwrap().is_empty());
    let records = json!(f.state.list_records("context_bindings").unwrap()).to_string();
    assert!(!records.contains("canary"));
}
#[tokio::test]
async fn private_owner_context_does_not_require_administration_and_revocation_is_immediate() {
    let mut f = Fixture::new().await;
    let issued = auth::issue_token(
        &f.state,
        "Private ACP client",
        &["acp".into(), "context:owner".into()],
        3600,
    )
    .unwrap();
    f.owner = issued["token"].as_str().unwrap().to_owned();
    assert!(auth::check(&f.state, &f.headers(true), "admin", false).is_err());
    assert!(auth::check(&f.state, &f.headers(true), "profiles", false).is_err());
    let mut client = f.client(true);
    client.init().await;
    let sid = client.new_session(f.params(None)).await;
    let result = client
        .prompt(&sid, json!({"name":"context_list","arguments":{}}))
        .await;
    assert_eq!(
        result["call"]["result"]["structuredContent"]["items"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    client.close().await;
    let mut restored = f.client(true);
    restored.init().await;
    let mut params = f.params(None);
    params["sessionId"] = json!(sid);
    assert!(restored.request(10, "session/load", params).await.0["error"].is_null());
    f.state
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE tokens SET scopes='[\"acp\"]' WHERE id=?1",
            [issued["id"].as_str().unwrap()],
        )
        .unwrap();
    assert!(auth::check(&f.state, &f.headers(true), "acp", false).is_ok());
    let denied = restored
        .request(
            11,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"read context"}]}),
        )
        .await;
    assert!(denied.0["error"].is_object());
    restored.close().await;
    let scope_only =
        auth::issue_token(&f.state, "Context only", &["context:owner".into()], 3600).unwrap();
    f.owner = scope_only["token"].as_str().unwrap().to_owned();
    assert!(auth::check(&f.state, &f.headers(true), "acp", false).is_err());
}

#[tokio::test]
async fn acp_only_never_implies_owner_and_grants_are_stripped_revalidated_and_audience_bound() {
    let f = Fixture::new().await;
    let mut plain = f.client(false);
    plain.init().await;
    let sid = plain.new_session(f.params(None)).await;
    let (r, u) = plain
        .request(
            3,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"normal prompt"}]}),
        )
        .await;
    assert!(r["error"].is_null());
    let detail: Value = serde_json::from_str(
        u[0]["params"]["update"]["content"]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(detail["mcp_names"], json!(["client-tools"]));
    plain.close().await;
    let mut group = f.client(false);
    group.init().await;
    let sid = group
        .new_session(f.params(Some("group-grant-canary")))
        .await;
    let result = group
        .prompt(&sid, json!({"name":"context_list","arguments":{}}))
        .await;
    assert_eq!(
        result["call"]["result"]["structuredContent"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(result["metadata"].get(context::META).is_none());
    assert!(
        !result["orientation"]
            .as_str()
            .unwrap()
            .contains("Private owner discussion")
    );
    assert_eq!(result["metadata"]["preserved-client-metadata"], true);
    assert!(!result.to_string().contains("grant-canary"));
    assert!(
        result["orientation"]
            .as_str()
            .unwrap()
            .contains("Parent conversation")
    );
    for (provider, conversation) in [("slack:fixture", "team-room"), ("web:fixture", "owner-dm")] {
        let denied=group.prompt(&sid,json!({"name":"context_read","arguments":{"provider_id":provider,"conversation_id":conversation}})).await;
        assert_eq!(denied["call"]["result"]["isError"], true);
    }
    let allowed=group.prompt(&sid,json!({"name":"context_read","arguments":{"provider_id":"web:fixture","conversation_id":"team-room","thread_id":"branch-1"}})).await;
    assert_eq!(allowed["call"]["result"]["isError"], false);
    f.source.revoked.store(true, Ordering::SeqCst);
    let denied = group
        .request(
            4,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"after revocation"}]}),
        )
        .await
        .0;
    assert!(denied["error"].is_object());
    group.close().await;
    f.source.revoked.store(false, Ordering::SeqCst);
    let mut admin = f.client(true);
    admin.init().await;
    let mut params = f.params(None);
    params["sessionId"] = json!(sid);
    assert!(
        admin.request(6, "session/load", params).await.0["error"].is_object(),
        "Admin must not widen a group session to owner-private context"
    );
    let mut params = f.params(Some("group-grant-canary"));
    params["sessionId"] = json!(sid);
    assert!(admin.request(7, "session/load", params).await.0["error"].is_null());
    let result = admin
        .prompt(&sid, json!({"name":"context_list","arguments":{}}))
        .await;
    assert_eq!(
        result["call"]["result"]["structuredContent"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    admin.close().await;
    let mut robin = f.client(false);
    robin.init().await;
    let mut params = f.params(Some("group-robin-grant-canary"));
    params["sessionId"] = json!(sid);
    assert!(
        robin.request(8, "session/load", params).await.0["error"].is_null(),
        "Another verified member can continue the same group conversation"
    );
    let result = robin
        .prompt(&sid, json!({"name":"context_list","arguments":{}}))
        .await;
    assert_eq!(
        result["call"]["result"]["structuredContent"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    robin.close().await;
    let audit: String = f
        .state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT group_concat(data) FROM events", [], |r| r.get(0))
        .unwrap();
    assert!(!audit.contains("canary"));
    assert!(!audit.contains("chosen design"));
}
#[tokio::test]
async fn owner_grants_cross_providers_but_resumed_sessions_require_the_same_verified_audience() {
    let f = Fixture::new().await;
    let mut first = f.client(false);
    first.init().await;
    let sid = first
        .new_session(f.params(Some("owner-grant-canary")))
        .await;
    let result = first
        .prompt(&sid, json!({"name":"context_list","arguments":{}}))
        .await;
    assert_eq!(
        result["call"]["result"]["structuredContent"]["items"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let briefing = result["orientation"].as_str().unwrap();
    assert!(briefing.contains("authorized_activity"));
    assert!(briefing.contains("Private owner discussion"));
    first.close().await;
    let mut second = f.client(false);
    second.init().await;
    for grant in [
        None,
        Some("group-grant-canary"),
        Some("wrong-grant"),
        Some("other-grant-canary"),
    ] {
        let mut params = f.params(grant);
        params["sessionId"] = json!(sid);
        assert!(second.request(5, "session/load", params).await.0["error"].is_object());
    }
    let listed = second.request(6, "session/list", json!({})).await.0;
    assert_eq!(listed["result"]["sessions"], json!([]));
    let mut params = f.params(Some("owner-grant-canary"));
    params["sessionId"] = json!(sid);
    assert!(second.request(7, "session/load", params).await.0["error"].is_null());
    assert_eq!(
        second
            .prompt(&sid, json!({"name":"context_list","arguments":{}}))
            .await["call"]["result"]["isError"],
        false
    );
    second.close().await;
}
#[tokio::test]
async fn missing_provider_and_http_capability_do_not_silently_enable_context() {
    let f = Fixture::new().await;
    f.state
        .config
        .write()
        .unwrap()
        .config
        .backend
        .as_mut()
        .unwrap()
        .env
        .insert("FIXTURE_NO_HTTP".into(), "1".into());
    let mut client = f.client(false);
    client.init().await;
    let denied = client
        .request(2, "session/new", f.params(Some("owner-grant-canary")))
        .await
        .0;
    assert!(
        denied["error"]["message"]
            .as_str()
            .unwrap()
            .contains("HTTP MCP")
    );
    let mut params = f.params(Some("owner-grant-canary"));
    params["_meta"][context::META]["provider_id"] = json!("unregistered");
    assert!(client.request(2, "session/new", params).await.0["error"].is_object());
    assert!(f.state.context_leases.lock().unwrap().is_empty());
    client.close().await;
}
#[tokio::test]
async fn mcp_tokens_expire_revoke_with_connection_and_do_not_accept_management_tokens() {
    let f = Fixture::new().await;
    let mut client = f.client(true);
    client.init().await;
    let sid = client.new_session(f.params(None)).await;
    let endpoint = format!(
        "{}/context/mcp",
        f.state.instance_config().server.public_url.unwrap()
    );
    let reply = reqwest::Client::new()
        .post(endpoint)
        .bearer_auth(&f.owner)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::UNAUTHORIZED);
    for lease in f.state.context_leases.lock().unwrap().values_mut() {
        lease.expires_at = 0;
    }
    assert!(
        client
            .request(
                4,
                "session/prompt",
                json!({"sessionId":sid,"prompt":[{"type":"text","text":"expired"}]})
            )
            .await
            .0["error"]
            .is_object()
    );
    client.close().await;
}
#[tokio::test]
async fn registry_is_private_validated_and_can_be_configured_without_exposing_credentials() {
    let f = Fixture::new().await;
    let url = format!(
        "{}/api/v1/context/providers",
        f.state.instance_config().server.public_url.unwrap()
    );
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(&url)
            .bearer_auth(&f.acp_only)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let data: Value = client
        .get(&url)
        .bearer_auth(&f.owner)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!data.to_string().contains("provider-secret-canary"));
    assert_eq!(data["providers"][0]["configured"], true);
    for (url, secret) in [
        ("http://192.168.1.5", "context-source"),
        ("https://user:pass@example.com", "context-source"),
        ("https://example.com?q=token", "context-source"),
        ("https://example.com", "control-token"),
    ] {
        let registry:context::Registry=serde_json::from_value(json!({"providers":[{"id":"web:fixture","label":"Fixture","url":url,"secret_ref":secret}]})).unwrap();
        assert!(context::validate_registry(&registry).is_err());
    }
    let valid = json!({"providers":[{"id":"web:fixture","label":"Fixture","url":"https://source.example/prefix","secret_ref":"context-source"}]});
    assert_eq!(
        client
            .put(&url)
            .bearer_auth(&f.owner)
            .json(&valid)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn mcp_http_rejects_foreign_origins_and_rechecks_the_original_acp_credential() {
    let f = Fixture::new().await;
    f.state
        .connections
        .lock()
        .unwrap()
        .insert("manual-fixture".into(), json!({"id":"manual-fixture"}));
    let headers = f.headers(true);
    let mut params = f.params(None);
    let prepared = context::prepare(
        &f.state,
        "manual-fixture",
        true,
        Some(&headers),
        &mut params,
        true,
    )
    .await
    .unwrap()
    .unwrap();
    let mcp = params["mcpServers"].as_array().unwrap().last().unwrap();
    let endpoint = mcp["url"].as_str().unwrap();
    let authorization = mcp["headers"][0]["value"].as_str().unwrap();
    let client = reqwest::Client::new();
    let rpc = json!({"jsonrpc":"2.0","id":1,"method":"tools/list"});
    assert_eq!(
        client
            .post(endpoint)
            .header("authorization", authorization)
            .header("origin", "https://foreign.example")
            .json(&rpc)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        client
            .post(endpoint)
            .header("authorization", authorization)
            .header("mcp-protocol-version", "2099-01-01")
            .json(&rpc)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        client
            .get(endpoint)
            .header("authorization", authorization)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        client
            .post(endpoint)
            .header("authorization", authorization)
            .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        client
            .post(endpoint)
            .header("authorization", authorization)
            .json(&rpc)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    f.state
        .db
        .lock()
        .unwrap()
        .execute("DELETE FROM tokens WHERE hash=?1", [auth::digest(&f.owner)])
        .unwrap();
    assert_eq!(
        client
            .post(endpoint)
            .header("authorization", authorization)
            .json(&rpc)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    context::revoke(&f.state, &prepared.hash);
    assert_eq!(
        client
            .post(endpoint)
            .header("authorization", authorization)
            .json(&rpc)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn unknown_legacy_audience_and_native_owner_history_cannot_be_loaded_by_a_bare_adapter() {
    let f = Fixture::new().await;
    let mut native = f.client(true);
    native.init().await;
    let native_sid = native.new_session(f.params(None)).await;
    native.close().await;
    let mut adapter = f.client(false);
    adapter.init().await;
    for (sid, grant) in [
        (native_sid.as_str(), None),
        ("legacy-unbound-session", None),
        ("legacy-unbound-session", Some("owner-grant-canary")),
        ("legacy-unbound-session", Some("group-grant-canary")),
    ] {
        let mut params = f.params(grant);
        params["sessionId"] = json!(sid);
        assert!(adapter.request(5, "session/load", params).await.0["error"].is_object());
    }
    adapter.close().await;
    let mut native = f.client(true);
    native.init().await;
    let mut params = f.params(None);
    params["sessionId"] = json!(native_sid);
    assert!(native.request(6, "session/load", params).await.0["error"].is_null());
    native.close().await;
}

#[tokio::test]
async fn global_update_switch_allows_non_owner_conversation_and_revokes_immediately() {
    let f = Fixture::new().await;
    f.state
        .set_record(
            "updates",
            "helper",
            json!({"observed_at":agent_instance::state::now_ms()}),
        )
        .unwrap();
    let mut group = f.client(false);
    group.init().await;
    let sid = group
        .new_session(f.params(Some("group-robin-grant-canary")))
        .await;
    let command = json!({"name":"installation_updates","arguments":{"action":"check"}});
    let disabled = group.prompt(&sid, command.clone()).await;
    assert_eq!(disabled["call"]["result"]["isError"], true, "{disabled}");
    let mut policy = agent_instance::updates::Policy {
        agent_initiated: true,
        ..Default::default()
    };
    f.state
        .set_record("updates", "policy", json!(policy))
        .unwrap();
    let enabled = group.prompt(&sid, command.clone()).await;
    assert_eq!(enabled["call"]["result"]["isError"], false, "{enabled}");
    assert_eq!(
        enabled["call"]["result"]["structuredContent"]["pending"], true,
        "{enabled}"
    );
    policy.agent_initiated = false;
    f.state
        .set_record("updates", "policy", json!(policy))
        .unwrap();
    let revoked = group.prompt(&sid, command).await;
    assert_eq!(revoked["call"]["result"]["isError"], true, "{revoked}");
    group.close().await;
}
