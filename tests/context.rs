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
    member: Arc<AtomicBool>,
    history_pages: Arc<AtomicBool>,
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
        ) || source.revoked.load(Ordering::SeqCst)
            || !source.member.load(Ordering::SeqCst))
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
        "read" if source.history_pages.load(Ordering::SeqCst) => {
            let (id, seq, text, next) = match body.get("before") {
                None => ("recent-20", 20, "Recent message only", json!(20)),
                Some(position) if position == 20 => {
                    ("older-10", 10, "Older source-only marker", Value::Null)
                }
                _ => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error":"Unsupported position"})),
                    );
                }
            };
            json!({"conversation_id":body["conversation_id"],"thread_id":body["thread_id"],"messages":[{"id":id,"seq":seq,"text":text,"actor":"alex","at":1,"url":format!("https://source.example/message/{id}")}],"complete":false,"next_cursor":next,"coverage":"Bounded source messages","observed_at":1})
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
            member: Arc::new(AtomicBool::new(true)),
            history_pages: Arc::new(AtomicBool::new(false)),
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
async fn original_input_recovers_work_after_lost_replies_without_replay_or_widening_audience() {
    use agent_instance::source_delivery as delivery;
    let f = Fixture::new().await;
    let mut client = f.client(false);
    client.init().await;
    let sid = client
        .new_session(f.params(Some("group-grant-canary")))
        .await;
    let source = json!({"id":"message-17","revision":"edited-r2","occurredAt":agent_instance::state::now_ms()});
    let args = json!({"title":"Check the proposal","goal":"Review the recorded proposal","source_input_id":"input-17","request_id":"creation-17-a"});
    let prompt = json!({"sessionId":sid,"prompt":[{"type":"text","text":format!("CONTEXT_TOOL {}",json!({"name":"work_create","arguments":args}))}],"_meta":{delivery::META:{"version":1,"inputId":"input-17","sources":[source]}}});
    // The fixture calls the actual HTTP MCP tool. The caller loses all response
    // frames; only this test's independent record oracle waits for the commit.
    client
        .tx
        .send(json!({"jsonrpc":"2.0","id":40,"method":"session/prompt","params":prompt}))
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(w) = f.state.list_records("work").unwrap().first() {
                break w.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    client.close().await;
    assert_eq!(first["state"], "proposed");
    assert_eq!(first["source"]["message_id"], "message-17");
    assert_eq!(first["source_input"]["sources"], json!([source]));

    let mut client = f.client(false);
    client.init().await;
    let mut load = f.params(Some("group-grant-canary"));
    load["sessionId"] = json!(sid);
    assert!(client.request(41, "session/load", load).await.0["error"].is_null());
    let query = json!({"sessionId":sid,"sources":[source]});
    let r = client.request(42, delivery::METHOD, query.clone()).await.0;
    assert_eq!(
        r["result"]["receipts"][0]["work"][0]["id"], first["id"],
        "{r}"
    );
    assert_eq!(r["result"]["receipts"][0]["work"][0]["state"], "proposed");
    let before = f.state.list_records("work").unwrap();
    let duplicate = client.request(43, "session/prompt", prompt).await.0;
    assert!(duplicate["error"].is_object(), "{duplicate}");
    assert_eq!(f.state.list_records("work").unwrap(), before);

    // A specifically correlated creation can be inspected/retried through the
    // existing tool; the original ACP input itself is never replayed.
    let recovered = client
        .prompt(&sid, json!({"name":"work_create","arguments":args}))
        .await;
    assert_eq!(
        recovered["call"]["result"]["structuredContent"]["id"],
        first["id"]
    );
    let mut changed = args.clone();
    changed["goal"] = json!("Different intent under the same key");
    let refused = client
        .prompt(&sid, json!({"name":"work_create","arguments":changed}))
        .await;
    assert_eq!(refused["call"]["result"]["isError"], true);

    let acceptance = json!({"name":"work_command","arguments":{"id":first["id"],"expected_revision":1,"command":{"type":"accept"}}});
    client.tx.send(json!({"jsonrpc":"2.0","id":44,"method":"session/prompt","params":{"sessionId":sid,"prompt":[{"type":"text","text":format!("CONTEXT_TOOL {acceptance}")}]}})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if f.state
                .get_record("work", first["id"].as_str().unwrap())
                .unwrap()
                .unwrap()["state"]
                == "accepted"
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    client.close().await;
    let mut client = f.client(false);
    client.init().await;
    // Same room, another authorized participant: retain the original creation.
    let mut load = f.params(Some("group-robin-grant-canary"));
    load["sessionId"] = json!(sid);
    assert!(client.request(45, "session/load", load).await.0["error"].is_null());
    let r = client.request(46, delivery::METHOD, query.clone()).await.0;
    assert_eq!(r["result"]["receipts"][0]["work"][0]["state"], "accepted");
    assert_eq!(r["result"]["receipts"][0]["completion"], "not_established");

    // Two authorized creations from the same original input remain two records.
    let mut second_args = args.clone();
    second_args["request_id"] = json!("creation-17-b");
    let second = client
        .prompt(&sid, json!({"name":"work_create","arguments":second_args}))
        .await;
    let second = &second["call"]["result"]["structuredContent"];
    assert_ne!(second["id"], first["id"]);
    assert!(second["id"].is_string());
    // Another delivery of this exact source can establish another responsibility.
    // Querying the source must union all origins, not only its preferred receipt.
    let third_args = json!({"title":"Check another aspect","goal":"Separate authorized responsibility","source_input_id":"input-18","request_id":"creation-18"});
    let (r,updates)=client.request(60,"session/prompt",json!({"sessionId":sid,"prompt":[{"type":"text","text":format!("CONTEXT_TOOL {}",json!({"name":"work_create","arguments":third_args}))}],"_meta":{delivery::META:{"version":1,"inputId":"input-18","sources":[source]}}})).await;
    assert!(r["error"].is_null(), "{r}");
    let detail: Value = serde_json::from_str(
        updates.last().unwrap()["params"]["update"]["content"]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(detail["orientation"].as_str().unwrap().contains("input-18"));
    assert_eq!(detail["call"]["result"]["isError"], false, "{detail}");
    assert!(detail["call"]["result"]["structuredContent"]["id"].is_string());
    let amended=client.prompt(&sid,json!({"name":"work_command","arguments":{"id":first["id"],"expected_revision":2,"command":{"type":"amend","title":"Check updated proposal","goal":"Keep the same responsibility"}}})).await;
    assert_eq!(
        amended["call"]["result"]["structuredContent"]["revision"],
        3
    );
    let stopped=client.prompt(&sid,json!({"name":"work_command","arguments":{"id":first["id"],"expected_revision":3,"command":{"type":"cancel","reason":"User stopped this responsibility"}}})).await;
    assert_eq!(
        stopped["call"]["result"]["structuredContent"]["state"],
        "cancelled"
    );
    // Force the original delivery's retained uncertainty; a work control must
    // not convert that separate fact into delivery, completion or rollback.
    delivery::observe(&f.state, "fixture", &sid, "input-17", "unknown").unwrap();
    let r = client.request(47, delivery::METHOD, query.clone()).await.0;
    let receipt = &r["result"]["receipts"][0];
    assert_eq!(receipt["work"].as_array().unwrap().len(), 3, "{r}");
    assert!(
        receipt["work"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == first["id"] && w["state"] == "cancelled")
    );
    assert_eq!(receipt["state"], "unknown");
    assert_eq!(receipt["completion"], "not_established");
    let mut wrong_revision = query.clone();
    wrong_revision["sources"][0]["revision"] = json!("r1");
    assert_eq!(
        client.request(48, delivery::METHOD, wrong_revision).await.0["result"]["receipts"],
        json!([])
    );

    // Work references survive the short receipt journal, but no delivery fact
    // is reconstructed from work acceptance.
    f.state
        .db
        .lock()
        .unwrap()
        .execute("DELETE FROM records WHERE collection='source_delivery'", [])
        .unwrap();
    let r = client.request(49, delivery::METHOD, query.clone()).await.0;
    assert_eq!(
        r["result"]["receipts"][0]["work"].as_array().unwrap().len(),
        3
    );
    assert_eq!(r["result"]["receipts"][0]["state"], "unknown");
    assert!(r["result"]["receipts"][0]["inputId"].is_null());
    let retry = client
        .prompt(&sid, json!({"name":"work_create","arguments":args}))
        .await;
    assert_eq!(
        retry["call"]["result"]["structuredContent"]["id"],
        first["id"]
    );
    assert_eq!(
        retry["call"]["result"]["structuredContent"]["state"],
        "cancelled"
    );
    assert!(delivery::register_input(&f.state,"fixture",&json!({"sessionId":sid,"_meta":{delivery::META:{"version":1,"inputId":"input-17","sources":[source]}}}),None,true).is_err());

    let mut other = f.client(false);
    other.init().await;
    let mut wrong_load = f.params(Some("other-grant-canary"));
    wrong_load["sessionId"] = json!(sid);
    assert!(other.request(50, "session/load", wrong_load).await.0["error"].is_object());
    assert!(other.request(51, delivery::METHOD, query.clone()).await.0["error"].is_object());
    let other_sid = other
        .new_session(f.params(Some("other-grant-canary")))
        .await;
    let denied = other
        .prompt(
            &other_sid,
            json!({"name":"work_get","arguments":{"id":first["id"]}}),
        )
        .await;
    assert_eq!(denied["call"]["result"]["isError"], true);
    other.close().await;
    f.source.revoked.store(true, Ordering::SeqCst);
    assert!(client.request(52, delivery::METHOD, query).await.0["error"].is_object());
    assert_eq!(f.state.list_records("work").unwrap().len(), 3);
    client.close().await;
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
async fn input_recovery_requires_loaded_current_authority_and_never_starts_work() {
    let f = Fixture::new().await;
    let mut client = f.client(false);
    client.init().await;
    let sid = client
        .new_session(f.params(Some("group-grant-canary")))
        .await;
    let mut other = f.client(false);
    other.init().await;
    for method in ["_session/input_receipts", "_session/acknowledge_inputs"] {
        let params = json!({"sessionId":sid,"inputIds":["fixture-input"]});
        let (allowed, updates) = client.request(30, method, params.clone()).await;
        assert_eq!(allowed["result"]["recoveryFixture"], method, "{allowed}");
        assert!(updates.is_empty());
        let denied = other.request(30, method, params.clone()).await.0;
        assert_eq!(denied["error"]["code"], -32602, "{denied}");
        f.source.revoked.store(true, Ordering::SeqCst);
        let denied = client.request(31, method, params.clone()).await.0;
        assert_eq!(denied["error"]["code"], -32002, "{denied}");
        f.source.revoked.store(false, Ordering::SeqCst);
        f.state
            .set_record("session_tool_access", &sid, json!({"required":true}))
            .unwrap();
        let denied = client.request(32, method, params).await.0;
        assert_eq!(denied["error"]["code"], -32002, "{denied}");
        f.state
            .db
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM records WHERE collection='session_tool_access' AND id=?1",
                [&sid],
            )
            .unwrap();
    }
    let work: i64 = f
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
    assert_eq!(
        work, 0,
        "Receipt controls must not admit a new work lifecycle"
    );
    client.close().await;
    other.close().await;
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
async fn external_read_continues_source_pages_without_resetting_or_widening_access() {
    let f = Fixture::new().await;
    f.source.history_pages.store(true, Ordering::SeqCst);
    let mut client = f.client(false);
    client.init().await;
    let sid = client
        .new_session(f.params(Some("group-grant-canary")))
        .await;
    let read = json!({"name":"context_read","arguments":{"provider_id":"web:fixture","conversation_id":"team-room","limit":1}});
    let first = client.prompt(&sid, read.clone()).await;
    let first_source = &first["call"]["result"]["structuredContent"]["source"];
    assert_eq!(first_source["messages"][0]["id"], "recent-20");
    assert!(!first.to_string().contains("Older source-only marker"));
    let first_request = f
        .source
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|q| q["operation"] == "read")
        .unwrap()
        .clone();
    assert_eq!(
        first_request,
        json!({"operation":"read","access":{"mode":"grant","grant":"group-grant-canary"},"conversation_id":"team-room","thread_id":null,"limit":1})
    );

    let mut older = read.clone();
    older["arguments"]["before"] = first_source["next_cursor"].clone();
    let second = client.prompt(&sid, older.clone()).await;
    let source = &second["call"]["result"]["structuredContent"]["source"];
    assert_eq!(source["conversation_id"], "team-room");
    assert_eq!(
        source["messages"][0],
        json!({"id":"older-10","seq":10,"text":"Older source-only marker","actor":"alex","at":1,"url":"https://source.example/message/older-10"})
    );
    assert_eq!(source["complete"], false);
    assert_eq!(source["coverage"], "Bounded source messages");
    assert!(source["next_cursor"].is_null());
    let reads = || {
        f.source
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|q| q["operation"] == "read")
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(reads().len(), 2);
    assert_eq!(reads()[1]["before"], first_source["next_cursor"]);

    for (provider, conversation) in [("slack:fixture", "team-room"), ("web:fixture", "owner-dm")] {
        let mut wrong = older.clone();
        wrong["arguments"]["provider_id"] = json!(provider);
        wrong["arguments"]["conversation_id"] = json!(conversation);
        let denied = client.prompt(&sid, wrong).await;
        assert_eq!(denied["call"]["result"]["isError"], true);
    }
    assert_eq!(reads().len(), 2, "A cursor cannot widen the source grant");
    for before in [
        json!(-1),
        json!("20"),
        Value::Null,
        json!(i64::MAX as u64 + 1),
    ] {
        let mut invalid = older.clone();
        invalid["arguments"]["before"] = before;
        let denied = client.prompt(&sid, invalid).await;
        assert_eq!(denied["call"]["result"]["isError"], true);
    }
    assert_eq!(
        reads().len(),
        2,
        "Invalid positions must not become first-page reads"
    );
    let mut rejected = older.clone();
    rejected["arguments"]["before"] = json!(21);
    let denied = client.prompt(&sid, rejected).await;
    assert_eq!(denied["call"]["result"]["isError"], true);
    assert_eq!(
        reads().len(),
        3,
        "Source rejection must not retry without before"
    );
    assert_eq!(reads()[2]["before"], 21);

    for membership_removed in [false, true] {
        f.source
            .revoked
            .store(!membership_removed, Ordering::SeqCst);
        f.source.member.store(!membership_removed, Ordering::SeqCst);
        let denied = client.request(20, "session/prompt", json!({"sessionId":sid,"prompt":[{"type":"text","text":format!("CONTEXT_TOOL {}", older)}]})).await.0;
        assert!(
            denied["error"].is_object(),
            "Each page requires current source authority"
        );
        assert_eq!(reads().len(), 3);
    }
    client.close().await;
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

fn enable_rebind_attention(f: &Fixture) {
    f.state.config.write().unwrap().config.attention = Some(agent_instance::attention::Config {
        endpoint: "http://127.0.0.1:9/".into(),
        model: "fixture".into(),
        protocol: "system_one".into(),
        auth: "bearer".into(),
        credential_secret: "context-source".into(),
        decisions_per_hour: 40,
        turns_per_hour: 1,
        uncertain_turns_per_hour: 1,
    });
}
fn rebind_event(sid: &str, id: &str) -> Value {
    json!({"sessionId":sid,"occurredAt":agent_instance::state::now_ms(),"context":{"conversation_id":"team-room","agent":{"id":"a","name":"Agent"},"participants":[{"id":"a","name":"Agent"},{"id":"b","name":"Human"}],"messages":[{"id":id,"sender":"b","text":"Updated request"}],"trigger_id":id}})
}
async fn restored_offer(f: &Fixture, c: &mut Client, sid: &str, id: &str) -> (Value, Value) {
    use agent_instance::{attention, source_delivery};
    let offer = c
        .request(80, attention::METHOD, rebind_event(sid, id))
        .await
        .0["result"]
        .clone();
    assert_eq!(offer["action"], "engage");
    let input = json!({"sessionId":sid,"prompt":[{"type":"text","text":"Update"}],"_meta":{"steering":{"inputId":id,"idleBehavior":"promptRequired"},attention::META:{"id":offer["id"],"ticket":offer["ticket"]},source_delivery::META:{"version":1,"sources":[{"id":id,"revision":"original-revision","occurredAt":agent_instance::state::now_ms()}]}}});
    let response = c.request(81, "_session/steering", input.clone()).await.0;
    assert_eq!(response["result"]["outcome"], "promptRequired");
    assert_eq!(
        response["result"]["_meta"][attention::META]["ticketState"],
        "restored"
    );
    assert!(
        f.state
            .get_record("context_bindings", sid)
            .unwrap()
            .is_some()
    );
    (offer, input)
}

#[tokio::test]
async fn restored_offer_survives_real_transport_handoff_once_with_unchanged_budget() {
    use agent_instance::attention;
    let f = Fixture::new().await;
    enable_rebind_attention(&f);
    let mut first = f.client(false);
    first.init().await;
    let sid = first
        .new_session(f.params(Some("group-grant-canary")))
        .await;
    let (offer, input) = restored_offer(&f, &mut first, &sid, "cross-transport").await;
    let original_source = f
        .state
        .list_records("source_delivery")
        .unwrap()
        .into_iter()
        .find(|row| row["inputId"] == "cross-transport")
        .unwrap();
    assert_eq!(original_source["state"], "promptRequired");
    assert_eq!(original_source["binding"]["actor_id"], "alex");
    let key = offer["id"].as_str().unwrap();
    let before = f
        .state
        .get_record("attention_events", key)
        .unwrap()
        .unwrap();
    first.close().await;
    let detached = f
        .state
        .get_record("attention_events", key)
        .unwrap()
        .unwrap();
    assert_eq!(detached["status"], "offered");
    assert_eq!(detached["disconnected_unapplied"], true);
    // Knowing a session ID on an unbound connection cannot revoke its handoff.
    let mut unbound = f.client(false);
    unbound.init().await;
    assert!(
        unbound
            .request(82, "session/cancel", json!({"sessionId":sid}))
            .await
            .0["error"]
            .is_object()
    );
    unbound.close().await;
    assert_eq!(
        f.state
            .get_record("attention_events", key)
            .unwrap()
            .unwrap()["status"],
        "offered"
    );
    let mut second = f.client(false);
    second.init().await;
    // A different speaker in the same verified group does not change audience.
    let mut load = f.params(Some("group-robin-grant-canary"));
    load["sessionId"] = json!(sid);
    assert!(second.request(82, "session/load", load).await.0["error"].is_null());
    assert_eq!(
        second.request(83, "session/prompt", input.clone()).await.0["result"]["stopReason"],
        "end_turn"
    );
    let promoted_source = f
        .state
        .list_records("source_delivery")
        .unwrap()
        .into_iter()
        .find(|row| row["inputId"] == "cross-transport")
        .unwrap();
    assert_eq!(
        promoted_source["binding"], original_source["binding"],
        "Promotion keeps the original participant attribution"
    );
    assert_eq!(promoted_source["sources"], original_source["sources"]);
    assert_eq!(promoted_source["kind"], "prompt");
    let recovered_source = second.request(87, agent_instance::source_delivery::METHOD,
        json!({"sessionId":sid,"sources":input["_meta"][agent_instance::source_delivery::META]["sources"]})).await.0;
    assert_eq!(
        recovered_source["result"]["receipts"][0]["inputId"],
        "cross-transport"
    );
    let admitted = f
        .state
        .get_record("attention_events", key)
        .unwrap()
        .unwrap();
    assert_ne!(admitted["connection"], before["connection"]);
    for field in [
        "claimed_at",
        "expires_at",
        "occurred_at",
        "requested_at",
        "ticket_hash",
        "binding_sha256",
        "evidence_sha256",
    ] {
        assert_eq!(admitted[field], before[field], "{field}");
    }
    assert_eq!(admitted["disconnected_unapplied"], false);
    assert_eq!(
        second.request(84, "session/prompt", input.clone()).await.0["error"]["code"],
        -32002
    );
    let other = second
        .request(85, attention::METHOD, rebind_event(&sid, "another-event"))
        .await
        .0["result"]
        .clone();
    let mut next = input;
    next["_meta"][attention::META] = json!({"id":other["id"],"ticket":other["ticket"]});
    assert_eq!(
        second.request(86, "session/prompt", next).await.0["error"]["code"],
        -32002,
        "Rebind must not renew capacity"
    );
    assert_eq!(
        f.state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM events WHERE kind='acp.prompt.started'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    second.close().await;
}

#[tokio::test]
async fn source_promotion_keeps_uncertain_and_private_handoffs_closed() {
    use agent_instance::source_delivery as delivery;
    let f = Fixture::new().await;
    for case in ["unknown", "incorporated", "other-room", "owner-actor"] {
        let owner = case == "owner-actor";
        let origin = json!({"owner":owner,"provider_id":"web:fixture","conversation_id":"team-room","actor_id":"alex","thread_id":null});
        let mut current = origin.clone();
        current["actor_id"] = json!("robin");
        if case == "other-room" {
            current["conversation_id"] = json!("other-room");
        }
        let params = json!({"sessionId":"source-test","_meta":{"steering":{"inputId":case},delivery::META:{"version":1,"sources":[{"id":case,"revision":"r1","occurredAt":agent_instance::state::now_ms()}]}}});
        delivery::register_input(&f.state, "fixture", &params, Some(&origin), false).unwrap();
        delivery::observe(
            &f.state,
            "fixture",
            "source-test",
            case,
            if case == "unknown" {
                "unknown"
            } else if case == "incorporated" {
                "incorporated"
            } else {
                "promptRequired"
            },
        )
        .unwrap();
        let before = f.state.list_records("source_delivery").unwrap();
        assert!(
            delivery::register_input(&f.state, "fixture", &params, Some(&current), true).is_err(),
            "{case}"
        );
        assert_eq!(
            f.state.list_records("source_delivery").unwrap(),
            before,
            "{case}"
        );
    }
}

#[tokio::test]
async fn restored_handoff_rejects_changed_identity_source_expiry_policy_and_cancellation() {
    use agent_instance::attention;
    for case in [
        "principal",
        "session",
        "provider",
        "audience",
        "missing-grant",
        "revoked-grant",
        "expired",
        "policy",
        "source-edit",
        "cancel-before",
        "cancel-after",
    ] {
        let f = Fixture::new().await;
        enable_rebind_attention(&f);
        let mut first = f.client(false);
        first.init().await;
        let sid = first
            .new_session(f.params(Some("group-grant-canary")))
            .await;
        let (offer, mut input) = restored_offer(&f, &mut first, &sid, case).await;
        let key = offer["id"].as_str().unwrap();
        let before = f
            .state
            .get_record("attention_events", key)
            .unwrap()
            .unwrap();
        if case == "cancel-before" {
            first
                .request(82, "session/cancel", json!({"sessionId":sid}))
                .await;
        }
        if case == "source-edit" {
            let mut event = rebind_event(&sid, case);
            event["context"]["messages"][0]["text"] = json!("Changed source");
            assert_eq!(
                first.request(83, attention::METHOD, event).await.0["result"]["reason"],
                "duplicate"
            );
        }
        first.close().await;
        if case == "expired" {
            let mut row = f
                .state
                .get_record("attention_events", key)
                .unwrap()
                .unwrap();
            row["expires_at"] = json!(0);
            f.state.set_record("attention_events", key, row).unwrap();
        }
        if case == "policy" {
            f.state.config.write().unwrap().config.purpose = "Changed purpose".into();
        }
        if case == "revoked-grant" {
            f.source.revoked.store(true, Ordering::SeqCst);
        }
        let mut second = if case == "principal" {
            let (tx, rx_in) = mpsc::channel(32);
            let (tx_out, rx) = mpsc::channel(32);
            let state = f.state.clone();
            let headers = f.headers(false);
            let task = tokio::spawn(async move {
                acp::relay_authenticated(
                    state,
                    "different-principal".into(),
                    headers,
                    rx_in,
                    tx_out,
                )
                .await
            });
            Client { tx, rx, task }
        } else {
            f.client(false)
        };
        second.init().await;
        let mut load = f.params(if case == "missing-grant" {
            None
        } else if case == "audience" {
            Some("other-grant-canary")
        } else {
            Some("group-grant-canary")
        });
        if case == "provider" {
            load["_meta"][context::META]["provider_id"] = json!("slack:fixture");
        }
        load["sessionId"] = json!(sid);
        let loaded = if case == "session" {
            input["sessionId"] = json!(
                second
                    .new_session(f.params(Some("group-grant-canary")))
                    .await
            );
            true
        } else {
            second.request(84, "session/load", load).await.0["error"].is_null()
        };
        if loaded {
            if case == "cancel-after" {
                second
                    .request(85, "session/cancel", json!({"sessionId":sid}))
                    .await;
            }
            assert!(
                second.request(86, "session/prompt", input).await.0["error"].is_object(),
                "{case}"
            );
        } else {
            assert!(
                ["provider", "audience", "missing-grant", "revoked-grant"].contains(&case),
                "{case}"
            );
        }
        let after = f
            .state
            .get_record("attention_events", key)
            .unwrap()
            .unwrap();
        assert_eq!(after["connection"], before["connection"], "{case}");
        assert_eq!(after["claimed_at"], before["claimed_at"], "{case}");
        second.close().await;
    }
}

#[tokio::test]
async fn ordinary_unused_offer_is_still_cancelled_on_transport_disconnect() {
    use agent_instance::attention;
    let f = Fixture::new().await;
    enable_rebind_attention(&f);
    let mut first = f.client(false);
    first.init().await;
    let sid = first
        .new_session(f.params(Some("group-grant-canary")))
        .await;
    let offer = first
        .request(80, attention::METHOD, rebind_event(&sid, "ordinary"))
        .await
        .0["result"]
        .clone();
    assert_eq!(offer["action"], "engage");
    first.close().await;
    let row = f
        .state
        .get_record("attention_events", offer["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(row["status"], "cancelled");
    assert_ne!(row["disconnected_unapplied"], true);
}

#[tokio::test]
async fn restored_ticket_can_be_rebound_to_only_one_concurrent_transport() {
    let f = Fixture::new().await;
    enable_rebind_attention(&f);
    let mut first = f.client(false);
    first.init().await;
    let sid = first
        .new_session(f.params(Some("group-grant-canary")))
        .await;
    let (_, input) = restored_offer(&f, &mut first, &sid, "atomic-rebind").await;
    first.close().await;
    let mut second = f.client(false);
    second.init().await;
    let mut third = f.client(false);
    third.init().await;
    let mut load = f.params(Some("group-grant-canary"));
    load["sessionId"] = json!(sid);
    assert!(second.request(82, "session/load", load.clone()).await.0["error"].is_null());
    assert!(third.request(82, "session/load", load).await.0["error"].is_null());
    let (a, b) = tokio::join!(
        second.request(83, "session/prompt", input.clone()),
        third.request(83, "session/prompt", input)
    );
    assert_eq!(
        [&a.0, &b.0]
            .iter()
            .filter(|v| v["result"]["stopReason"] == "end_turn")
            .count(),
        1
    );
    assert_eq!(
        [&a.0, &b.0]
            .iter()
            .filter(|v| v["error"]["code"] == -32002)
            .count(),
        1
    );
    second.close().await;
    third.close().await;
}

#[tokio::test]
async fn restored_ticket_without_verified_source_cannot_survive_transport_close() {
    use agent_instance::attention;
    let f = Fixture::new().await;
    enable_rebind_attention(&f);
    let mut first = f.client(false);
    first.init().await;
    let sid = first.new_session(f.params(None)).await;
    let offer = first
        .request(80, attention::METHOD, rebind_event(&sid, "unbound"))
        .await
        .0["result"]
        .clone();
    let input = json!({"sessionId":sid,"prompt":[{"type":"text","text":"Update"}],"_meta":{"steering":{"inputId":"unbound","idleBehavior":"promptRequired"},attention::META:{"id":offer["id"],"ticket":offer["ticket"]}}});
    let response = first.request(81, "_session/steering", input).await.0;
    assert_eq!(
        response["result"]["_meta"][attention::META]["ticketState"],
        "restored"
    );
    first.close().await;
    let row = f
        .state
        .get_record("attention_events", offer["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(row["status"], "cancelled");
    assert_ne!(row["disconnected_unapplied"], true);
}

#[tokio::test]
async fn disabled_provider_blocks_existing_grants_and_owner_context_without_deleting_identity() {
    let f = Fixture::new().await;
    let mut client = f.client(false);
    client.init().await;
    let sid = client
        .new_session(f.params(Some("group-grant-canary")))
        .await;
    let original = f.state.get_record("context_bindings", &sid).unwrap();
    let mut registry = context::registry(&f.state).unwrap();
    registry.providers[0].enabled = false;
    write_private(
        &f.state.home.join("context-providers.json"),
        &serde_json::to_vec(&registry).unwrap(),
    )
    .unwrap();
    let seen = f.source.seen.lock().unwrap().len();
    let denied = client
        .request(
            50,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"retrieve context"}]}),
        )
        .await;
    assert!(denied.0["error"].is_object());
    assert_eq!(f.source.seen.lock().unwrap().len(), seen);
    assert_eq!(
        f.state.get_record("context_bindings", &sid).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read_to_string(f.state.home.join("secrets/context-source")).unwrap(),
        "provider-secret-canary"
    );
    let mut owner = f.client(true);
    owner.init().await;
    let owner_sid = owner.new_session(f.params(None)).await;
    let response = owner
        .prompt(&owner_sid, json!({"name":"context_list","arguments":{}}))
        .await;
    let items = response["call"]["result"]["structuredContent"]["items"]
        .as_array()
        .unwrap();
    assert_eq!(items.len(), 2); // only the other enabled provider
    client.close().await;
    owner.close().await;
}

#[tokio::test]
async fn conditional_provider_update_preserves_other_admin_edits_and_rejects_stale_target() {
    let f = Fixture::new().await;
    let client = reqwest::Client::new();
    let base = f.state.instance_config().server.public_url.unwrap();
    let url = format!("{base}/api/v1/context/providers");
    let mut registry = serde_json::to_value(context::registry(&f.state).unwrap()).unwrap();
    registry["providers"][0]["enabled"] = json!(false);
    assert!(
        client
            .put(&url)
            .bearer_auth(&f.owner)
            .json(&registry)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let expected = registry["providers"][0].clone();
    // A different admin disables B and adds C after setup captured A. The
    // targeted update must preserve both changes instead of writing old A/B.
    registry["providers"][1]["enabled"] = json!(false);
    registry["providers"].as_array_mut().unwrap().push(json!({"id":"new-provider","label":"New","url":"http://127.0.0.1:9","secret_ref":"new","enabled":false}));
    assert!(
        client
            .put(&url)
            .bearer_auth(&f.owner)
            .json(&registry)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let patch = json!({"expected_provider":expected,"enabled":true});
    let endpoint = format!("{url}/web:fixture");
    let denied = client
        .patch(&endpoint)
        .bearer_auth(&f.acp_only)
        .json(&patch)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        serde_json::to_value(context::registry(&f.state).unwrap()).unwrap(),
        registry
    );
    let response = client
        .patch(&endpoint)
        .bearer_auth(&f.owner)
        .json(&patch)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value = response.json().await.unwrap();
    assert_eq!(result["enabled"], true);
    let current = serde_json::to_value(context::registry(&f.state).unwrap()).unwrap();
    registry["providers"][0]["enabled"] = json!(true);
    assert_eq!(current, registry);
    // A lost successful response cannot make the same stale precondition write again.
    let stale = client
        .patch(&endpoint)
        .bearer_auth(&f.owner)
        .json(&patch)
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(
        serde_json::to_value(context::registry(&f.state).unwrap()).unwrap(),
        current
    );
    let wrong = client
        .patch(format!("{url}/different"))
        .bearer_auth(&f.owner)
        .json(&patch)
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);
    let mut missing = patch.clone();
    missing["expected_provider"]["id"] = json!("absent");
    let missing = client
        .patch(format!("{url}/absent"))
        .bearer_auth(&f.owner)
        .json(&missing)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::CONFLICT);
    assert_eq!(
        serde_json::to_value(context::registry(&f.state).unwrap()).unwrap(),
        current
    );
    assert!(
        f.source.seen.lock().unwrap().is_empty(),
        "Metadata update must not call providers"
    );
    let descriptor: Value = client
        .get(format!("{base}/api/v1/descriptor"))
        .bearer_auth(&f.owner)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        descriptor["context_provider_mutation"],
        json!({"version":1,"method":"PATCH","path":"api/v1/context/providers/{id}","precondition":"expected_provider"})
    );
}
