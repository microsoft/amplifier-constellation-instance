use agent_instance::{auth, host, memory, server, state::AppState};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;
fn fixture() -> (tempfile::TempDir, Arc<AppState>) {
    let dir = tempfile::tempdir().unwrap();
    host::init(
        dir.path(),
        "Memory individual".into(),
        None,
        "127.0.0.1".into(),
        0,
    )
    .unwrap();
    let state = Arc::new(AppState::open(dir.path().into()).unwrap());
    (dir, state)
}
fn remember(s: &AppState, key: &str, statement: &str, binding: Option<&Value>) -> Value {
    if let Some(b) = binding {
        return memory::mcp_call(s,b,"memory_remember",&json!({"key":key,"statement":statement,"source_note":"User explicitly supplied this in the current conversation"})).unwrap();
    }
    memory::remember(
        s,
        serde_json::from_value(
            json!({"key":key,"statement":statement,"source_note":"Owner correction"}),
        )
        .unwrap(),
        "operator",
        None,
    )
    .unwrap()
}
#[test]
fn conflicting_accounts_keep_sources_and_need_explicit_revisioned_resolution() {
    let (_dir, s) = fixture();
    let first = remember(&s, "release date", "Monday", None);
    let conflict = remember(&s, "Release Date", "Tuesday", None);
    assert_eq!(conflict["id"], first["id"]);
    assert_eq!(conflict["state"], "conflicted");
    assert!(conflict["selected_candidate_id"].is_null());
    assert_eq!(conflict["candidates"].as_array().unwrap().len(), 2);
    let orientation = memory::orientation(&s, &json!({"owner":true})).unwrap();
    assert!(orientation["items"][0]["statement"].is_null());
    let command = |revision| {
        serde_json::from_value(json!({"expected_revision":revision,"command":{"type":"resolve","candidate_id":conflict["candidates"][1]["id"],"reason":"Owner confirmed the rescheduled date"}})).unwrap()
    };
    assert!(
        memory::command(
            &s,
            first["id"].as_str().unwrap(),
            command(1),
            "operator",
            None
        )
        .is_err()
    );
    let resolved = memory::command(
        &s,
        first["id"].as_str().unwrap(),
        command(2),
        "operator",
        None,
    )
    .unwrap();
    assert_eq!(resolved["state"], "active");
    assert_eq!(resolved["candidates"][0]["statement"], "Monday");
    // A fresh report of a previously rejected value reopens the conflict rather than burying it.
    let reopened = remember(&s, "release date", "Monday", None);
    assert_eq!(reopened["state"], "conflicted");
    assert_eq!(
        reopened["candidates"][0]["sources"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
#[test]
fn audiences_are_separate_and_never_inherited_between_conversations() {
    let (_dir, s) = fixture();
    let a = json!({"provider_id":"slack","conversation_id":"work","owner":false,"actor_id":"Alex"});
    let b = json!({"provider_id":"slack","conversation_id":"personal","owner":false});
    let work = remember(&s, "priority", "Ship the release", Some(&a));
    let private = remember(&s, "priority", "Family dinner", None);
    remember(&s, "priority", "A different context", Some(&b));
    assert_ne!(work["id"], private["id"]);
    let rows = memory::list(&s, Some(&a)).unwrap();
    assert_eq!(rows["items"].as_array().unwrap().len(), 1);
    assert_eq!(rows["items"][0]["id"], work["id"]);
    assert!(memory::get(&s, private["id"].as_str().unwrap(), Some(&a)).is_err());
    assert_eq!(
        memory::list(&s, Some(&json!({"owner":true}))).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert!(memory::mcp_call(&s,&a,"memory_remember",&json!({"key":"private","statement":"leak","source_note":"bad","audience":{"kind":"owner"}})).is_err());
    assert!(memory::mcp_call(&s,&a,"memory_command",&json!({"id":private["id"],"expected_revision":1,"command":{"type":"archive","reason":"Unauthorized"}})).is_err());
    assert_eq!(
        work["candidates"][0]["sources"][0]["source"]["conversation_id"],
        "work"
    );
}
#[test]
fn restart_retains_memory_and_archived_topics_stay_out_of_orientation() {
    let (dir, s) = fixture();
    let record = remember(&s, "decision", "Use an explicit ACP boundary", None);
    let fresh = AppState::open(dir.path().into()).unwrap();
    let id = record["id"].as_str().unwrap();
    assert_eq!(memory::get(&fresh, id, None).unwrap(), record);
    memory::command(&fresh,id,serde_json::from_value(json!({"expected_revision":1,"command":{"type":"archive","reason":"Superseded by new project"}})).unwrap(),"operator",None).unwrap();
    assert_eq!(
        memory::orientation(&fresh, &json!({"owner":true})).unwrap()["items"],
        json!([])
    );
    assert_eq!(
        memory::get(&fresh, id, None).unwrap()["candidates"][0]["statement"],
        "Use an explicit ACP boundary"
    );
}
#[tokio::test]
async fn management_scopes_discovery_and_event_privacy() {
    let (_dir, s) = fixture();
    let observe = auth::issue_token(&s, "observer", &["observe".into()], 3600).unwrap();
    let read = auth::issue_token(&s, "reader", &["memory:read".into()], 3600).unwrap();
    let write = auth::issue_token(&s, "writer", &["memory".into()], 3600).unwrap();
    let router = server::router(s.clone());
    for (method, path, token, body, expected) in [
        (
            "GET",
            "memory",
            observe["token"].as_str().unwrap(),
            Value::Null,
            403,
        ),
        (
            "GET",
            "memory",
            read["token"].as_str().unwrap(),
            Value::Null,
            200,
        ),
        (
            "POST",
            "memory",
            read["token"].as_str().unwrap(),
            json!({"key":"private","statement":"Personal detail","source_note":"Owner supplied"}),
            403,
        ),
        (
            "POST",
            "memory",
            write["token"].as_str().unwrap(),
            json!({"key":"private","statement":"Personal detail","source_note":"Owner supplied"}),
            200,
        ),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/v1/{path}"))
                    .header("host", "localhost")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(if body.is_null() {
                        String::new()
                    } else {
                        body.to_string()
                    }))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            expected,
            "{}",
            String::from_utf8_lossy(&to_bytes(response.into_body(), 1048576).await.unwrap())
        );
    }
    let events =
        s.db.lock()
            .unwrap()
            .query_row(
                "SELECT data FROM events WHERE kind='memory.changed'",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap();
    assert!(!events.contains("Personal detail"));
    assert!(!events.contains("Owner supplied"));
    let openapi = agent_instance::discovery::openapi();
    assert!(openapi["paths"]["/memory"]["post"]["requestBody"].is_object());
}
