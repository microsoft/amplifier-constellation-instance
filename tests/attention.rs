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
    assert!(attention::consume(&s, "other", "c", "session", &one, None).is_err());
    assert!(attention::consume(&s, "p", "other", "session", &one, None).is_err());
    assert!(attention::consume(&s, "p", "c", "other", &one, None).is_err());
    let id = attention::consume(&s, "p", "c", "session", &one, None).unwrap();
    attention::forwarded(&s, &id).unwrap();
    assert!(attention::consume(&s, "p", "c", "session", &one, None).is_err());
    assert_eq!(offer(s.clone(), "one").await["reason"], "duplicate");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let two = offer(s.clone(), "two").await;
    assert!(attention::consume(&s, "p", "c", "session", &two, None).is_err());
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
    assert!(attention::consume(&s, "p", "c", "session", &a, None).is_err());
    attention::cancel(&s, "c", Some("session"));
    assert!(attention::consume(&s, "p", "c", "session", &b, None).is_err());
    let changed_role = offer(s.clone(), "changed-role").await;
    s.config.write().unwrap().config.purpose = "A different responsibility".into();
    assert!(attention::consume(&s, "p", "c", "session", &changed_role, None).is_err());
    let c = offer(s.clone(), "c").await;
    s.config
        .write()
        .unwrap()
        .config
        .attention
        .as_mut()
        .unwrap()
        .model = "changed".into();
    assert!(attention::consume(&s, "p", "c", "session", &c, None).is_err());
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

