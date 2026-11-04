use agent_instance::{
    attention::{self, Config, Event},
    attention_policy,
    config::{ConfigDocument, InstanceConfig, private_dir, write_private},
    state::{AppState, now_ms},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tempfile::TempDir;

fn install(endpoint: String, limit: usize) -> (TempDir, Arc<AppState>) {
    let home = tempfile::tempdir().unwrap();
    private_dir(&home.path().join("secrets")).unwrap();
    write_private(&home.path().join("secrets/decision"), b"test-only").unwrap();
    let mut c = InstanceConfig::blank("Explorer".into(), "owner".into());
    c.enabled = false;
    c.attention = Some(Config {
        endpoint,
        model: "fixture".into(),
        protocol: "system_one".into(),
        auth: "bearer".into(),
        credential_secret: "decision".into(),
        decisions_per_hour: 20,
        turns_per_hour: 10,
        uncertain_turns_per_hour: limit,
    });
    write_private(
        &home.path().join("config.json"),
        &serde_json::to_vec(&ConfigDocument {
            revision: 1,
            config: c,
        })
        .unwrap(),
    )
    .unwrap();
    let s = Arc::new(AppState::open(home.path().into()).unwrap());
    s.runtime_enabled.store(true, Ordering::SeqCst);
    (home, s)
}
fn event(id: &str) -> Event {
    serde_json::from_value(json!({"sessionId":"session","occurredAt":now_ms(),"context":{"conversation_id":"room","agent":{"id":"agent","name":"Explorer"},"participants":[{"id":"agent","name":"Explorer"},{"id":"human","name":"Human"}],"messages":[{"id":id,"sender":"human","text":"What are you all working on?"}],"trigger_id":id}})).unwrap()
}
fn answer(choice: &str) -> Value {
    json!({"answers":{"participation":{"type":"choice","choice":choice,"probabilities":{"engage":if choice=="engage"{0.8}else{0.1},"hold":if choice=="hold"{0.8}else{0.1},"uncertain":if choice=="uncertain"{0.8}else{0.1}}}}})
}
async fn provider(
    body: Value,
    delay: u64,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let route = axum::Router::new().route(
        "/",
        axum::routing::post(move |axum::Json(v): axum::Json<Value>| {
            let body = body.clone();
            let count = count.clone();
            async move {
                assert_eq!(v["questions"]["participation"]["type"], "choice");
                count.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                axum::Json(body)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, route).await.unwrap() });
    (format!("http://{addr}/"), calls, task)
}
async fn offer(s: Arc<AppState>, id: &str) -> Value {
    let e = event(id);
    let r = attention::reserve(&s, "p", "c", &e, None).unwrap();
    attention::evaluate(s, r, e).await.unwrap()
}
#[test]
fn semantic_uncertainty_is_distinct_from_refusal_and_malformed_response() {
    assert_eq!(
        attention_policy::normalize(&answer("uncertain")).unwrap(),
        (
            "uncertain".into(),
            "semantic_uncertainty".into(),
            true,
            true
        )
    );
    assert!(!attention_policy::normalize(&answer("hold")).unwrap().2);
    let mut weak = answer("hold");
    weak["answers"]["participation"]["probabilities"] =
        json!({"hold":0.4,"engage":0.3,"uncertain":0.3});
    assert_eq!(
        attention_policy::normalize(&weak).unwrap().1,
        "low_confidence"
    );
    assert!(attention_policy::normalize(&weak).unwrap().2);
    assert!(
        !attention_policy::normalize(&json!({"answers":{"participation":{"type":"refusal"}}}))
            .unwrap()
            .2
    );
    assert!(
        attention_policy::normalize(
            &json!({"answers":{"participation":{"type":"choice","choice":"engage"}}})
        )
        .is_err()
    );
}
#[tokio::test]
async fn uncertain_offer_needs_separate_single_use_bounded_admission() {
    let (url, count, server) = provider(answer("uncertain"), 0).await;
    let (_home, s) = install(url, 1);
    let one = offer(s.clone(), "one").await;
    assert_eq!(one["action"], "engage");
    assert!(attention::report(&s).unwrap()["counts"]["status:admitted"].is_null());
    assert!(attention::consume(&s, "other", "c", "session", &one).is_err());
    assert!(attention::consume(&s, "p", "other", "session", &one).is_err());
    assert!(attention::consume(&s, "p", "c", "other", &one).is_err());
    let id = attention::consume(&s, "p", "c", "session", &one).unwrap();
    attention::forwarded(&s, &id).unwrap();
    assert!(attention::consume(&s, "p", "c", "session", &one).is_err());
    assert_eq!(offer(s.clone(), "one").await["reason"], "duplicate");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let two = offer(s.clone(), "two").await;
    assert!(attention::consume(&s, "p", "c", "session", &two).is_err());
    assert_eq!(
        attention::report(&s).unwrap()["counts"]["status:admitted"],
        1
    );
    server.abort();
}
#[tokio::test]
async fn hold_and_refusal_never_forward_and_provider_errors_are_bounded_and_labeled() {
    for (body, action, reason) in [
        (answer("hold"), "hold", "judgment"),
        (
            json!({"answers":{"participation":{"type":"refusal"}}}),
            "hold",
            "provider_refusal",
        ),
        (json!({"broken":true}), "engage", "provider_error"),
    ] {
        let (url, _, server) = provider(body, 0).await;
        let (_h, s) = install(url, 1);
        let o = offer(s.clone(), "one").await;
        assert_eq!(o["action"], action);
        assert_eq!(o["reason"], reason);
        assert_eq!(offer(s, "one").await["reason"], "duplicate");
        server.abort();
    }
}
#[tokio::test]
async fn supersession_cancellation_and_config_change_revoke_offers() {
    let (url, _, server) = provider(answer("engage"), 0).await;
    let (_h, s) = install(url, 10);
    let a = offer(s.clone(), "a").await;
    let mut late = event("late");
    late.occurred_at -= 10000;
    assert_eq!(
        attention::reserve(&s, "p", "c", &late, None).unwrap()["reason"],
        "superseded"
    );
    assert_eq!(
        s.get_record("attention_events", a["id"].as_str().unwrap())
            .unwrap()
            .unwrap()["status"],
        "offered"
    );
    let b = offer(s.clone(), "b").await;
    assert!(attention::consume(&s, "p", "c", "session", &a).is_err());
    attention::cancel(&s, "c", Some("session"));
    assert!(attention::consume(&s, "p", "c", "session", &b).is_err());
    let c = offer(s.clone(), "c").await;
    s.config
        .write()
        .unwrap()
        .config
        .attention
        .as_mut()
        .unwrap()
        .model = "changed".into();
    assert!(attention::consume(&s, "p", "c", "session", &c).is_err());
    server.abort();
}
#[tokio::test]
async fn in_flight_cancellation_and_restart_do_not_reissue_event() {
    let (url, count, server) = provider(answer("engage"), 80).await;
    let (h, s) = install(url, 10);
    let e = event("e");
    let row = attention::reserve(&s, "p", "c", &e, None).unwrap();
    let state = s.clone();
    let job = tokio::spawn(async move { attention::evaluate(state, row, e).await.unwrap() });
    while count.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    attention::cancel(&s, "c", None);
    assert_eq!(job.await.unwrap()["action"], "hold");
    let reopened = AppState::open(h.path().into()).unwrap();
    assert_eq!(
        attention::reserve(&reopened, "p", "new", &event("e"), None).unwrap()["reason"],
        "duplicate"
    );
    server.abort();
}
#[test]
fn scoped_context_and_freshness_checked_before_provider_access() {
    let (_h, s) = install("http://127.0.0.1:9/".into(), 10);
    let mut e = event("e");
    assert!(
        attention::reserve(
            &s,
            "p",
            "c",
            &e,
            Some(&json!({"conversation_id":"private","thread_id":""}))
        )
        .is_err()
    );
    e.occurred_at -= 700000;
    assert_eq!(
        attention::reserve(&s, "p", "c", &e, None).unwrap()["reason"],
        "stale"
    );
    e.context.messages[0].sender = "agent".into();
    assert!(attention::reserve(&s, "p", "c", &e, None).is_err());
}

#[test]
fn owner_scope_is_not_a_synthetic_room_and_zero_population_is_unknown() {
    let (_h, s) = install("http://127.0.0.1:9/".into(), 10);
    assert!(attention::report(&s).unwrap()["semantic_uncertainty"]["rate"].is_null());
    let e = event("owner-event");
    assert_eq!(
        attention::reserve(
            &s,
            "p",
            "c",
            &e,
            Some(&json!({"owner":true,"actor_id":"owner"}))
        )
        .unwrap()["status"],
        "evaluating"
    );
}
