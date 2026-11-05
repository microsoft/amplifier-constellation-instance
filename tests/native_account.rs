#![cfg(unix)]
use agent_instance::{
    auth,
    config::{BackendConfig, ConfigDocument, write_private},
    host, scheduler, server,
    state::AppState,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, os::unix::fs::PermissionsExt, sync::Arc};
use tower::ServiceExt;
struct Fixture {
    dir: tempfile::TempDir,
    state: Arc<AppState>,
    router: Router,
    token: String,
}
impl Fixture {
    async fn start(&self) -> Value {
        self.api("POST","native-account/login",json!({"expected_backend_sha256":scheduler::backend_hash(&self.state),"callback_port":0})).await
    }
    async fn wait(&self, phase: &str) -> Value {
        // The supervised engine has 15 seconds to emit its first line. Cold
        // Python startup on hosted Intel Macs can exceed a four-second fixture
        // deadline, so allow that real contract plus a little scheduling time.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
        while tokio::time::Instant::now() < deadline {
            let value = self.api("GET", "native-account/login", Value::Null).await;
            if value["state"] == phase {
                return value;
            }
            if value["state"] == "failed" && phase != "failed" {
                panic!("sign-in failed before reaching {phase}: {value}");
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!(
            "sign-in never reached {phase}: {}",
            self.api("GET", "native-account/login", Value::Null).await
        );
    }
    fn pid(&self) -> i32 {
        std::fs::read_to_string(self.dir.path().join("engine/oauth-pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }
}
#[tokio::test]
async fn native_oauth_keeps_pending_link_ephemeral_and_configuration_unchanged() {
    let f = Fixture::new();
    let initial = f.api("GET", "native-config", Value::Null).await;
    let started = f.start().await;
    let pending = f.wait("pending").await;
    assert_eq!(started["id"], pending["id"]);
    assert_eq!(f.start().await["id"], pending["id"]);
    assert!(
        pending["authorization_url"]
            .as_str()
            .unwrap()
            .contains("ephemeral-fixture-state")
    );
    assert!(
        !f.state
            .get_record("native_account", "login")
            .unwrap()
            .unwrap()
            .to_string()
            .contains("ephemeral-fixture-state")
    );
    let rejected=f.request("PUT","native-config",json!({"expected_revision":initial["revision"],"expected_backend_sha256":initial["backend_sha256"],"config":initial["config"]}),&f.token).await;
    assert_eq!(rejected.0, 400);
    reqwest::get(pending["callback"].as_str().unwrap())
        .await
        .unwrap();
    let done = f.wait("connected").await;
    assert_eq!(done["account"]["connected"], true);
    assert!(done.get("authorization_url").is_none());
    let account = f.api("GET", "native-account", Value::Null).await;
    assert!(!account.to_string().contains("must-never"));
    assert!(
        account["account"]["credentials_file"]
            .as_str()
            .unwrap()
            .ends_with("accounts/chatgpt/credentials.json")
    );
    assert_eq!(
        f.api("GET", "native-config", Value::Null).await["revision"],
        initial["revision"]
    );
    let catalog = f.api("POST", "native-account/models", json!({})).await;
    assert_eq!(catalog["runtime_catalog_available"], true);
    assert_eq!(catalog["models"].as_array().unwrap().len(), 1);
    assert_eq!(catalog["models"][0]["slug"], "fixture-account-model");
    assert!(catalog["models"][0].get("private").is_none());
    let events = f.api("GET", "events", Value::Null).await.to_string();
    assert!(!events.contains("ephemeral-fixture-state"));
    assert!(!events.contains("must-never"));
    assert!(!events.contains("fixture@example"));
    f.api("DELETE", "native-account", json!({"confirm":true}))
        .await;
    assert_eq!(
        f.api("GET", "native-account", Value::Null).await["account"]["connected"],
        false
    );
}
#[tokio::test]
async fn account_models_follow_installed_runtime_support_without_migrating_saved_settings() {
    let f = Fixture::new();
    let path = f.dir.path().join("engine/runtime.json");
    let original = std::fs::read(&path).unwrap();
    for (mode, count, present) in [
        ("normal", 1, true),
        ("empty", 0, true),
        ("legacy", 2, false),
    ] {
        std::fs::write(f.dir.path().join("engine/catalog-mode"), mode).unwrap();
        let value = f.api("POST", "native-account/models", json!({})).await;
        assert_eq!(value["models"].as_array().unwrap().len(), count, "{mode}");
        assert_eq!(value["runtime_catalog_available"], present, "{mode}");
        assert!(!value.to_string().contains("do not forward"));
    }
    for mode in ["missing", "malformed"] {
        std::fs::write(f.dir.path().join("engine/catalog-mode"), mode).unwrap();
        let (status, _) = f
            .request("POST", "native-account/models", json!({}), &f.token)
            .await;
        assert_eq!(status, 400, "{mode}");
    }
    assert_eq!(std::fs::read(path).unwrap(), original);
}
#[tokio::test]
async fn cancellation_expiry_and_stale_jobs_reap_the_callback_process() {
    let f = Fixture::new();
    f.start().await;
    let pending = f.wait("pending").await;
    let pid = f.pid();
    assert_eq!(
        f.request(
            "DELETE",
            "native-account/login",
            json!({"job_id":"old"}),
            &f.token
        )
        .await
        .0,
        409
    );
    f.api(
        "DELETE",
        "native-account/login",
        json!({"job_id":pending["id"]}),
    )
    .await;
    f.wait("cancelled").await;
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert!(
        !f.dir
            .path()
            .join("engine/accounts/chatgpt/credentials.json")
            .exists()
    );
    std::fs::write(f.dir.path().join("engine/oauth-mode"), "expire").unwrap();
    f.start().await;
    f.wait("pending").await;
    let pid = f.pid();
    f.wait("expired").await;
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    std::fs::write(f.dir.path().join("engine/oauth-mode"), "bad-url").unwrap();
    f.start().await;
    let failed = f.wait("failed").await;
    assert!(!failed.to_string().contains("untrusted.invalid"));
}
#[tokio::test]
async fn account_authority_and_capability_are_independent_of_ordinary_acp() {
    let f = Fixture::new();
    let spec = f.api("GET", "openapi.json", Value::Null).await;
    let paths = &spec["paths"];
    assert_eq!(
        paths["/native-account/login"]["post"]["x-required-scope"],
        "admin"
    );
    assert_eq!(
        paths["/native-account/login"]["post"]["requestBody"]["content"]["application/json"]["schema"]
            ["required"],
        json!(["expected_backend_sha256"])
    );
    assert_eq!(
        paths["/native-account/login"]["delete"]["requestBody"]["content"]["application/json"]["schema"]
            ["required"],
        json!(["job_id"])
    );
    assert_eq!(
        paths["/native-account"]["delete"]["requestBody"]["content"]["application/json"]["schema"]
            ["required"],
        json!(["confirm"])
    );
    assert!(paths["/native-account"]["get"].get("requestBody").is_none());
    let observe = auth::issue_token(
        &f.state,
        "observer",
        &["observe".into(), "ui".into(), "acp".into()],
        3600,
    )
    .unwrap();
    for (method, path, body) in [
        ("GET", "native-account", Value::Null),
        (
            "POST",
            "native-account/login",
            json!({"expected_backend_sha256":scheduler::backend_hash(&f.state)}),
        ),
        ("POST", "native-account/models", json!({})),
        ("DELETE", "native-account", json!({"confirm":true})),
    ] {
        assert_eq!(
            f.request(method, path, body, observe["token"].as_str().unwrap())
                .await
                .0,
            403
        );
    }
    assert_eq!(
        f.request(
            "POST",
            "native-account/login",
            json!({"expected_backend_sha256":"old"}),
            &f.token
        )
        .await
        .0,
        409
    );
    let descriptor = f.api("GET", "descriptor", Value::Null).await;
    assert!(
        descriptor["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("agent-instance.native-account/1"))
    );
    f.state.config.write().unwrap().config.backend = None;
    let descriptor = f.api("GET", "descriptor", Value::Null).await;
    assert!(
        !descriptor["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["path"].as_str().unwrap_or("").starts_with("native-"))
    );
}
#[tokio::test]
async fn restarted_host_reports_interruption_without_replaying_consent() {
    let f = Fixture::new();
    f.state
        .set_record(
            "native_account",
            "login",
            json!({"id":"persisted-job","state":"pending","started_at":1,"callback_port":1455}),
        )
        .unwrap();
    let value = f.api("GET", "native-account/login", Value::Null).await;
    assert_eq!(value["state"], "interrupted");
    assert!(value.get("authorization_url").is_none());
    assert!(!f.dir.path().join("engine/oauth-pid").exists());
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        host::init(
            dir.path(),
            "Native configuration".into(),
            None,
            "127.0.0.1".into(),
            0,
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("engine")).unwrap();
        std::fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/native_account.py"
            ),
            dir.path().join("engine/agent-runtime"),
        )
        .unwrap();
        std::fs::set_permissions(
            dir.path().join("engine/agent-runtime"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        write_private(&dir.path().join("engine/runtime.json"),&serde_json::to_vec(&json!({"provider":{"kind":"mock","model":"fixture","endpoint":"https://example.invalid","api_key_env":"AGENT_MODEL_API_KEY","native":{}},"mode":"ask","decisions":[]})).unwrap()).unwrap();
        write_private(
            &dir.path().join("secrets/model-key"),
            b"private-secret-canary",
        )
        .unwrap();
        let mut doc: ConfigDocument =
            serde_json::from_slice(&std::fs::read(dir.path().join("config.json")).unwrap())
                .unwrap();
        doc.config.backend = Some(BackendConfig {
            command: "${AGENT_HOME}/engine/agent-runtime".into(),
            args: vec!["--home".into(), "${AGENT_HOME}/engine".into(), "acp".into()],
            env: BTreeMap::new(),
            secret_env: BTreeMap::from([("AGENT_MODEL_API_KEY".into(), "model-key".into())]),
            working_directory: "${AGENT_HOME}/workspace".into(),
            allowed_working_roots: vec!["${AGENT_HOME}/workspace".into()],
        });
        write_private(
            &dir.path().join("config.json"),
            &serde_json::to_vec(&doc).unwrap(),
        )
        .unwrap();
        let state = Arc::new(AppState::open(dir.path().into()).unwrap());
        let token = std::fs::read_to_string(dir.path().join("secrets/control-token")).unwrap();
        let router = server::router(state.clone());
        Self {
            dir,
            state,
            router,
            token,
        }
    }
    async fn request(&self, method: &str, path: &str, body: Value, token: &str) -> (u16, Value) {
        let r = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/v1/{path}"))
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(if body.is_null() {
                        String::new()
                    } else {
                        body.to_string()
                    }))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = r.status().as_u16();
        let v = serde_json::from_slice(&to_bytes(r.into_body(), 2097152).await.unwrap()).unwrap();
        (status, v)
    }
    async fn api(&self, method: &str, path: &str, body: Value) -> Value {
        let (s, v) = self.request(method, path, body, &self.token).await;
        assert_eq!(s, 200, "{v}");
        v
    }
}

#[tokio::test]
async fn managed_shutdown_stops_the_pending_callback() {
    let f = Fixture::new();
    f.start().await;
    f.wait("pending").await;
    let pid = f.pid();
    f.state.runtime_stop.send(()).unwrap();
    f.wait("interrupted").await;
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}