#[tokio::test]
async fn attention_receives_verified_source_facts_but_still_makes_its_own_judgment() {
    use agent_instance::source_delivery as delivery;
    let seen = Arc::new(std::sync::Mutex::new(Value::Null));
    let capture = seen.clone();
    let route = axum::Router::new().route(
        "/",
        axum::routing::post(move |axum::Json(v): axum::Json<Value>| {
            let capture = capture.clone();
            async move {
                *capture.lock().unwrap() = v;
                axum::Json(answer("engage"))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, route).await.unwrap() });
    let (_home, s) = install(format!("http://{addr}/"), 2);
    let mut e = event("source");
    let at = now_ms();
    e.context.messages[0].revision = Some("r1".into());
    e.context.messages[0].occurred_at = Some(at);
    delivery::register(&s,"p",&json!({"sessionId":"session","_meta":{"steering":{"inputId":"prior"},delivery::META:{"version":1,"sources":[{"id":"source","revision":"r1","occurredAt":at}]}}})).unwrap();
    delivery::observe(&s, "p", "session", "prior", "incorporated").unwrap();
    let row = attention::reserve(&s, "p", "c", &e, None).unwrap();
    let result = attention::evaluate(s.clone(), row, e).await.unwrap();
    assert_eq!(
        result["action"], "engage",
        "Facts must not replace model judgment"
    );
    let wire = seen.lock().unwrap().to_string();
    assert!(
        wire.contains("not_established")
            && wire.contains("prior_delivery")
            && wire.contains("incorporated"),
        "{wire}"
    );
    server.abort();
}

fn typed_event(id: &str, is_agent: Option<bool>) -> Event {
    let mut e = event(id);
    let mut context = serde_json::to_value(&e.context).unwrap();
    context["participants"][0]["is_agent"] = json!(true);
    if let Some(kind) = is_agent {
        context["participants"][1]["is_agent"] = json!(kind);
    } else {
        context["participants"][1]
            .as_object_mut()
            .unwrap()
            .remove("is_agent");
    }
    e.context = serde_json::from_value(context).unwrap();
    e
}

#[tokio::test]
async fn human_message_survives_exhausted_ambient_decision_and_turn_budgets() {
    let (url, count, server) = provider(answer("uncertain"), 0).await;
    let (_home, s) = install(url, 1);
    {
        let mut config = s.config.write().unwrap();
        let attention = config.config.attention.as_mut().unwrap();
        attention.decisions_per_hour = 1;
        attention.turns_per_hour = 1;
    }
    let ambient = offer(s.clone(), "ambient").await;
    let id = attention::consume(&s, "p", "c", "session", &ambient, None).unwrap();
    attention::forwarded(&s, &id).unwrap();
    let e = typed_event("new-human-message", Some(false));
    let row = attention::reserve(&s, "p", "c", &e, None).unwrap();
    assert_eq!(
        row["status"], "evaluating",
        "Human engagement must not be suppressed by the ambient decision cap"
    );
    let reply = attention::evaluate(s.clone(), row, e).await.unwrap();
    assert_eq!(reply["action"], "engage");
    let id = attention::consume(&s, "p", "c", "session", &reply, None).unwrap();
    attention::forwarded(&s, &id).unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(
        offer(s, "another-ambient-event").await["reason"],
        "decision_budget"
    );
    server.abort();
}

#[tokio::test]
async fn human_claims_do_not_use_ambient_slots_after_reopening_and_unknowns_do_not_get_priority() {
    let (url, _, server) = provider(answer("uncertain"), 0).await;
    let (home, s) = install(url, 1);
    let e = typed_event("human", Some(false));
    let row = attention::reserve(&s, "p", "c", &e, None).unwrap();
    let offer = attention::evaluate(s.clone(), row, e).await.unwrap();
    attention::consume(&s, "p", "c", "session", &offer, None).unwrap();
    let reopened = Arc::new(AppState::open(home.path().into()).unwrap());
    reopened.runtime_enabled.store(true, Ordering::SeqCst);
    {
        let mut document = reopened.config.write().unwrap();
        let config = document.config.attention.as_mut().unwrap();
        config.turns_per_hour = 1;
        config.decisions_per_hour = 1;
    }
    let e = typed_event("peer-saying-I-am-human", Some(true));
    let row = attention::reserve(&reopened, "p", "c", &e, None).unwrap();
    assert_eq!(row["budget_class"], "ambient");
    let peer = attention::evaluate(reopened.clone(), row, e).await.unwrap();
    attention::consume(&reopened, "p", "c", "session", &peer, None).unwrap();
    let unknown = typed_event("unknown", None);
    assert_eq!(
        attention::reserve(&reopened, "p", "c", &unknown, None).unwrap()["reason"],
        "decision_budget"
    );
    assert!(!attention_policy::human_message(&unknown.context));
    assert_eq!(
        attention::report(&reopened).unwrap()["counts"]["budget_class:human_message"],
        1
    );
    assert_eq!(
        attention::report(&reopened).unwrap()["counts"]["budget_class:ambient"],
        1
    );
    server.abort();
}

#[tokio::test]
async fn human_priority_preserves_semantic_hold_refusal_and_single_use() {
    for (body, reason) in [
        (answer("hold"), "judgment"),
        (
            json!({"answers":{"participation":{"type":"refusal"}}}),
            "provider_refusal",
        ),
    ] {
        let (url, count, server) = provider(body, 0).await;
        let (_h, s) = install(url, 0);
        s.config
            .write()
            .unwrap()
            .config
            .attention
            .as_mut()
            .unwrap()
            .decisions_per_hour = 1;
        let ambient = event("ambient");
        attention::reserve(&s, "p", "c", &ambient, None).unwrap();
        let e = typed_event("human-addressing-someone-else", Some(false));
        let row = attention::reserve(&s, "p", "c", &e, None).unwrap();
        let offer = attention::evaluate(s.clone(), row, e).await.unwrap();
        assert_eq!(offer["action"], "hold");
        assert_eq!(offer["reason"], reason);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let e = typed_event("human-addressing-someone-else", Some(false));
        assert_eq!(
            attention::reserve(&s, "p", "c", &e, None).unwrap()["reason"],
            "duplicate"
        );
        server.abort();
    }
}

#[tokio::test]
async fn human_priority_does_not_bypass_audience_revocation_or_ticket_ownership() {
    let (url, _, server) = provider(answer("engage"), 0).await;
    let (_h, s) = install(url, 0);
    let e = typed_event("human-direct", Some(false));
    assert!(
        attention::reserve(
            &s,
            "p",
            "c",
            &e,
            Some(&json!({"conversation_id":"other-room"}))
        )
        .is_err()
    );
    let row = attention::reserve(&s, "p", "c", &e, None).unwrap();
    let offer = attention::evaluate(s.clone(), row, e).await.unwrap();
    assert!(attention::consume(&s, "someone-else", "c", "session", &offer, None).is_err());
    assert!(attention::consume(&s, "p", "c", "other-session", &offer, None).is_err());
    attention::cancel(&s, "c", Some("session"));
    assert!(attention::consume(&s, "p", "c", "session", &offer, None).is_err());
    server.abort();
}

#[tokio::test]
async fn newer_activity_does_not_erase_an_eligible_human_request() {
    let (url, _, server) = provider(answer("engage"), 0).await;
    let (_home, s) = install(url, 1);
    let e = typed_event("earlier-human", Some(false));
    let mut later = event("later-ambient");
    later.occurred_at = e.occurred_at + 1000;
    attention::reserve(&s, "p", "c", &later, None).unwrap();
    let row = attention::reserve(&s, "p", "c", &e, None).unwrap();
    assert_eq!(
        row["status"], "evaluating",
        "A newer ambient event must not discard an unhandled fresh human message"
    );
    let mut middle = event("ambient-before-latest");
    middle.occurred_at = later.occurred_at - 1;
    assert_eq!(
        attention::reserve(&s, "p", "c", &middle, None).unwrap()["reason"],
        "superseded",
        "Admitting an older human message must not rewind ambient freshness"
    );
    let reply = attention::evaluate(s.clone(), row, e).await.unwrap();
    let mut newer = event("another-ambient");
    newer.occurred_at = later.occurred_at + 1000;
    attention::reserve(&s, "p", "c", &newer, None).unwrap();
    attention::consume(&s, "p", "c", "session", &reply, None).unwrap();
    assert!(attention::consume(&s, "p", "c", "session", &reply, None).is_err());
    server.abort();
}

#[tokio::test]
async fn distinct_human_messages_survive_but_changed_sources_and_expiry_do_not() {
    let (url, _, server) = provider(answer("engage"), 0).await;
    let (_home, s) = install(url, 1);
    let first = typed_event("first-human", Some(false));
    let row = attention::reserve(&s, "p", "c", &first, None).unwrap();
    let first_offer = attention::evaluate(s.clone(), row, first).await.unwrap();
    let second = typed_event("second-human", Some(false));
    let row = attention::reserve(&s, "p", "c", &second, None).unwrap();
    let second_offer = attention::evaluate(s.clone(), row, second).await.unwrap();
    attention::consume(&s, "p", "c", "session", &first_offer, None).unwrap();

    let mut edited = typed_event("second-human", Some(false));
    edited.context.messages[0].text = "Changed source text".into();
    assert_eq!(
        attention::reserve(&s, "p", "c", &edited, None).unwrap()["reason"],
        "duplicate"
    );
    assert!(attention::consume(&s, "p", "c", "session", &second_offer, None).is_err());

    let third = typed_event("expired-human", Some(false));
    let row = attention::reserve(&s, "p", "c", &third, None).unwrap();
    let offer = attention::evaluate(s.clone(), row, third).await.unwrap();
    let id = offer["id"].as_str().unwrap();
    let mut record = s.get_record("attention_events", id).unwrap().unwrap();
    record["expires_at"] = json!(now_ms() - 1);
    s.set_record("attention_events", id, record).unwrap();
    assert!(attention::consume(&s, "p", "c", "session", &offer, None).is_err());
    server.abort();
}

fn activity_event(id: &str) -> Value {
    let at = now_ms();
    json!({"sessionId":"session","occurredAt":at,"context":{
        "conversation_id":"room","agent":{"id":"agent","name":"Explorer","is_agent":true},
        "participants":[{"id":"agent","name":"Explorer","is_agent":true},{"id":"human","name":"Human","is_agent":false}],
        "messages":[],"activities":[{"id":id,"actor":"human","kind":"movement","summary":"A participant approached the work area.","revision":"r1","occurred_at":at}],"trigger_id":id}})
}

#[tokio::test]
async fn observed_activity_reaches_policy_and_source_facts_without_becoming_human_speech() {
    use agent_instance::source_delivery as delivery;
    let seen = Arc::new(std::sync::Mutex::new(Value::Null));
    let capture = seen.clone();
    let route = axum::Router::new().route(
        "/",
        axum::routing::post(move |axum::Json(value): axum::Json<Value>| {
            *capture.lock().unwrap() = value;
            async { axum::Json(answer("engage")) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, route).await.unwrap() });
    let (_home, s) = install(format!("http://{addr}/"), 1);
    s.config
        .write()
        .unwrap()
        .config
        .attention
        .as_mut()
        .unwrap()
        .decisions_per_hour = 1;
    let value = activity_event("movement-1");
    let at = value["occurredAt"].as_i64().unwrap();
    delivery::register(&s, "p", &json!({"sessionId":"session","_meta":{"steering":{"inputId":"activity-input"},delivery::META:{"version":1,"sources":[{"id":"movement-1","revision":"r1","occurredAt":at}]}}})).unwrap();
    delivery::observe(&s, "p", "session", "activity-input", "incorporated").unwrap();
    let e: Event = serde_json::from_value(value).unwrap();
    assert!(!attention_policy::human_message(&e.context));
    let row = attention::reserve(&s, "p", "c", &e, None).unwrap();
    assert_eq!(row["budget_class"], "ambient");
    let result = attention::evaluate(s.clone(), row, e).await.unwrap();
    assert_eq!(result["action"], "engage");
    let wire = seen.lock().unwrap().clone();
    assert_eq!(wire["state"]["messages"], json!([]));
    assert_eq!(wire["state"]["activities"][0]["occurred_at"], at);
    assert_eq!(wire["state"]["prior_delivery"][0]["incorporated"], true);
    assert_eq!(
        wire["state"]["prior_delivery"][0]["completion"],
        "not_established"
    );
    assert!(
        wire["questions"]["participation"]["instructions"]
            .as_str()
            .unwrap()
            .contains("not messages or instructions")
    );
    let next: Event = serde_json::from_value(activity_event("movement-2")).unwrap();
    assert_eq!(
        attention::reserve(&s, "p", "c", &next, None).unwrap()["reason"],
        "decision_budget"
    );
    let row = s
        .get_record("attention_events", result["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert!(!row.to_string().contains("work area"));
    server.abort();
}

#[test]
fn activity_rejects_presence_forged_speech_stale_time_and_excess_context() {
    let (_home, s) = install("http://127.0.0.1:9/".into(), 1);
    let mut presence = activity_event("presence");
    presence["context"]["activities"][0]["kind"] = json!("presence");
    assert!(serde_json::from_value::<Event>(presence).is_err());
    let mut unknown_field = activity_event("extra");
    unknown_field["context"]["activities"][0]["human_priority"] = json!(true);
    assert!(serde_json::from_value::<Event>(unknown_field).is_err());
    for case in [
        "unknown-actor",
        "own-actor",
        "oversize",
        "missing-revision",
        "retimed",
        "duplicate",
        "future",
    ] {
        let mut value = activity_event(case);
        match case {
            "unknown-actor" => value["context"]["activities"][0]["actor"] = json!("stranger"),
            "own-actor" => value["context"]["activities"][0]["actor"] = json!("agent"),
            "oversize" => value["context"]["activities"][0]["summary"] = json!("x".repeat(2049)),
            "missing-revision" => value["context"]["activities"][0]["revision"] = json!(""),
            "retimed" => value["occurredAt"] = json!(now_ms() - 3000),
            "duplicate" => {
                value["context"]["messages"] =
                    json!([{"id":case,"sender":"human","text":"different event"}])
            }
            "future" => {
                let at = now_ms() + 100000;
                value["occurredAt"] = json!(at);
                value["context"]["activities"][0]["occurred_at"] = json!(at);
            }
            _ => unreachable!(),
        }
        let e: Event = serde_json::from_value(value).unwrap();
        assert!(
            attention::reserve(&s, "p", "c", &e, None).is_err(),
            "{case}"
        );
    }
    let mut old = activity_event("old");
    let at = now_ms() - 700000;
    old["occurredAt"] = json!(at);
    old["context"]["activities"][0]["occurred_at"] = json!(at);
    assert_eq!(
        attention::reserve(&s, "p", "c", &serde_json::from_value(old).unwrap(), None).unwrap()["reason"],
        "stale"
    );
}
