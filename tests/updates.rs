use agent_instance::{auth, host, server, state::AppState, updates};
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
#[tokio::test]
async fn global_switch_authorizes_agent_without_requester_identity() {
    let (_d, s) = fixture();
    assert_eq!(updates::policy(&s).unwrap().check_minutes, 60);
    assert_eq!(
        api(
            &s,
            "POST",
            "updates/report",
            json!({"status":{"state":"idle"}}),
            false
        )
        .await
        .0,
        401
    );
    assert_eq!(
        api(
            &s,
            "POST",
            "updates/report",
            json!({"status":{"state":"idle"}}),
            true
        )
        .await
        .0,
        200
    );
    assert!(updates::mcp_call(&s, &json!({"action":"update"})).is_err());
    assert_eq!(
        api(
            &s,
            "PUT",
            "updates/policy",
            json!({"agent_initiated":true}),
            true
        )
        .await
        .0,
        200
    );
    // No Slack identity, owner, audience, or requester exists in this authorization path.
    let accepted = updates::mcp_call(&s, &json!({"action":"update"})).unwrap();
    assert_eq!(accepted["actor"], "agent");
    assert_eq!(accepted["pending"], true);
    assert_eq!(
        api(
            &s,
            "POST",
            "updates/report",
            json!({"status":{"state":"succeeded"},"finished_request":accepted["id"]}),
            true
        )
        .await
        .0,
        200
    );
    assert_eq!(
        api(
            &s,
            "PUT",
            "updates/policy",
            json!({"revision":1,"agent_initiated":false}),
            true
        )
        .await
        .0,
        200
    );
    assert!(updates::mcp_call(&s, &json!({"action":"update"})).is_err());
    assert_eq!(
        api(&s, "PUT", "updates/policy", json!({"revision":1}), true)
            .await
            .0,
        409
    );
}
#[tokio::test]
async fn drain_tracks_live_prompts_and_rejects_new_ones() {
    let (_d, s) = fixture();
    let prompt = updates::admit(&s).unwrap();
    let (_, v) = api(
        &s,
        "POST",
        "updates/report",
        json!({"status":{"state":"waiting_for_idle"},"drain_seconds":60}),
        true,
    )
    .await;
    assert_eq!(v["active_prompts"], 1);
    assert!(updates::admit(&s).is_none());
    drop(prompt);
    assert_eq!(updates::status(&s).unwrap()["active_prompts"], 0);
    api(
        &s,
        "POST",
        "updates/report",
        json!({"status":{"state":"idle"}}),
        true,
    )
    .await;
    assert!(updates::admit(&s).is_some());
}
#[tokio::test]
async fn operation_payloads_cannot_select_host_commands_or_sources() {
    let (_d, s) = fixture();
    api(&s, "POST", "updates/report", json!({"status":{}}), true).await;
    let body = json!({"action":"update","request_id":uuid::Uuid::new_v4().to_string(),"command":"/bin/sh"});
    assert_eq!(api(&s, "POST", "updates", body, true).await.0, 422);
    assert_eq!(
        api(
            &s,
            "PUT",
            "updates/policy",
            json!({"check_minutes":2}),
            true
        )
        .await
        .0,
        400
    );
    let body = json!({"action":"update","request_id":uuid::Uuid::new_v4().to_string()});
    let first = api(&s, "POST", "updates", body.clone(), true).await;
    assert_eq!(first.0, 200);
    assert_eq!(api(&s, "POST", "updates", body, true).await.1, first.1);
    assert_eq!(
        api(
            &s,
            "POST",
            "updates",
            json!({"action":"check","request_id":uuid::Uuid::new_v4().to_string()}),
            true
        )
        .await
        .0,
        409
    );
    // A read-only observer cannot change the instance-wide switch.
    let _ = auth::digest("observer");
}
