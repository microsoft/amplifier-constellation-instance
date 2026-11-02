use agent_instance::{activity::DIAGNOSTIC_RETENTION, auth, host, server, state::AppState};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

struct Fixture {
    _dir: tempfile::TempDir,
    state: Arc<AppState>,
    router: Router,
    token: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        host::init(
            dir.path(),
            "Activity fixture".into(),
            None,
            "127.0.0.1".into(),
            0,
        )
        .unwrap();
        let state = Arc::new(AppState::open(dir.path().to_owned()).unwrap());
        state
            .db
            .lock()
            .unwrap()
            .execute("DELETE FROM events", [])
            .unwrap();
        let token = std::fs::read_to_string(dir.path().join("secrets/control-token")).unwrap();
        Self {
            router: server::router(state.clone()),
            _dir: dir,
            state,
            token,
        }
    }
    async fn get(&self, path: &str, token: Option<&str>) -> (u16, Value) {
        let mut request = Request::builder().uri(format!("/api/v1/{path}"));
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = self
            .router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status().as_u16();
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    fn token_for(&self, scope: &str) -> String {
        auth::issue_token(&self.state, "Activity test", &[scope.to_owned()], 600).unwrap()["token"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

#[tokio::test]
async fn recent_meaningful_paging_survives_more_than_500_ui_events() {
    let f = Fixture::new();
    let started = f
        .state
        .event(
            "acp.prompt.started",
            json!({"work_id":"work-a","session_id":"project-a"}),
        )
        .unwrap();
    for revision in 0..1200 {
        f.state
            .event(
                "view.changed",
                json!({"id":"display-a","revision":revision}),
            )
            .unwrap();
    }
    let finished = f.state.event("acp.prompt.finished", json!({"work_id":"work-a","session_id":"project-a","stop_reason":"end_turn","duration_ms":30})).unwrap();

    let (status, first) = f.get("activity?limit=1", Some(&f.token)).await;
    assert_eq!(status, 200);
    assert_eq!(first["items"][0]["seq"], finished["seq"]);
    assert_eq!(first["items"][0]["summary"], "Work turn finished");
    assert_eq!(first["items"][0]["category"], "outcomes");
    assert_eq!(first["items"][0]["severity"], "success");
    assert_eq!(first["next_before"], finished["seq"]);
    assert_eq!(first["has_more"], true);
    let (_, second) = f
        .get(
            &format!("activity?limit=1&before={}", first["next_before"]),
            Some(&f.token),
        )
        .await;
    assert_eq!(second["items"][0]["seq"], started["seq"]);
    assert_eq!(second["has_more"], false);
    assert!(second["next_before"].is_null());
    let (_, filtered) = f
        .get(
            "activity?category=outcomes&search=PROJECT-A",
            Some(&f.token),
        )
        .await;
    assert_eq!(filtered["items"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["items"][0]["seq"], finished["seq"]);
    let (_, diagnostics) = f
        .get(
            "activity?diagnostics=true&category=diagnostics&limit=2",
            Some(&f.token),
        )
        .await;
    assert_eq!(diagnostics["items"].as_array().unwrap().len(), 2);
    assert_eq!(diagnostics["items"][0]["kind"], "view.changed");
    assert_eq!(diagnostics["has_more"], true);
    // The existing oldest-first event endpoint remains available unchanged.
    let (_, raw) = f.get("events", Some(&f.token)).await;
    assert_eq!(raw["events"].as_array().unwrap().len(), 500);
    assert_eq!(raw["events"][0]["seq"], started["seq"]);
    assert!(
        raw["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "view.changed")
    );
}

#[tokio::test]
async fn meaningful_and_diagnostic_retention_have_independent_budgets() {
    let f = Fixture::new();
    f.state
        .config
        .write()
        .unwrap()
        .config
        .limits
        .event_retention = 100;
    let first = f
        .state
        .event("config.changed", json!({"revision":1}))
        .unwrap();
    for revision in 0..1500 {
        f.state
            .event("view.changed", json!({"revision":revision}))
            .unwrap();
    }
    let (_, activity) = f.get("activity", Some(&f.token)).await;
    assert_eq!(
        activity["items"][0]["seq"], first["seq"],
        "UI traffic must not evict meaningful activity"
    );
    for revision in 2..=101 {
        f.state
            .event("config.changed", json!({"revision":revision}))
            .unwrap();
    }
    let db = f.state.db.lock().unwrap();
    let meaningful: i64 = db
        .query_row(
            "SELECT count(*) FROM events WHERE activity_diagnostic=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let diagnostics: i64 = db
        .query_row(
            "SELECT count(*) FROM events WHERE activity_diagnostic=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(meaningful, 100);
    assert_eq!(diagnostics, DIAGNOSTIC_RETENTION as i64);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM events WHERE seq=?1",
            [first["seq"].as_i64().unwrap()],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn activity_requires_observe_scope_and_omits_content_and_secret_fields() {
    let f = Fixture::new();
    f.state.event("acp.prompt.failed", json!({
        "work_id":"test-work","session_id":"test-session","duration_ms":12,"error_code":-32000,
        "prompt":"prompt-canary","error":{"message":"error-canary"},"output":"output-canary",
        "profile":"profile-canary","token":"token-canary","env":{"SECRET":"env-canary"}
    })).unwrap();
    assert_eq!(f.get("activity", None).await.0, 401);
    let ui = f.token_for("ui");
    let acp = f.token_for("acp");
    let observer = f.token_for("observe");
    assert_eq!(f.get("activity", Some(&ui)).await.0, 403);
    assert_eq!(f.get("activity", Some(&acp)).await.0, 403);
    let (status, body) = f.get("activity?category=problems", Some(&observer)).await;
    assert_eq!(status, 200);
    assert!(!body.to_string().contains("canary"));
    assert_eq!(body["items"][0]["data"]["work_id"], "test-work");
    assert_eq!(body["items"][0]["data"]["error_code"], -32000);
    let (_, searching_content) = f.get("activity?search=canary", Some(&observer)).await;
    assert!(
        searching_content["items"].as_array().unwrap().is_empty(),
        "Search must use only the safe projection"
    );
    for query in [
        "limit=0",
        "limit=201",
        "before=-1",
        "category=unknown",
        "unexpected=true",
    ] {
        assert_eq!(
            f.get(&format!("activity?{query}"), Some(&observer)).await.0,
            400
        );
    }
}

#[tokio::test]
async fn existing_raw_events_migrate_without_losing_sequence_ids_or_work() {
    let f = Fixture::new();
    {
        let db = f.state.db.lock().unwrap();
        db.execute_batch("DROP TABLE events; CREATE TABLE events(seq INTEGER PRIMARY KEY AUTOINCREMENT,at INTEGER NOT NULL,kind TEXT NOT NULL,data TEXT NOT NULL);
            INSERT INTO events(seq,at,kind,data) VALUES(41,100,'acp.activity','{\"method\":\"session/prompt\",\"session_id\":\"old-session\"}');
            INSERT INTO events(seq,at,kind,data) VALUES(42,101,'acp.response','{\"method\":\"session/prompt\",\"success\":true}');
            INSERT INTO events(seq,at,kind,data) VALUES(43,102,'view.changed','{\"revision\":5}');").unwrap();
    }
    let reopened = Arc::new(AppState::open(f.state.home.clone()).unwrap());
    let projection: String = reopened
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT activity_category FROM events WHERE seq=42",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(projection, "outcomes");
    assert_eq!(
        reopened.event("service.started", json!({})).unwrap()["seq"],
        44
    );
    // Reopening an already migrated database is idempotent.
    let again = AppState::open(f.state.home.clone()).unwrap();
    assert_eq!(
        again
            .db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM events", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        4
    );
}
