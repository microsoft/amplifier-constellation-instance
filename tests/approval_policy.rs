use agent_instance::{
    acp, auth,
    config::{BackendConfig, ConfigDocument, InstanceConfig, private_dir, write_private},
    server,
    state::{AppState, now_ms},
};
use axum::{
    body::{Body, to_bytes},
    http::{HeaderMap, Request},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tower::ServiceExt;
struct Fixture {
    dir: tempfile::TempDir,
    state: Arc<AppState>,
    admin: String,
    client: String,
    client_id: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        private_dir(&dir.path().join("work")).unwrap();
        let mut c = InstanceConfig::blank("Approval fixture".into(), "fixture-owner".into());
        c.setup_complete = true;
        c.enabled = true;
        c.backend = Some(BackendConfig {
            command: agent_instance::platform::python().into(),
            args: vec![format!(
                "{}/tests/fixtures/approval_backend.py",
                env!("CARGO_MANIFEST_DIR")
            )],
            env: BTreeMap::new(),
            secret_env: BTreeMap::new(),
            working_directory: "${AGENT_HOME}/work".into(),
            allowed_working_roots: vec!["${AGENT_HOME}/work".into()],
        });
        write_private(
            &dir.path().join("config.json"),
            &serde_json::to_vec(&ConfigDocument {
                revision: 1,
                config: c,
            })
            .unwrap(),
        )
        .unwrap();
        let state = Arc::new(AppState::open(dir.path().into()).unwrap());
        let admin = auth::issue_token(&state, "Owner", &["admin".into()], 3600).unwrap();
        let client = auth::issue_token(&state, "Slack", &["acp".into()], 3600).unwrap();
        Self {
            dir,
            state,
            admin: admin["token"].as_str().unwrap().into(),
            client: client["token"].as_str().unwrap().into(),
            client_id: client["id"].as_str().unwrap().into(),
        }
    }
    async fn api(&self, method: &str, path: &str, body: Value, token: &str) -> (u16, Value) {
        let req = Request::builder()
            .method(method)
            .uri(format!("/api/v1/{path}"))
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let r = server::router(self.state.clone())
            .oneshot(req)
            .await
            .unwrap();
        let status = r.status().as_u16();
        let bytes = to_bytes(r.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    async fn policy(&self) -> Value {
        self.api("GET", "approval-policy", Value::Null, &self.admin)
            .await
            .1
    }
    async fn configure(&self, mode: &str, delegate: bool) -> Value {
        let p = self.policy().await;
        let body = json!({"expected_revision":p["revision"],"mode":mode,"delegated_clients":if delegate{vec![self.client_id.clone()]}else{vec![]}});
        let (status, p) = self.api("PUT", "approval-policy", body, &self.admin).await;
        assert_eq!(status, 200, "{p}");
        p
    }
    async fn connect(&self) -> Client {
        let (tx, rx) = mpsc::channel(32);
        let (outgoing, mut incoming) = mpsc::channel(32);
        let state = self.state.clone();
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            format!("Bearer {}", self.client).parse().unwrap(),
        );
        let task = tokio::spawn(async move {
            acp::relay_authenticated(state, "spoofed-user-metadata".into(), headers, rx, outgoing)
                .await
        });
        tx.send(json!({"jsonrpc":"2.0","id":100,"method":"initialize","params":{"protocolVersion":1,"_meta":{"com.microsoft.amplifier.approval-policy":{"owner":"engine","instanceId":"forged"}}}})).await.unwrap();
        let init = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(init.get("result").is_some(), "{init}");
        Client {
            tx,
            rx: incoming,
            task,
        }
    }
}
struct Client {
    tx: mpsc::Sender<Value>,
    rx: mpsc::Receiver<Value>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}
impl Client {
    async fn recv(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(6), self.rx.recv())
            .await
            .expect("fixture timeout")
            .expect("closed")
    }
    async fn session(&mut self, f: &Fixture) -> String {
        self.tx.send(json!({"jsonrpc":"2.0","id":101,"method":"session/new","params":{"cwd":f.dir.path().join("work"),"mcpServers":[]}})).await.unwrap();
        self.recv().await["result"]["sessionId"]
            .as_str()
            .unwrap()
            .into()
    }
    async fn prompt(&self, sid: &str, spec: Value) {
        self.tx.send(json!({"jsonrpc":"2.0","id":102,"method":"session/prompt","params":{"sessionId":sid,"prompt":[{"type":"text","text":spec.to_string()}]}})).await.unwrap();
    }
    async fn answer(&self, request: &Value, kind: &str) {
        let option = request["params"]["options"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["kind"] == kind)
            .unwrap();
        self.tx.send(json!({"jsonrpc":"2.0","id":request["id"],"result":{"outcome":{"outcome":"selected","optionId":option["optionId"]}}})).await.unwrap();
    }
    async fn close(self) {
        drop(self.tx);
        assert!(
            tokio::time::timeout(Duration::from_secs(5), self.task)
                .await
                .unwrap()
                .unwrap()
                .is_ok()
        );
    }
}
fn kinds(v: &Value) -> Vec<&str> {
    v["params"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["kind"].as_str())
        .collect()
}
#[tokio::test]
async fn owner_api_defaults_cas_delegation_and_no_secret_projection() {
    let f = Fixture::new();
    let p = f.policy().await;
    assert_eq!(p["mode"], "ask");
    assert_eq!(p["revision"], 0);
    assert_eq!(p["delegated_clients"], json!([]));
    assert_eq!(
        f.api("GET", "approval-policy", Value::Null, &f.client)
            .await
            .0,
        403
    );
    let p = f.configure("automatic", true).await;
    assert_eq!(p["revision"], 1);
    assert_eq!(
        f.api(
            "PUT",
            "approval-policy",
            json!({"expected_revision":0,"mode":"ask","delegated_clients":[]}),
            &f.admin
        )
        .await
        .0,
        409
    );
    assert_eq!(
        f.api(
            "PUT",
            "approval-policy",
            json!({"expected_revision":1,"mode":"ask","delegated_clients":["absent"]}),
            &f.admin
        )
        .await
        .0,
        400
    );
    assert!(!p.to_string().contains(&f.client));
    assert!(!p.to_string().contains(&auth::digest(&f.client)));
}
#[tokio::test]
async fn remembers_across_chats_and_restart_then_revoke_asks_again() {
    let mut f = Fixture::new();
    f.configure("ask", true).await;
    let mut c = f.connect().await;
    let a = c.session(&f).await;
    c.prompt(&a, json!({})).await;
    let request = c.recv().await;
    assert!(kinds(&request).contains(&"allow_always"));
    c.answer(&request, "allow_always").await;
    let result = c.recv().await;
    assert_eq!(result["result"]["decision"]["optionId"], "permit");
    assert_eq!(result["result"]["initProfile"]["owner"], "instance");
    assert_eq!(
        result["result"]["initProfile"]["instanceId"],
        f.state.instance_config().instance_id
    );
    let b = c.session(&f).await;
    assert_ne!(a, b);
    c.prompt(&b, json!({})).await;
    assert_eq!(c.recv().await["result"]["decision"]["optionId"], "permit");
    c.close().await;
    f.state = Arc::new(AppState::open(f.dir.path().into()).unwrap());
    let mut c = f.connect().await;
    let sid = c.session(&f).await;
    c.prompt(&sid, json!({})).await;
    assert_eq!(c.recv().await["result"]["decision"]["optionId"], "permit");
    let p = f.policy().await;
    assert_eq!(p["rules"].as_array().unwrap().len(), 1);
    assert_eq!(p["rules"][0]["creator"], format!("token:{}", f.client_id));
    assert!(!p.to_string().contains("raw-arguments-canary"));
    let result = f
        .api(
            "POST",
            &format!(
                "approval-policy/rules/{}/revoke",
                p["rules"][0]["id"].as_str().unwrap()
            ),
            json!({"expected_revision":p["revision"]}),
            &f.admin,
        )
        .await;
    assert_eq!(result.0, 200);
    assert_eq!(result.1["rules"][0]["usable"], false);
    c.prompt(&sid, json!({})).await;
    let request = c.recv().await;
    assert_eq!(request["method"], "session/request_permission");
    c.answer(&request, "reject_once").await;
    c.recv().await;
    c.close().await;
}
#[tokio::test]
async fn default_once_does_not_remember_and_automatic_is_explicit() {
    let f = Fixture::new();
    let mut c = f.connect().await;
    let sid = c.session(&f).await;
    for _ in 0..2 {
        c.prompt(&sid, json!({})).await;
        let r = c.recv().await;
        assert!(!kinds(&r).contains(&"allow_always"));
        c.answer(&r, "allow_once").await;
        assert_eq!(c.recv().await["result"]["decision"]["optionId"], "permit");
    }
    assert_eq!(f.policy().await["rules"], json!([]));
    f.configure("automatic", false).await;
    c.prompt(&sid, json!({})).await;
    assert_eq!(c.recv().await["result"]["decision"]["optionId"], "permit");
    c.close().await;
}
#[tokio::test]
async fn changed_semantics_producer_and_backend_do_not_reuse_rules() {
    let f = Fixture::new();
    f.configure("ask", true).await;
    let mut c = f.connect().await;
    let sid = c.session(&f).await;
    c.prompt(&sid, json!({})).await;
    let r = c.recv().await;
    c.answer(&r, "allow_always").await;
    c.recv().await;
    for spec in [
        json!({"key":"b".repeat(64)}),
        json!({"producer":"other/1"}),
        json!({"lifetime":"conversation"}),
        json!({"descriptor":{"allowOptionId":"refuse"}}),
    ] {
        c.prompt(&sid, spec).await;
        let r = c.recv().await;
        assert_eq!(r["method"], "session/request_permission");
        c.answer(&r, "reject_once").await;
        c.recv().await;
    }
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
        .insert("FIXTURE_ENDPOINT".into(), "different-endpoint".into());
    let mut c = f.connect().await;
    let sid = c.session(&f).await;
    c.prompt(&sid, json!({})).await;
    let r = c.recv().await;
    assert_eq!(r["method"], "session/request_permission");
    c.answer(&r, "reject_once").await;
    c.recv().await;
    c.close().await;
}
#[tokio::test]
async fn stale_revision_cancel_and_duplicate_answers_never_remember() {
    let f = Fixture::new();
    f.configure("ask", true).await;
    let mut c = f.connect().await;
    let sid = c.session(&f).await;
    c.prompt(&sid, json!({})).await;
    let old = c.recv().await;
    f.configure("ask", true).await;
    c.answer(&old, "allow_always").await;
    assert_eq!(c.recv().await["result"]["decision"]["outcome"], "cancelled");
    assert_eq!(f.policy().await["rules"], json!([]));
    c.prompt(&sid, json!({})).await;
    let r = c.recv().await;
    assert_ne!(r["id"], old["id"]);
    c.answer(&old, "allow_always").await;
    c.tx.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":sid}}))
        .await
        .unwrap();
    let result = c.recv().await;
    assert_eq!(result["id"], 102);
    c.answer(&r, "allow_always").await;
    assert_eq!(f.policy().await["rules"], json!([]));
    c.close().await;
}
#[tokio::test]
async fn revoked_or_expired_issuer_and_removed_delegation_invalidate_matches() {
    for action in ["expiry", "revocation", "delegation"] {
        let f = Fixture::new();
        f.configure("ask", true).await;
        let mut c = f.connect().await;
        let sid = c.session(&f).await;
        c.prompt(&sid, json!({})).await;
        let r = c.recv().await;
        c.answer(&r, "allow_always").await;
        c.recv().await;
        c.close().await;
        match action {
            "expiry" => {
                f.state
                    .db
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE tokens SET expires_at=?1 WHERE id=?2",
                        rusqlite::params![now_ms() - 1, f.client_id],
                    )
                    .unwrap();
            }
            "revocation" => {
                f.state
                    .db
                    .lock()
                    .unwrap()
                    .execute("DELETE FROM tokens WHERE id=?1", [&f.client_id])
                    .unwrap();
            }
            _ => {
                f.configure("ask", false).await;
            }
        }
        assert_eq!(f.policy().await["rules"][0]["usable"], false);
        let new = auth::issue_token(&f.state, "Other ACP", &["acp".into()], 3600).unwrap();
        let f = Fixture {
            client: new["token"].as_str().unwrap().into(),
            ..f
        };
        let mut c = f.connect().await;
        let sid = c.session(&f).await;
        c.prompt(&sid, json!({})).await;
        let r = c.recv().await;
        assert_eq!(r["method"], "session/request_permission");
        c.answer(&r, "reject_once").await;
        c.recv().await;
        c.close().await;
    }
}
#[tokio::test]
async fn ambiguous_allow_is_not_automatic_and_unnegotiated_engine_cannot_remember() {
    let f = Fixture::new();
    f.configure("automatic", true).await;
    let mut c = f.connect().await;
    let sid = c.session(&f).await;
    c.prompt(&sid, json!({"ambiguous":true})).await;
    let r = c.recv().await;
    assert!(!kinds(&r).contains(&"allow_always"));
    c.answer(&r, "reject_once").await;
    c.recv().await;
    c.close().await;
    f.configure("ask", true).await;
    f.state
        .config
        .write()
        .unwrap()
        .config
        .backend
        .as_mut()
        .unwrap()
        .env
        .insert("APPROVAL_PRODUCER".into(), "none".into());
    let mut c = f.connect().await;
    let sid = c.session(&f).await;
    c.prompt(&sid, json!({})).await;
    let r = c.recv().await;
    assert!(!kinds(&r).contains(&"allow_always"));
    c.answer(&r, "allow_once").await;
    c.recv().await;
    c.close().await;
}

