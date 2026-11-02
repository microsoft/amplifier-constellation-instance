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

#[tokio::test]
async fn native_credential_action_is_private_revisioned_and_connects_the_environment() {
    let f = Fixture::new();
    let initial = f.api("GET", "native-config", Value::Null).await;
    let body = json!({"expected_backend_sha256":initial["backend_sha256"],"value":"credential-test-canary"});
    let (denied, _) = f
        .request(
            "POST",
            "native-config/credential",
            body.clone(),
            "invalid-token",
        )
        .await;
    assert_ne!(denied, 200);
    let mut stale = body.clone();
    stale["expected_backend_sha256"] = json!("old");
    assert_eq!(
        f.request("POST", "native-config/credential", stale, &f.token)
            .await
            .0,
        409
    );
    let result = f.api("POST", "native-config/credential", body).await;
    assert_eq!(result["configured"], true);
    assert!(!result.to_string().contains("credential-test-canary"));
    let config = f.state.instance_config();
    let name = &config.backend.unwrap().secret_env["AGENT_MODEL_API_KEY"];
    assert!(name.starts_with("native-model-key-"));
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("secrets").join(name)).unwrap(),
        "credential-test-canary"
    );
    let inspected = f.api("GET", "native-config", Value::Null).await;
    assert!(!inspected.to_string().contains("credential-test-canary"));
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
                "/tests/fixtures/native_config.py"
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
async fn native_configuration_uses_versioned_engine_commands_and_does_not_forward_credentials() {
    let f = Fixture::new();
    let initial = f.api("GET", "native-config", Value::Null).await;
    assert_eq!(initial["credential_inherited"], false);
    assert_eq!(
        initial["credential_mapping"]["AGENT_MODEL_API_KEY"],
        "model-key"
    );
    assert!(!initial.to_string().contains("private-secret-canary"));
    assert_eq!(
        f.api("GET", "native-config/schema", Value::Null).await["x-agent-runtime"]["contract_version"],
        1
    );
    let mut config = initial["config"].clone();
    config["provider"]["model"] = json!("updated-fixture");
    let body = json!({"config":config,"expected_revision":initial["revision"],"expected_backend_sha256":initial["backend_sha256"]});
    assert_eq!(
        f.api("POST", "native-config/validate", body.clone()).await["valid"],
        true
    );
    assert_eq!(
        f.api("GET", "native-config", Value::Null).await["config"]["provider"]["model"],
        "fixture"
    );
    let approved = scheduler::backend_hash(&f.state);
    let saved = f.api("PUT", "native-config", body.clone()).await;
    assert_eq!(saved["restart_required"], true);
    assert_eq!(saved["active_connections_unchanged"], true);
    assert_ne!(scheduler::backend_hash(&f.state), approved);
    assert_eq!(
        f.api("GET", "native-config", Value::Null).await["config"]["provider"]["model"],
        "updated-fixture"
    );
    assert_eq!(
        f.request("PUT", "native-config", body, &f.token).await.0,
        409
    );
    assert!(!std::fs::read_dir(f.dir.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".native-config-")
    }));
}
#[tokio::test]
async fn only_administration_can_edit_native_config_and_invalid_config_leaves_file_unchanged() {
    let f = Fixture::new();
    let observe = auth::issue_token(&f.state, "observer", &["observe".into()], 3600).unwrap();
    assert_eq!(
        f.request(
            "GET",
            "native-config",
            Value::Null,
            observe["token"].as_str().unwrap()
        )
        .await
        .0,
        403
    );
    let initial = f.api("GET", "native-config", Value::Null).await;
    let mut config = initial["config"].clone();
    config["provider"]["model"] = json!("");
    assert_eq!(f.request("PUT","native-config",json!({"config":config,"expected_revision":initial["revision"],"expected_backend_sha256":initial["backend_sha256"]}),&f.token).await.0,400);
    assert_eq!(
        f.api("GET", "native-config", Value::Null).await["revision"],
        initial["revision"]
    );
}
