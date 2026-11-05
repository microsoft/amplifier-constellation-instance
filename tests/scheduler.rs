use agent_instance::{
    auth,
    config::{BackendConfig, ConfigDocument, InstanceConfig, private_dir, write_private},
    scheduler, server,
    state::AppState,
    work,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tower::ServiceExt;
struct Fixture {
    dir: tempfile::TempDir,
    state: Arc<AppState>,
    router: Router,
    token: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for name in ["work", "profiles", "secrets"] {
            private_dir(&dir.path().join(name)).unwrap();
        }
        let mut config = InstanceConfig::blank("Scheduled individual".into(), "test-owner".into());
        config.enabled = true;
        config.setup_complete = true;
        config.backend = Some(BackendConfig {
            command: agent_instance::platform::python().into(),
            args: vec![
                format!(
                    "{}/tests/fixtures/scheduler_backend.py",
                    env!("CARGO_MANIFEST_DIR")
                ),
                "${AGENT_HOME}/work".into(),
            ],
            env: BTreeMap::new(),
            secret_env: BTreeMap::new(),
            working_directory: "${AGENT_HOME}/work".into(),
            allowed_working_roots: vec!["${AGENT_HOME}/work".into()],
        });
        write_private(
            &dir.path().join("config.json"),
            &serde_json::to_vec(&ConfigDocument {
                revision: 1,
                config,
            })
            .unwrap(),
        )
        .unwrap();
        let state = Arc::new(AppState::open(dir.path().into()).unwrap());
        let token =
            auth::issue_token(&state, "operator", &["admin".into()], 3600).unwrap()["token"]
                .as_str()
                .unwrap()
                .into();
        let router = server::router(state.clone());
        Self {
            dir,
            state,
            router,
            token,
        }
    }
    async fn request(&self, method: &str, path: &str, body: Value, token: &str) -> (u16, Value) {
        let response = self
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
        let status = response.status().as_u16();
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    async fn api(&self, method: &str, path: &str, body: Value) -> Value {
        let (s, v) = self.request(method, path, body, &self.token).await;
        assert_eq!(s, 200, "{path}: {v}");
        v
    }
    fn enable(&self, paused: bool) {
        self.enable_mode(paused, None);
    }
    fn enable_mode(&self, paused: bool, mode: Option<&str>) {
        let current = scheduler::inspect(&self.state).unwrap();
        scheduler::configure(
            &self.state,
            scheduler::Settings {
                expected_revision: current["settings"]["revision"].as_u64().unwrap(),
                enabled: true,
                paused,
                concurrency: 1,
                approved_backend_sha256: Some(scheduler::backend_hash(&self.state)),
                session_mode: mode.map(str::to_owned),
            },
            "operator",
        )
        .unwrap();
    }
    fn create(&self, goal: &str, accept: bool) -> Value {
        let mut w = work::create_work(
            &self.state,
            serde_json::from_value(json!({"title":"Fixture local work","goal":goal})).unwrap(),
            "operator",
            None,
        )
        .unwrap();
        if accept {
            w = work::command_work(
                &self.state,
                w["id"].as_str().unwrap(),
                serde_json::from_value(
                    json!({"expected_revision":w["revision"],"command":{"type":"accept"}}),
                )
                .unwrap(),
                "operator",
                None,
            )
            .unwrap();
        }
        w
    }
    fn queue(&self, w: &Value, budget: Value) -> Value {
        scheduler::command(&self.state,w["id"].as_str().unwrap(),serde_json::from_value(json!({"type":"queue","expected_work_revision":w["revision"],"request_id":format!("request-{}",w["id"].as_str().unwrap()),"budget":budget})).unwrap(),"operator").unwrap()
    }
    async fn wait(&self, run: &Value, states: &[&str]) -> Value {
        for _ in 0..200 {
            let r = self
                .state
                .get_record("work_execution", run["id"].as_str().unwrap())
                .unwrap()
                .unwrap();
            if states.contains(&r["state"].as_str().unwrap()) {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "Timed out: {:?}",
            self.state
                .get_record("work_execution", run["id"].as_str().unwrap())
        );
    }
}
fn budget() -> Value {
    json!({"max_seconds":10,"max_output_bytes":8192,"max_tool_calls":10})
}
#[tokio::test]
async fn detached_acp_execution_persists_report_and_requests_review_not_completion() {
    let f = Fixture::new();
    f.enable(false);
    let w = f.create("REVISED write and check a local fixture artifact", true);
    // Authorize through HTTP then drop the response/client. The service loop owns execution.
    let r=f.api("POST",&format!("work/{}/execution",w["id"].as_str().unwrap()),json!({"type":"queue","request_id":"detached","expected_work_revision":w["revision"],"budget":budget()})).await;
    let (stop, task) = scheduler::start(f.state.clone()).unwrap();
    let done = f
        .wait(
            &r,
            &["reported", "needs_reconciliation", "failed_before_prompt"],
        )
        .await;
    assert_eq!(done["state"], "reported", "{done}");
    assert!(
        done["report"]
            .as_str()
            .unwrap()
            .contains("Created result.txt")
    );
    assert!(!done["report"].as_str().unwrap().contains("OBSOLETE"));
    assert!(!done["report"].as_str().unwrap().contains("LATE OLD"));
    assert!(done["output_bytes"].as_u64().unwrap() > done["report"].as_str().unwrap().len() as u64);
    assert_eq!(done["report_trust"], "agent_report_unverified");
    let work = work::get_work(&f.state, w["id"].as_str().unwrap(), None).unwrap();
    assert_eq!(work["state"], "review");
    assert!(work["verification"].is_null());
    assert_eq!(work["evidence"][0]["trust"], "agent_report_unverified");
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("work/result.txt")).unwrap(),
        "actual fixture artifact\n"
    );
    let init: Value =
        serde_json::from_slice(&std::fs::read(f.dir.path().join("work/initialize.json")).unwrap())
            .unwrap();
    assert_eq!(init["clientCapabilities"], json!({}));
    let session: Value =
        serde_json::from_slice(&std::fs::read(f.dir.path().join("work/session.json")).unwrap())
            .unwrap();
    assert_eq!(session["mcpServers"], json!([]));
    assert!(done["session_mode"].is_null());
    assert_eq!(done["mode_applied"], false);
    assert!(!f.dir.path().join("work/mode.json").exists());
    let restarted = AppState::open(f.dir.path().into()).unwrap();
    assert_eq!(
        scheduler::inspect_work(&restarted, w["id"].as_str().unwrap(), None).unwrap()["runs"][0]["state"],
        "reported"
    );
    stop.send(()).unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn scopes_revision_dedupe_and_acceptance_are_separate_from_execution_authority() {
    let f = Fixture::new();
    let reader = auth::issue_token(&f.state, "reader", &["work:read".into()], 3600).unwrap();
    let writer = auth::issue_token(&f.state, "writer", &["work".into()], 3600).unwrap();
    let executor = auth::issue_token(&f.state, "executor", &["work:execute".into()], 3600).unwrap();
    for t in [
        reader["token"].as_str().unwrap(),
        writer["token"].as_str().unwrap(),
    ] {
        assert_eq!(f.request("GET", "scheduler", Value::Null, t).await.0, 200);
        assert_eq!(f.request("PUT","scheduler",json!({"expected_revision":0,"enabled":true,"paused":false,"concurrency":1,"approved_backend_sha256":scheduler::backend_hash(&f.state)}),t).await.0,403);
    }
    assert_eq!(
        f.request(
            "POST",
            "work",
            json!({"title":"x","goal":"y"}),
            executor["token"].as_str().unwrap()
        )
        .await
        .0,
        403
    );
    f.enable(true);
    let w = f.create("proposed", false);
    let path = format!("work/{}/execution", w["id"].as_str().unwrap());
    let body = json!({"type":"queue","request_id":"one","expected_work_revision":w["revision"],"budget":budget()});
    assert_eq!(f.request("POST", &path, body, &f.token).await.0, 400);
    let w = f.create("accepted", true);
    let path = format!("work/{}/execution", w["id"].as_str().unwrap());
    let body = json!({"type":"queue","request_id":"one","expected_work_revision":w["revision"],"budget":budget()});
    let one = f.api("POST", &path, body.clone()).await;
    let two = f.api("POST", &path, body.clone()).await;
    assert_eq!(one, two);
    let mut conflict = body.clone();
    conflict["budget"]["max_seconds"] = json!(99);
    assert_eq!(f.request("POST", &path, conflict, &f.token).await.0, 400);
    let (stop, task) = scheduler::start(f.state.clone()).unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        f.state
            .get_record("work_execution", one["id"].as_str().unwrap())
            .unwrap()
            .unwrap()["state"],
        "queued"
    );
    assert!(!f.dir.path().join("work/prompts.log").exists());
    f.enable(false);
    f.wait(&one, &["reported"]).await;
    stop.send(()).unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn interruption_never_replays_side_effects_without_reconciliation() {
    let f = Fixture::new();
    f.enable(false);
    let w = f.create("SLOW_RUN", true);
    let run = f.queue(&w, budget());
    let (stop, task) = scheduler::start(f.state.clone()).unwrap();
    f.wait(&run, &["running"]).await;
    // Terminating the service leaves a durable ambiguous result, never an automatically queued retry.
    stop.send(()).unwrap();
    task.await.unwrap();
    let interrupted = f.wait(&run, &["needs_reconciliation"]).await;
    let reopened = Arc::new(AppState::open(f.dir.path().into()).unwrap());
    let (stop, task) = scheduler::start(reopened.clone()).unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        scheduler::inspect(&reopened).unwrap()["counts"]["needs_reconciliation"],
        1
    );
    let current = work::get_work(&reopened, w["id"].as_str().unwrap(), None).unwrap();
    let path = format!("work/{}/execution", w["id"].as_str().unwrap());
    let retry = json!({"type":"retry","run_id":run["id"],"expected_revision":interrupted["revision"],"expected_work_revision":current["revision"],"request_id":"retry-1","reason":"Inspected artifacts","budget":budget()});
    assert_eq!(f.request("POST", &path, retry, &f.token).await.0, 400);
    let reconciled=f.api("POST",&path,json!({"type":"reconcile","run_id":run["id"],"expected_revision":interrupted["revision"],"note":"Checked the workspace; no final artifact was produced. Retrying is authorized.","allow_retry":true})).await;
    assert_eq!(
        reconciled["reconciliation"]["side_effects_independently_verified"],
        false
    );
    let changed=work::command_work(&reopened,w["id"].as_str().unwrap(),serde_json::from_value(json!({"expected_revision":current["revision"],"command":{"type":"amend","title":"Continue checked fixture","goal":"Write fixture artifact"}})).unwrap(),"operator",None).unwrap();
    let new=f.api("POST",&path,json!({"type":"retry","run_id":run["id"],"expected_revision":reconciled["revision"],"expected_work_revision":changed["revision"],"request_id":"retry-2","reason":"Resume with revised local goal after inspection","budget":budget()})).await;
    assert_ne!(new["id"], run["id"]);
    assert_eq!(new["retry_of"], run["id"]);
    for _ in 0..200 {
        if reopened
            .get_record("work_execution", new["id"].as_str().unwrap())
            .unwrap()
            .unwrap()["state"]
            == "reported"
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        reopened
            .get_record("work_execution", new["id"].as_str().unwrap())
            .unwrap()
            .unwrap()["state"],
        "reported"
    );
    stop.send(()).unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn time_output_tool_and_approval_limits_stop_with_explicit_uncertainty() {
    for (goal, budget, reason) in [
        (
            "SLOW_RUN",
            json!({"max_seconds":1,"max_output_bytes":8192,"max_tool_calls":10}),
            "time_budget",
        ),
        (
            "OUTPUT_BUDGET",
            json!({"max_seconds":10,"max_output_bytes":1024,"max_tool_calls":10}),
            "output_budget",
        ),
        (
            "TOOL_BUDGET",
            json!({"max_seconds":10,"max_output_bytes":8192,"max_tool_calls":1}),
            "tool_budget",
        ),
        ("NEEDS_APPROVAL", budget(), "permission_required"),
    ] {
        let f = Fixture::new();
        f.enable(false);
        let w = f.create(goal, true);
        let r = f.queue(&w, budget);
        let (stop, task) = scheduler::start(f.state.clone()).unwrap();
        let done = f
            .wait(
                &r,
                &["needs_reconciliation", "failed_before_prompt", "reported"],
            )
            .await;
        assert_eq!(done["state"], "needs_reconciliation", "{done}");
        assert_eq!(done["reason"], reason);
        assert_ne!(
            work::get_work(&f.state, w["id"].as_str().unwrap(), None).unwrap()["state"],
            "completed"
        );
        stop.send(()).unwrap();
        task.await.unwrap();
    }
}
#[tokio::test]
async fn changed_goal_or_backend_cancels_old_authorization_and_scoped_tools_cannot_approve() {
    let f = Fixture::new();
    f.enable(true);
    let w = f.create("SLOW_RUN", true);
    let run = f.queue(&w, budget());
    work::command_work(&f.state,w["id"].as_str().unwrap(),serde_json::from_value(json!({"expected_revision":w["revision"],"command":{"type":"amend","title":"New goal","goal":"Different authorized work"}})).unwrap(),"operator",None).unwrap();
    f.enable(false);
    let (stop, task) = scheduler::start(f.state.clone()).unwrap();
    assert_eq!(
        f.wait(&run, &["failed_before_prompt"]).await["reason"],
        "authorization_changed"
    );
    assert!(!f.dir.path().join("work/prompts.log").exists());
    let room = json!({"owner":false,"provider_id":"slack","conversation_id":"other"});
    assert!(work::mcp_call(&f.state, &room, "work_execution", &json!({"id":w["id"]})).is_err());
    assert!(
        work::mcp_call(
            &f.state,
            &json!({"owner":true}),
            "work_execute",
            &json!({"id":w["id"]})
        )
        .is_err()
    );
    let w = f.create("next", true);
    f.enable(true);
    let run = f.queue(&w, budget());
    f.state
        .config
        .write()
        .unwrap()
        .config
        .backend
        .as_mut()
        .unwrap()
        .env
        .insert("NEW_BEHAVIOR".into(), "1".into());
    // A paused settings edit can reapprove the backend, but old runs retain their prior hash.
    f.enable(false);
    assert_eq!(
        f.wait(&run, &["failed_before_prompt"]).await["reason"],
        "authorization_changed"
    );
    stop.send(()).unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn explicit_cancel_and_goal_revision_interrupt_active_run() {
    for amended in [false, true] {
        let f = Fixture::new();
        f.enable(false);
        let w = f.create("SLOW_RUN", true);
        let r = f.queue(&w, budget());
        let (stop, task) = scheduler::start(f.state.clone()).unwrap();
        let active = f.wait(&r, &["running"]).await;
        if amended {
            let w = work::get_work(&f.state, w["id"].as_str().unwrap(), None).unwrap();
            work::command_work(&f.state,w["id"].as_str().unwrap(),serde_json::from_value(json!({"expected_revision":w["revision"],"command":{"type":"amend","title":"Changed","goal":"New goal"}})).unwrap(),"operator",None).unwrap();
        } else {
            f.api("POST",&format!("work/{}/execution",w["id"].as_str().unwrap()),json!({"type":"cancel","run_id":r["id"],"expected_revision":active["revision"],"reason":"Stop this attempt"})).await;
        }
        let done = f.wait(&r, &["needs_reconciliation"]).await;
        assert_eq!(
            done["reason"],
            if amended {
                "work_changed"
            } else {
                "cancel_requested"
            }
        );
        stop.send(()).unwrap();
        task.await.unwrap();
    }
}

#[tokio::test]
async fn queue_is_bounded_and_cancel_before_start_has_no_uncertain_side_effects() {
    let f = Fixture::new();
    f.enable(true);
    let mut first = None;
    for _ in 0..64 {
        let w = f.create("Queued fixture", true);
        let r = f.queue(&w, budget());
        if first.is_none() {
            first = Some((w, r));
        }
    }
    let overflow = f.create("Beyond queue capacity", true);
    assert!(scheduler::command(&f.state,overflow["id"].as_str().unwrap(),serde_json::from_value(json!({"type":"queue","expected_work_revision":overflow["revision"],"request_id":"overflow","budget":budget()})).unwrap(),"operator").is_err());
    let (w, r) = first.unwrap();
    let cancelled=f.api("POST",&format!("work/{}/execution",w["id"].as_str().unwrap()),json!({"type":"cancel","run_id":r["id"],"expected_revision":r["revision"],"reason":"Withdraw unused authorization"})).await;
    assert_eq!(cancelled["state"], "cancelled");
    assert_eq!(cancelled["prompt_may_have_run"], false);
    assert!(!f.dir.path().join("work/prompts.log").exists());
    let queued = f.queue(&overflow, budget());
    assert_eq!(queued["state"], "queued");
}

#[tokio::test]
async fn pausing_allows_current_work_to_finish_but_serial_queue_does_not_start_another() {
    let f = Fixture::new();
    f.enable(false);
    let one = f.queue(&f.create("DELAY_RUN first", true), budget());
    let two = f.queue(&f.create("DELAY_RUN second", true), budget());
    let (stop, task) = scheduler::start(f.state.clone()).unwrap();
    f.wait(&one, &["running"]).await;
    f.enable(true);
    assert_eq!(
        f.state
            .get_record("work_execution", two["id"].as_str().unwrap())
            .unwrap()
            .unwrap()["state"],
        "queued"
    );
    f.wait(&one, &["reported"]).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        f.state
            .get_record("work_execution", two["id"].as_str().unwrap())
            .unwrap()
            .unwrap()["state"],
        "queued"
    );
    f.enable(false);
    f.wait(&two, &["reported"]).await;
    let events = f.api("GET", "events", Value::Null).await.to_string();
    assert!(!events.contains("Created result.txt"));
    assert!(!events.contains("DELAY_RUN"));
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn explicit_mode_is_applied_before_prompt_and_authorization_survives_settings_changes() {
    let f = Fixture::new();
    f.enable_mode(true, Some("workspace"));
    let first = f.queue(&f.create("DELAY_RUN with a captured mode", true), budget());
    assert_eq!(first["session_mode"], "workspace");
    // A settings change before startup must not silently change an existing authorization.
    f.enable_mode(false, Some("read_only"));
    let (stop, task) = scheduler::start(f.state.clone()).unwrap();
    let active = f.wait(&first, &["running"]).await;
    assert_eq!(active["mode_applied"], true);
    let applied: Value =
        serde_json::from_slice(&std::fs::read(f.dir.path().join("work/mode.json")).unwrap())
            .unwrap();
    assert_eq!(applied["modeId"], "workspace");
    // Nor may settings alter the mode of an active session.
    f.enable_mode(false, Some("ask"));
    let done = f.wait(&first, &["reported"]).await;
    assert_eq!(done["session_mode"], "workspace");
    let prompt_mode: Value =
        serde_json::from_slice(&std::fs::read(f.dir.path().join("work/prompt-mode.json")).unwrap())
            .unwrap();
    assert_eq!(prompt_mode["mode"], "workspace");
    let next = f.queue(&f.create("A newly authorized run", true), budget());
    assert_eq!(next["session_mode"], "ask");
    let done = f.wait(&next, &["reported"]).await;
    assert_eq!(done["mode_applied"], true);
    let prompt_mode: Value =
        serde_json::from_slice(&std::fs::read(f.dir.path().join("work/prompt-mode.json")).unwrap())
            .unwrap();
    assert_eq!(prompt_mode["mode"], "ask");
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn unsupported_or_rejected_modes_fail_without_sending_the_work_prompt() {
    for (mode, env, reason, called) in [
        ("unavailable", None, "unsupported_session_mode", false),
        (
            "workspace",
            Some("NO_MODES"),
            "unsupported_session_mode",
            false,
        ),
        ("workspace", Some("REJECT_MODE"), "backend_error", true),
    ] {
        let f = Fixture::new();
        if let Some(env) = env {
            f.state
                .config
                .write()
                .unwrap()
                .config
                .backend
                .as_mut()
                .unwrap()
                .env
                .insert(env.into(), "1".into());
        }
        f.enable_mode(false, Some(mode));
        let w = f.create("Do not start until the chosen mode is available", true);
        let queued = f.queue(&w, budget());
        let (stop, task) = scheduler::start(f.state.clone()).unwrap();
        let failed = f.wait(&queued, &["failed_before_prompt", "reported"]).await;
        assert_eq!(failed["state"], "failed_before_prompt", "{failed}");
        assert_eq!(failed["reason"], reason);
        assert_eq!(failed["mode_applied"], false);
        assert_eq!(failed["prompt_may_have_run"], false);
        assert_eq!(f.dir.path().join("work/mode.json").exists(), called);
        assert!(!f.dir.path().join("work/prompts.log").exists());
        assert!(!f.dir.path().join("work/result.txt").exists());
        assert_eq!(
            work::get_work(&f.state, w["id"].as_str().unwrap(), None).unwrap()["state"],
            "accepted"
        );
        stop.send(()).unwrap();
        task.await.unwrap();
    }
}

#[tokio::test]
async fn unsolicited_mode_change_interrupts_an_authorized_run_for_inspection() {
    let f = Fixture::new();
    f.enable_mode(false, Some("workspace"));
    let queued = f.queue(&f.create("MODE_CHANGED", true), budget());
    let (stop, task) = scheduler::start(f.state.clone()).unwrap();
    let failed = f.wait(&queued, &["needs_reconciliation"]).await;
    assert_eq!(failed["reason"], "session_mode_changed");
    assert_eq!(failed["session_mode"], "workspace");
    assert_eq!(failed["prompt_may_have_run"], true);
    assert!(!f.dir.path().join("work/result.txt").exists());
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn bounded_continuation_reuses_session_and_counts_tools_across_turns() {
    let f = Fixture::new();
    f.enable(false);
    let w = f.create(
        "MULTITURN TURN_TOOL write and then independently inspect local fixture output",
        true,
    );
    let r = f.queue(
        &w,
        json!({"max_seconds":10,"max_output_bytes":8192,"max_tool_calls":10,"max_turns":4}),
    );
    let (stop, task) = scheduler::start(f.state.clone()).unwrap();
    let done = f
        .wait(
            &r,
            &[
                "reported",
                "needs_reconciliation",
                "failed_before_prompt",
                "blocked",
                "paused_limit",
            ],
        )
        .await;
    assert_eq!(done["state"], "reported", "{done}");
    assert_eq!(done["turns_completed"], 2);
    assert_eq!(done["turns_started"], 2);
    assert_eq!(done["tool_calls"], 2);
    assert_eq!(done["turns"][0]["disposition"], "continue");
    assert_eq!(done["turns"][1]["disposition"], "review");
    assert_eq!(done["session_id"], "fixture-background");
    assert_eq!(
        work::get_work(&f.state, w["id"].as_str().unwrap(), None).unwrap()["state"],
        "review"
    );
    stop.send(()).unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn exhausted_turn_budget_pauses_and_explicit_resume_loads_the_saved_session() {
    let f = Fixture::new();
    f.enable(false);
    let w = f.create("ALWAYS_CONTINUE local steps", true);
    let r = f.queue(
        &w,
        json!({"max_seconds":10,"max_output_bytes":8192,"max_tool_calls":10,"max_turns":2}),
    );
    let (stop, task) = scheduler::start(f.state.clone()).unwrap();
    let first = f
        .wait(
            &r,
            &[
                "paused_limit",
                "reported",
                "needs_reconciliation",
                "failed_before_prompt",
            ],
        )
        .await;
    assert_eq!(first["state"], "paused_limit", "{first}");
    assert_eq!(first["turns_completed"], 2);
    assert_eq!(first["load_session_supported"], true);
    let current = work::get_work(&f.state, w["id"].as_str().unwrap(), None).unwrap();
    assert_eq!(current["state"], "in_progress");
    assert!(current["verification"].is_null());
    let resumed=scheduler::command(&f.state,w["id"].as_str().unwrap(),serde_json::from_value(json!({"type":"resume","run_id":first["id"],"expected_revision":first["revision"],"expected_work_revision":current["revision"],"request_id":"resume-same-session","budget":{"max_seconds":10,"max_output_bytes":8192,"max_tool_calls":10,"max_turns":2},"reason":"Continue the next two local steps"})).unwrap(),"operator").unwrap();
    let second = f
        .wait(
            &resumed,
            &[
                "paused_limit",
                "needs_reconciliation",
                "failed_before_prompt",
            ],
        )
        .await;
    assert_eq!(second["state"], "paused_limit", "{second}");
    assert_eq!(second["session_id"], first["session_id"]);
    assert_eq!(second["execution_chain_id"], first["execution_chain_id"]);
    assert!(
        second["report"]
            .as_str()
            .unwrap()
            .contains("Finished step 4")
    );
    assert!(
        !second["report"]
            .as_str()
            .unwrap()
            .contains("REPLAYED HISTORY")
    );
    assert!(f.dir.path().join("work/loaded.json").exists());
    stop.send(()).unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn explicit_blocker_stops_continuation_without_claiming_review_or_completion() {
    let f = Fixture::new();
    f.enable(false);
    let w = f.create("REPORT_BLOCKER local work", true);
    let r = f.queue(
        &w,
        json!({"max_seconds":10,"max_output_bytes":8192,"max_tool_calls":10,"max_turns":5}),
    );
    let (stop, task) = scheduler::start(f.state.clone()).unwrap();
    let done = f
        .wait(
            &r,
            &["blocked", "needs_reconciliation", "failed_before_prompt"],
        )
        .await;
    assert_eq!(done["state"], "blocked", "{done}");
    assert_eq!(done["turns_completed"], 1);
    let work = work::get_work(&f.state, w["id"].as_str().unwrap(), None).unwrap();
    assert_eq!(work["state"], "blocked");
    assert_eq!(work["blocker"], "Need an input file");
    assert!(work["verification"].is_null());
    stop.send(()).unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn malformed_status_does_not_automatically_spend_more_authorized_turns() {
    let f = Fixture::new();
    f.enable(false);
    let w = f.create("Ordinary report without continuation convention", true);
    let r = f.queue(
        &w,
        json!({"max_seconds":10,"max_output_bytes":8192,"max_tool_calls":10,"max_turns":3}),
    );
    let (stop, task) = scheduler::start(f.state.clone()).unwrap();
    let done = f
        .wait(
            &r,
            &["blocked", "needs_reconciliation", "failed_before_prompt"],
        )
        .await;
    assert_eq!(done["state"], "blocked", "{done}");
    assert_eq!(done["turns_completed"], 1);
    stop.send(()).unwrap();
    task.await.unwrap();
}