#[tokio::test]
async fn original_backend_id_cannot_bypass_remapped_revision_or_cancel() {
    let f = Fixture::new();
    f.configure("ask", true).await;
    let mut c = f.connect().await;
    let sid = c.session(&f).await;
    c.prompt(&sid, json!({})).await;
    let r = c.recv().await;
    assert_ne!(r["id"], 1);
    c.tx.send(json!({"jsonrpc":"2.0","id":1,"result":{"outcome":{"outcome":"selected","optionId":"permit"}}})).await.unwrap();
    // A following cancellation must still settle this pending prompt, rather than
    // finding an already-approved operation from the forged original ID.
    c.tx.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":sid}}))
        .await
        .unwrap();
    let result = c.recv().await;
    assert!(
        result["result"]["stopReason"] == "cancelled"
            || result["result"]["decision"]["outcome"] == "cancelled",
        "{result}"
    );
    assert_eq!(f.policy().await["rules"], json!([]));
    c.close().await;
}
#[tokio::test]
async fn expired_revoked_or_changed_binding_before_callback_is_not_passthrough() {
    for failure in ["expired", "revoked", "backend"] {
        let f = Fixture::new();
        let mut c = f.connect().await;
        let sid = c.session(&f).await;
        c.prompt(&sid, json!({"gate":true})).await;
        assert_eq!(
            c.recv().await["params"]["update"]["content"]["text"],
            "fixture-gated-before-permission"
        );
        match failure {
            "expired" => {
                f.state
                    .db
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE tokens SET expires_at=?1 WHERE id=?2",
                        rusqlite::params![now_ms() - 1, f.client_id],
                    )
                    .unwrap();
            }
            "revoked" => {
                f.state
                    .db
                    .lock()
                    .unwrap()
                    .execute("DELETE FROM tokens WHERE id=?1", [&f.client_id])
                    .unwrap();
            }
            _ => {
                f.state
                    .config
                    .write()
                    .unwrap()
                    .config
                    .backend
                    .as_mut()
                    .unwrap()
                    .env
                    .insert("ENDPOINT".into(), "changed".into());
            }
        }
        std::fs::write(f.dir.path().join("work/release-permission"), b"release").unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), c.task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        while let Ok(v) = c.rx.try_recv() {
            assert_ne!(v["method"], "session/request_permission");
        }
        assert_eq!(f.policy().await["rules"], json!([]));
    }
}
#[tokio::test]
async fn lost_client_and_failed_durable_write_do_not_grant_or_replay() {
    let mut f = Fixture::new();
    f.configure("ask", true).await;
    let mut c = f.connect().await;
    let sid = c.session(&f).await;
    c.prompt(&sid, json!({})).await;
    c.recv().await;
    c.close().await;
    f.state = Arc::new(AppState::open(f.dir.path().into()).unwrap());
    assert_eq!(f.policy().await["rules"], json!([]));
    let mut c = f.connect().await;
    let sid = c.session(&f).await;
    c.prompt(&sid, json!({})).await;
    let r = c.recv().await;
    f.state.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_approval_receipt BEFORE INSERT ON records WHEN NEW.collection='approval_decisions' BEGIN SELECT RAISE(ABORT, 'fixture storage failure'); END;").unwrap();
    c.answer(&r, "allow_always").await;
    assert!(
        tokio::time::timeout(Duration::from_secs(5), c.task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert_eq!(f.policy().await["rules"], json!([]));
}
#[test]
fn launch_fingerprint_uses_supplied_values_and_detects_later_rotation() {
    let f = Fixture::new();
    let mut b = f.state.instance_config().backend.unwrap();
    b.secret_env
        .insert("PROVIDER".into(), "fixture-provider".into());
    private_dir(&f.dir.path().join("secrets")).unwrap();
    write_private(&f.dir.path().join("secrets/fixture-provider"), b"before").unwrap();
    let captured = BTreeMap::from([("PROVIDER".into(), "before".into())]);
    let launch = agent_instance::approval_policy::fingerprint_with_secrets(&b, &captured).unwrap();
    assert_eq!(
        launch,
        agent_instance::approval_policy::backend_fingerprint(&f.state, &b).unwrap()
    );
    write_private(&f.dir.path().join("secrets/fixture-provider"), b"after").unwrap();
    assert_ne!(
        launch,
        agent_instance::approval_policy::backend_fingerprint(&f.state, &b).unwrap()
    );
    assert_eq!(
        launch,
        agent_instance::approval_policy::fingerprint_with_secrets(&b, &captured).unwrap()
    );
}

#[tokio::test]
async fn cancellation_before_permission_arrives_cannot_fall_back_to_manual() {
    let f = Fixture::new();
    f.configure("automatic", true).await;
    let mut c = f.connect().await;
    let sid = c.session(&f).await;
    c.prompt(&sid, json!({"gate":true})).await;
    c.recv().await;
    c.tx.send(
        json!({"jsonrpc":"2.0","id":103,"method":"session/cancel","params":{"sessionId":sid}}),
    )
    .await
    .unwrap();
    assert_eq!(c.recv().await["id"], 103);
    std::fs::write(f.dir.path().join("work/release-permission"), b"release").unwrap();
    let r = c.recv().await;
    assert_eq!(r["id"], 102);
    assert_eq!(r["result"]["decision"]["outcome"], "cancelled");
    assert_eq!(f.policy().await["rules"], json!([]));
    c.close().await;
}
#[tokio::test]
async fn revoked_capacity_can_be_reused_without_evicting_active_rules() {
    let f = Fixture::new();
    f.configure("ask", true).await;
    let mut c = f.connect().await;
    let sid = c.session(&f).await;
    c.prompt(&sid, json!({})).await;
    let r = c.recv().await;
    c.answer(&r, "allow_always").await;
    c.recv().await;
    let mut stored = f
        .state
        .get_record("approval_policy", "policy")
        .unwrap()
        .unwrap();
    let template = stored["rules"][0].clone();
    let active_id = template["id"].clone();
    for i in 1..256 {
        let mut r = template.clone();
        r["id"] = json!(format!("retained-revoked-{i}"));
        r["status"] = json!("revoked");
        stored["rules"].as_array_mut().unwrap().push(r);
    }
    f.state
        .set_record("approval_policy", "policy", stored)
        .unwrap();
    c.prompt(&sid, json!({"key":"b".repeat(64)})).await;
    let r = c.recv().await;
    assert!(kinds(&r).contains(&"allow_always"));
    c.answer(&r, "allow_always").await;
    c.recv().await;
    let p = f.policy().await;
    let rules = p["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 256);
    assert!(
        rules
            .iter()
            .any(|r| r["id"] == active_id && r["status"] == "active")
    );
    assert_eq!(rules.iter().filter(|r| r["status"] == "active").count(), 2);
    c.close().await;
}
#[tokio::test]
async fn managed_backend_change_serializes_with_durable_authorization() {
    use agent_instance::approval_policy::{Bridge, backend_fingerprint};
    let f = Fixture::new();
    f.configure("automatic", false).await;
    let backend = f.state.instance_config().backend.unwrap();
    let bridge = Bridge::from_launch(backend_fingerprint(&f.state, &backend).unwrap());
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", f.client).parse().unwrap(),
    );
    let request = json!({"jsonrpc":"2.0","id":1,"params":{"sessionId":"fixture-session","toolCall":{"toolCallId":"fixture-call"},"options":[{"optionId":"permit","kind":"allow_once"}]}});
    let offer = bridge
        .offer(&f.state, Some(&headers), &request, "fixture-prompt")
        .unwrap()
        .unwrap();
    let db = f.state.db.lock().unwrap();
    let state = f.state.clone();
    let decision = std::thread::spawn(move || offer.decide(&state, Some(&headers), None).unwrap());
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if f.state.approval_policy_guard.try_lock().is_err() {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    let state = f.state.clone();
    let mut doc = state.config.read().unwrap().clone();
    doc.config
        .backend
        .as_mut()
        .unwrap()
        .env
        .insert("CHANGED_ENDPOINT".into(), "new".into());
    let mutation = std::thread::spawn(move || {
        server::replace_config(&state, doc.revision, doc.config, "fixture-owner").unwrap()
    });
    // The decision already holds the authorization gate while the database is
    // held here. A coordinated backend change cannot overtake that decision.
    assert!(f.state.approval_policy_guard.try_lock().is_err());
    drop(db);
    assert_eq!(
        decision.join().unwrap().unwrap()["result"]["outcome"]["optionId"],
        "permit"
    );
    mutation.join().unwrap();
    assert_eq!(
        f.state.instance_config().backend.unwrap().env["CHANGED_ENDPOINT"],
        "new"
    );
}

#[tokio::test]
async fn one_policy_owner_maps_remember_to_once_and_does_not_answer_questions() {
    let f = Fixture::new();
    f.configure("ask", true).await;
    let mut c = f.connect().await;
    let sid = c.session(&f).await;
    c.prompt(&sid, json!({"engine_remember":true})).await;
    let request = c.recv().await;
    let options = request["params"]["options"].as_array().unwrap();
    assert!(!options.iter().any(|o| o["optionId"] == "engine-cache"));
    assert_eq!(
        options
            .iter()
            .filter(|o| o["kind"] == "allow_always")
            .count(),
        1
    );
    c.answer(&request, "allow_always").await;
    assert_eq!(c.recv().await["result"]["decision"]["optionId"], "permit");
    f.configure("automatic", true).await;
    c.prompt(&sid, json!({"question":true})).await;
    let request = c.recv().await;
    assert_eq!(request["method"], "_fixture/question");
    assert_eq!(request["id"], 1);
    c.answer(&request, "reject_once").await;
    c.recv().await;
    c.close().await;
}

#[tokio::test]
async fn stop_reaches_engine_without_first_resolving_permission_as_refusal() {
    let f = Fixture::new();
    f.configure("ask", true).await;
    let mut c = f.connect().await;
    let sid = c.session(&f).await;
    c.prompt(&sid, json!({"stop_order_required":true})).await;
    let request = c.recv().await;
    c.tx.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":sid}}))
        .await
        .unwrap();
    let terminal = c.recv().await;
    assert_eq!(terminal["id"], 102);
    assert_eq!(terminal["result"]["stopReason"], "cancelled", "{terminal}");
    c.answer(&request, "allow_always").await;
    c.tx.send(
        json!({"jsonrpc":"2.0","id":103,"method":"session/close","params":{"sessionId":sid}}),
    )
    .await
    .unwrap();
    assert_eq!(c.recv().await["id"], 103);
    assert_eq!(f.policy().await["rules"], json!([]));
    assert!(
        f.state
            .list_records("approval_decisions")
            .unwrap()
            .is_empty()
    );
    c.close().await;
}

#[tokio::test]
async fn backend_withdrawal_uses_frontend_id_and_retires_only_that_offer() {
    let f = Fixture::new();
    f.configure("ask", true).await;
    let mut c = f.connect().await;
    let sid = c.session(&f).await;
    c.prompt(&sid, json!({"withdraw":true})).await;
    let request = c.recv().await;
    let withdrawn = c.recv().await;
    assert_eq!(withdrawn["method"], "$/cancel_request");
    assert_eq!(
        withdrawn["params"]["requestId"], request["id"],
        "{withdrawn}"
    );
    c.answer(&request, "allow_always").await;
    c.tx.send(
        json!({"jsonrpc":"2.0","id":103,"method":"session/cancel","params":{"sessionId":sid}}),
    )
    .await
    .unwrap();
    let mut ids = std::collections::BTreeSet::new();
    for _ in 0..2 {
        let v = c.recv().await;
        ids.insert(v["id"].as_u64().unwrap());
        if v["id"] == 102 {
            assert_eq!(v["result"]["stopReason"], "cancelled");
        }
    }
    assert_eq!(ids, std::collections::BTreeSet::from([102, 103]));
    assert_eq!(f.policy().await["rules"], json!([]));
    assert!(
        f.state
            .list_records("approval_decisions")
            .unwrap()
            .is_empty()
    );
    c.close().await;
}
