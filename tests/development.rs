use agent_instance::{
    auth, development, host, server, state::AppState, tool_connections::Connection,
};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use tower::ServiceExt;

fn fixture() -> (tempfile::TempDir, Arc<AppState>) {
    let d = tempfile::tempdir().unwrap();
    host::init(
        d.path(),
        "Development fixture".into(),
        None,
        "127.0.0.1".into(),
        0,
    )
    .unwrap();
    let s = Arc::new(AppState::open(d.path().into()).unwrap());
    (d, s)
}
#[test]
fn network_status_tracks_native_owner_policy_without_claiming_build_readiness() {
    let (_d, s) = fixture();
    let engine = s.home.join("engine");
    std::fs::create_dir_all(&engine).unwrap();
    let executable = engine.join("agent-runtime");
    std::fs::write(&executable, b"fixture-only, never executed").unwrap();
    let mut config = s.instance_config();
    config.backend = Some(agent_instance::config::BackendConfig {
        command: executable.to_string_lossy().into_owned(),
        args: vec!["--home".into(), engine.to_string_lossy().into_owned()],
        env: BTreeMap::new(),
        secret_env: BTreeMap::new(),
        working_directory: s.home.to_string_lossy().into_owned(),
        allowed_working_roots: vec![s.home.to_string_lossy().into_owned()],
    });
    server::replace_config(&s, 1, config, "fixture").unwrap();
    for policy in [None, Some("inherit"), Some("isolated")] {
        let mut native = json!({"limits":{"command_seconds":60}});
        if let Some(policy) = policy {
            native["command_network"] = json!(policy);
        }
        std::fs::write(engine.join("runtime.json"), native.to_string()).unwrap();
        let summary = development::summary(&s).unwrap();
        assert_eq!(
            summary["environment"]["network"],
            policy.unwrap_or("isolated")
        );
        assert_eq!(summary["environment"]["build_verified"], false);
        assert_eq!(summary["ready_to_develop"], false);
    }
}
#[test]
fn setup_preserves_other_tools_replaces_only_github_and_keeps_secrets_private() {
    let (_d, s) = fixture();
    let mut c = s.instance_config();
    let project = Connection {
        optional: false,
        name: "project-tools".into(),
        url: "https://project.example/mcp".into(),
        header_secrets: BTreeMap::new(),
    };
    c.tool_connections.push(project.clone());
    server::replace_config(&s, 1, c, "fixture").unwrap();
    development::install(&s, 2, "Bearer private-development-canary", "fixture").unwrap();
    let first = s.instance_config();
    assert!(first.tool_connections.contains(&project));
    assert_eq!(first.tool_connections.len(), 2);
    assert!(development::install(&s, 2, "Bearer replacement-canary", "fixture").is_err());
    assert_eq!(s.instance_config().tool_connections, first.tool_connections);
    development::install(&s, 3, "Bearer replacement-canary", "fixture").unwrap();
    assert_eq!(s.instance_config().tool_connections.len(), 2);
    assert!(s.instance_config().tool_connections.contains(&project));
    let status = development::summary(&s).unwrap();
    assert_eq!(status["github"]["configured"], true);
    assert_eq!(status["github"]["verification"], Value::Null);
    assert_eq!(status["ready_to_develop"], false);
    assert!(
        !serde_json::to_string(&s.instance_config())
            .unwrap()
            .contains("canary")
    );
    let db = s.db.lock().unwrap();
    let mut stmt = db.prepare("SELECT data FROM events").unwrap();
    for row in stmt.query_map([], |r| r.get::<_, String>(0)).unwrap() {
        assert!(!row.unwrap().contains("canary"));
    }
}
#[test]
fn local_enablement_preserves_existing_identity_expiry_and_remote_grants() {
    let (_d, s) = fixture();
    let local =
        auth::issue_token(&s, "local", &["acp".into(), "context:owner".into()], 1200).unwrap();
    let remote = auth::issue_token(&s, "remote", &["acp".into()], 1200).unwrap();
    agent_instance::config::write_private(
        &s.home.join("secrets/acp-token"),
        local["token"].as_str().unwrap().as_bytes(),
    )
    .unwrap();
    development::enable_local(&s).unwrap();
    development::enable_local(&s).unwrap();
    assert_eq!(
        agent_instance::acp::read_token_file(&s.home.join("secrets/acp-token")).unwrap(),
        local["token"]
    );
    let db = s.db.lock().unwrap();
    let (scope, expiry): (String, i64) = db
        .query_row(
            "SELECT scopes,expires_at FROM tokens WHERE id=?1",
            [local["id"].as_str().unwrap()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&scope).unwrap(),
        json!(["acp", "context:owner", "tools:use"])
    );
    assert_eq!(expiry, local["expires_at"].as_i64().unwrap());
    let remote_scope: String = db
        .query_row(
            "SELECT scopes FROM tokens WHERE id=?1",
            [remote["id"].as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&remote_scope).unwrap(),
        json!(["acp"])
    );
}
#[test]
fn invalid_local_credential_never_falls_back_to_admin_or_enables_remote_tools() {
    let (_d, s) = fixture();
    let admin = std::fs::read(s.home.join("secrets/control-token")).unwrap();
    agent_instance::config::write_private(&s.home.join("secrets/acp-token"), &admin).unwrap();
    assert!(development::enable_local(&s).is_err());
    assert_eq!(
        development::summary(&s).unwrap()["github"]["local_clients_enabled"],
        false
    );
    agent_instance::config::write_private(
        &s.home.join("secrets/acp-token"),
        b"invalid-private-token",
    )
    .unwrap();
    assert!(development::enable_local(&s).is_err());
}
#[tokio::test]
async fn api_requires_admin_and_checks_revisions_before_credentials_or_network() {
    let (_d, s) = fixture();
    let observer = auth::issue_token(&s, "observer", &["observe".into()], 600).unwrap();
    let admin = std::fs::read_to_string(s.home.join("secrets/control-token")).unwrap();
    for (method, path, token, body, expected) in [
        ("GET", "development", None, None, 401),
        (
            "GET",
            "development",
            Some(observer["token"].as_str().unwrap()),
            None,
            403,
        ),
        (
            "POST",
            "development/github",
            Some(observer["token"].as_str().unwrap()),
            Some(json!({"expected_revision":1})),
            403,
        ),
        (
            "POST",
            "development/github",
            Some(admin.as_str()),
            Some(json!({"expected_revision":999})),
            409,
        ),
        (
            "POST",
            "development/github",
            Some(admin.as_str()),
            Some(json!({"expected_revision":1,"credential":"bad\nsecret"})),
            400,
        ),
        (
            "POST",
            "development/github/check",
            Some(admin.as_str()),
            Some(json!({})),
            400,
        ),
    ] {
        let mut req = Request::builder()
            .method(method)
            .uri(format!("/api/v1/{path}"));
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let response = server::router(s.clone())
            .oneshot(
                req.header("content-type", "application/json")
                    .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected, "{method} {path}");
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("bad\\nsecret"));
    }
}
