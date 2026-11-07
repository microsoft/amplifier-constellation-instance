use agent_instance::{host, server, state::AppState};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;
fn fixture() -> (tempfile::TempDir, Arc<AppState>) {
    let d = tempfile::tempdir().unwrap();
    host::init(
        d.path(),
        "Update fixture".into(),
        None,
        "127.0.0.1".into(),
        0,
    )
    .unwrap();
    let s = Arc::new(AppState::open(d.path().into()).unwrap());
    (d, s)
}
async fn api(
    s: &Arc<AppState>,
    method: &str,
    path: &str,
    body: Value,
    authorized: bool,
) -> (u16, Value) {
    let mut r = Request::builder()
        .uri(format!("/api/v1/{path}"))
        .method(method)
        .header("content-type", "application/json");
    if authorized {
        let token = std::fs::read_to_string(s.home.join("secrets/control-token")).unwrap();
        r = r.header("authorization", format!("Bearer {}", token.trim()));
    }
    let response = server::router(s.clone())
        .oneshot(r.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    (
        response.status().as_u16(),
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap_or_else(|_| json!({"error":"non-json rejection"})),
    )
}
fn catalog(state: &str, request: Option<&str>, settled: bool) -> Value {
    json!({"schema":1,"services":[{"id":"minecraft","label":"Minecraft","setup_url":"https://example.test/minecraft"}],"connections":[{"id":"world","service":"minecraft","label":"Workshop","revision":"r1","context_provider_ids":["world-context"],"state":state,"request_id":request,"settled":settled,"message":""}]})
}
fn operation(action: &str) -> Value {
    json!({"connection_id":"world","revision":"r1","action":action,"request_id":uuid::Uuid::new_v4().to_string()})
}
#[tokio::test]
async fn disconnect_preserves_context_until_quiesced_then_only_matching_connect_reopens() {
    let (d, s) = fixture();
    assert_eq!(
        api(
            &s,
            "POST",
            "service-connections/report",
            catalog("connected", None, false),
            true
        )
        .await
        .0,
        200
    );
    assert!(agent_instance::service_connections::context_enabled(&s, "world-context").unwrap());
    let mut noncanonical = operation("disconnect");
    noncanonical["request_id"] = json!("AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA");
    assert_eq!(
        api(&s, "POST", "service-connections", noncanonical, true)
            .await
            .0,
        400
    );
    let disconnect = operation("disconnect");
    assert_eq!(
        api(&s, "POST", "service-connections", disconnect.clone(), true)
            .await
            .0,
        200
    );
    // Queueing must not break a concurrent updater's saved-session verification.
    assert!(agent_instance::service_connections::context_enabled(&s, "world-context").unwrap());
    api(
        &s,
        "POST",
        "service-connections/report",
        catalog(
            "busy",
            Some(disconnect["request_id"].as_str().unwrap()),
            false,
        ),
        true,
    )
    .await;
    assert!(agent_instance::service_connections::context_enabled(&s, "world-context").unwrap());
    let mut quiesced = catalog("busy", disconnect["request_id"].as_str(), false);
    quiesced["connections"][0]["context_quiesced"] = json!(true);
    api(&s, "POST", "service-connections/report", quiesced, true).await;
    assert!(!agent_instance::service_connections::context_enabled(&s, "world-context").unwrap());
    api(
        &s,
        "POST",
        "service-connections/report",
        catalog("busy", disconnect["request_id"].as_str(), false),
        true,
    )
    .await;
    assert!(!agent_instance::service_connections::context_enabled(&s, "world-context").unwrap());
    let reopened = Arc::new(AppState::open(d.path().into()).unwrap());
    assert!(
        !agent_instance::service_connections::context_enabled(&reopened, "world-context").unwrap()
    );
    assert_eq!(
        api(
            &reopened,
            "POST",
            "service-connections",
            disconnect.clone(),
            true
        )
        .await
        .0,
        200
    );
    let mut changed = disconnect.clone();
    changed["action"] = json!("connect");
    assert_eq!(
        api(&s, "POST", "service-connections", changed, true)
            .await
            .0,
        409
    );
    assert_eq!(
        api(
            &s,
            "POST",
            "service-connections",
            operation("connect"),
            true
        )
        .await
        .0,
        409
    );
    let result = api(
        &s,
        "POST",
        "service-connections/report",
        catalog(
            "disconnected",
            Some(disconnect["request_id"].as_str().unwrap()),
            true,
        ),
        true,
    )
    .await;
    assert!(result.1["request"].is_null());
    let connect = operation("connect");
    assert_eq!(
        api(&s, "POST", "service-connections", connect.clone(), true)
            .await
            .0,
        200
    );
    api(
        &s,
        "POST",
        "service-connections/report",
        catalog("connected", None, true),
        true,
    )
    .await;
    assert!(!agent_instance::service_connections::context_enabled(&s, "world-context").unwrap());
    api(
        &s,
        "POST",
        "service-connections/report",
        catalog(
            "connected",
            Some(connect["request_id"].as_str().unwrap()),
            true,
        ),
        true,
    )
    .await;
    assert!(agent_instance::service_connections::context_enabled(&s, "world-context").unwrap());
    // Removing a saved enrollment cannot reopen context or erase its saved card.
    let r = api(
        &s,
        "POST",
        "service-connections/report",
        json!({"schema":1,"services":[],"connections":[]}),
        true,
    )
    .await;
    assert_eq!(r.1["connections"][0]["state"], "unknown");
    assert!(!agent_instance::service_connections::context_enabled(&s, "world-context").unwrap());
}
#[tokio::test]
async fn fixed_actions_require_auth_enrollment_revision_and_unambiguous_context_scope() {
    let (_d, s) = fixture();
    assert_eq!(
        api(
            &s,
            "POST",
            "service-connections/report",
            catalog("connected", None, false),
            false
        )
        .await
        .0,
        401
    );
    let mut bad = catalog("connected", None, false);
    bad["services"][0]["setup_url"] = json!("javascript:alert(1)");
    assert_eq!(
        api(&s, "POST", "service-connections/report", bad, true)
            .await
            .0,
        400
    );
    api(
        &s,
        "POST",
        "service-connections/report",
        catalog("connected", None, false),
        true,
    )
    .await;
    let mut c = operation("disconnect");
    c["revision"] = json!("old");
    assert_eq!(api(&s, "POST", "service-connections", c, true).await.0, 409);
    let mut c = operation("disconnect");
    c["argv"] = json!(["/bin/sh"]);
    assert_ne!(api(&s, "POST", "service-connections", c, true).await.0, 200);
    let c = operation("disconnect");
    api(&s, "POST", "service-connections", c, true).await;
    let mut changed = catalog("connected", None, false);
    changed["connections"][0]["context_provider_ids"] = json!(["other"]);
    assert_eq!(
        api(&s, "POST", "service-connections/report", changed, true)
            .await
            .0,
        409
    );
}

#[tokio::test]
async fn context_ready_is_bound_to_connect_and_does_not_settle_it() {
    let (_d, s) = fixture();
    api(
        &s,
        "POST",
        "service-connections/report",
        catalog("disconnected", None, false),
        true,
    )
    .await;
    let c = operation("connect");
    api(&s, "POST", "service-connections", c.clone(), true).await;
    let mut progress = catalog("busy", Some(c["request_id"].as_str().unwrap()), false);
    progress["connections"][0]["context_ready"] = json!(true);
    let status = api(&s, "POST", "service-connections/report", progress, true).await;
    assert!(status.1["request"].is_object());
    assert!(agent_instance::service_connections::context_enabled(&s, "world-context").unwrap());
    api(
        &s,
        "POST",
        "service-connections/report",
        catalog("unknown", Some(c["request_id"].as_str().unwrap()), false),
        true,
    )
    .await;
    assert!(!agent_instance::service_connections::context_enabled(&s, "world-context").unwrap());
    let mut failure = catalog(
        "disconnected",
        Some(c["request_id"].as_str().unwrap()),
        true,
    );
    failure["connections"][0]["failed"] = json!(true);
    assert!(
        api(&s, "POST", "service-connections/report", failure, true)
            .await
            .1["request"]
            .is_null()
    );
    assert!(!agent_instance::service_connections::context_enabled(&s, "world-context").unwrap());
}

#[tokio::test]
async fn add_validates_owner_schema_and_requires_saved_enrollment_for_settlement() {
    let (_d, s) = fixture();
    let service = json!({"id":"minecraft","label":"Minecraft","add":{"revision":"template1","fields":[{"name":"label","label":"Name","kind":"text","max_length":50,"required":true},{"name":"server","label":"Server","kind":"choice","required":true,"choices":[{"value":"workshop","label":"Workshop"}]}]}});
    let report = json!({"schema":1,"services":[service],"connections":[]});
    assert_eq!(
        api(
            &s,
            "POST",
            "service-connections/report",
            report.clone(),
            true
        )
        .await
        .0,
        200
    );
    let mut c = json!({"action":"add","service_id":"minecraft","revision":"template1","request_id":uuid::Uuid::new_v4().to_string(),"values":{"label":"A","server":"unknown"}});
    assert_eq!(
        api(&s, "POST", "service-connections", c.clone(), true)
            .await
            .0,
        400
    );
    c["values"]["server"] = json!("workshop");
    c["values"]["command"] = json!("rm anything");
    assert_eq!(
        api(&s, "POST", "service-connections", c.clone(), true)
            .await
            .0,
        400
    );
    c["values"].as_object_mut().unwrap().remove("command");
    assert_eq!(
        api(&s, "POST", "service-connections", c.clone(), true)
            .await
            .0,
        200
    );
    let mut result = report;
    result["add_result"] = json!({"service_id":"minecraft","request_id":c["request_id"],"state":"saved","settled":true,"connection_id":"world","message":"Saved"});
    assert!(
        api(
            &s,
            "POST",
            "service-connections/report",
            result.clone(),
            true
        )
        .await
        .1["request"]
            .is_object()
    );
    result["connections"] = catalog("disconnected", None, false)["connections"].clone();
    assert!(
        api(&s, "POST", "service-connections/report", result, true)
            .await
            .1["request"]
            .is_null()
    );
    assert!(!agent_instance::service_connections::context_enabled(&s, "world-context").unwrap());
}

#[tokio::test]
async fn existing_only_catalog_has_no_invented_setup_requirement() {
    let (_d, s) = fixture();
    let mut c = catalog("connected", None, false);
    c["services"][0]
        .as_object_mut()
        .unwrap()
        .remove("setup_url");
    let (status, body) = api(&s, "POST", "service-connections/report", c, true).await;
    assert_eq!(status, 200);
    assert!(body["services"][0]["add"].is_null());
}
