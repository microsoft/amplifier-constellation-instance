//! An opt-in, instance-owned ACP client. It never needs a connected browser.
//! A persisted prompt marker precedes delivery: uncertainty requires review, not replay.
use crate::{
    acp, auth,
    config::expand,
    context,
    error::{ApiError, ApiResult},
    state::{AppState, now_ms},
    work,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
};

const RUNS: &str = "work_execution";
const ACTIVE: &[&str] = &["queued", "starting", "running", "cancelling"];
const MAX_QUEUE: usize = 64;
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub max_seconds: u64,
    #[serde(default = "one_turn")]
    pub max_turns: usize,
    pub max_output_bytes: usize,
    pub max_tool_calls: usize,
}
fn one_turn() -> usize {
    1
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            max_seconds: 300,
            max_turns: 1,
            max_output_bytes: 131072,
            max_tool_calls: 30,
        }
    }
}
impl Budget {
    fn validate(&self) -> ApiResult<()> {
        if !(1..=3600).contains(&self.max_seconds)
            || !(1024..=1048576).contains(&self.max_output_bytes)
            || self.max_tool_calls > 100
            || !(1..=12).contains(&self.max_turns)
        {
            return Err(ApiError::bad(
                "Budgets: 1–3600 seconds, 1024–1048576 output bytes, 0–100 observed tool calls, 1–12 turns",
            ));
        }
        Ok(())
    }
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub expected_revision: u64,
    pub enabled: bool,
    pub paused: bool,
    pub concurrency: usize,
    /// The operator has checked that this exact backend is restricted to authorized local work.
    pub approved_backend_sha256: Option<String>,
    /// Optional ACP mode, explicitly authorized for new background runs only.
    #[serde(default)]
    pub session_mode: Option<String>,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Queue {
        expected_work_revision: u64,
        request_id: String,
        budget: Budget,
    },
    Cancel {
        run_id: String,
        expected_revision: u64,
        reason: String,
    },
    Reconcile {
        run_id: String,
        expected_revision: u64,
        note: String,
        allow_retry: bool,
    },
    Resume {
        run_id: String,
        expected_revision: u64,
        expected_work_revision: u64,
        request_id: String,
        budget: Budget,
        reason: String,
    },
    Retry {
        run_id: String,
        expected_revision: u64,
        expected_work_revision: u64,
        request_id: String,
        budget: Budget,
        reason: String,
    },
}
pub fn backend_hash(state: &AppState) -> String {
    let config = state.instance_config();
    let mut value = json!({"backend":config.backend});
    if let Some(b) = &config.backend
        && [Some("agent-runtime"), Some("agent-runtime.exe")].contains(
            &std::path::Path::new(&b.command)
                .file_name()
                .and_then(|n| n.to_str()),
        )
    {
        let cwd = std::path::PathBuf::from(expand(&state.home, &b.working_directory));
        let absolute = |v: &str| {
            let p = std::path::PathBuf::from(expand(&state.home, v));
            if p.is_absolute() { p } else { cwd.join(p) }
        };
        let home = b.args.iter().enumerate().find_map(|(i, a)| {
            if a == "--home" {
                b.args.get(i + 1).map(|v| absolute(v))
            } else {
                a.strip_prefix("--home=").map(absolute)
            }
        });
        let digest = home
            .and_then(|h| {
                let p = h.join("runtime.json");
                std::fs::metadata(&p)
                    .ok()
                    .filter(|m| m.len() <= 1024 * 1024)
                    .and_then(|_| std::fs::read(p).ok())
            })
            .map(|bytes| {
                use sha2::{Digest, Sha256};
                format!("{:x}", Sha256::digest(bytes))
            });
        value["native_configuration_sha256"] = json!(digest);
        return auth::digest(&value.to_string());
    }
    auth::digest(&serde_json::to_string(&config.backend).unwrap())
}
fn settings_db(db: &Connection) -> ApiResult<Value> {
    Ok(work::read(db,"scheduler","settings")?.unwrap_or_else(||json!({"revision":0,"enabled":false,"paused":false,"concurrency":1,"approved_backend_sha256":null,"session_mode":null})))
}
fn runs_db(db: &Connection, work_id: Option<&str>, active_only: bool) -> ApiResult<Vec<Value>> {
    // Dashboard reads avoid loading historical report text; execution history is bounded per response.
    let mut query = db.prepare("SELECT CASE WHEN ?2 IS NULL AND ?3=0 THEN json_remove(value,'$.report','$.work_snapshot') ELSE value END FROM records WHERE collection=?1 AND (?2 IS NULL OR json_extract(value,'$.work_id')=?2) AND (?3=0 OR json_extract(value,'$.state') IN ('queued','starting','running','cancelling')) ORDER BY CASE WHEN ?3=1 THEN json_extract(value,'$.created_at') END ASC, json_extract(value,'$.created_at') DESC,id LIMIT 100")?;
    let rows = query
        .query_map(params![RUNS, work_id, active_only], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|s| serde_json::from_str(&s).map_err(|e| anyhow::anyhow!(e).into()))
        .collect()
}
fn touch(run: &mut Value) {
    run["revision"] = json!(run["revision"].as_u64().unwrap_or(0) + 1);
    run["updated_at"] = json!(now_ms());
}
fn terminal(run: &mut Value, status: &str, reason: &str) {
    run["state"] = json!(status);
    run["reason"] = json!(reason);
    run["finished_at"] = json!(now_ms());
    touch(run);
}
fn event(state: &AppState, run: &Value) -> ApiResult<()> {
    state.event("scheduler.run_changed",json!({"id":run["id"],"work_id":run["work_id"],"revision":run["revision"],"action":run["state"],"reason":run["reason"]}))?;
    Ok(())
}
pub fn inspect(state: &AppState) -> ApiResult<Value> {
    let db = state.db.lock().unwrap();
    let config = settings_db(&db)?;
    let rows = runs_db(&db, None, false)?;
    let mut query=db.prepare("SELECT json_extract(value,'$.state'),COUNT(*) FROM records WHERE collection=?1 GROUP BY json_extract(value,'$.state')")?;
    let counts = query
        .query_map([RUNS], |r| {
            Ok((r.get::<_, String>(0)?, json!(r.get::<_, i64>(1)?)))
        })?
        .collect::<rusqlite::Result<serde_json::Map<String, Value>>>()?;
    Ok(
        json!({"schema":"agent-instance.scheduler/1","settings":config,"backend_sha256":backend_hash(state),"runtime_enabled":state.enabled(),"counts":counts,"runs":rows,"limits":{"queued":MAX_QUEUE,"concurrency":4,"turns_per_run":12},"policy":{"callback_permissions":"deny","client_tools":[],"context_providers":[],"external_delivery":"none","backend_tools":"operator-approved local backend configuration; not enforced by ACP transport","success":"report_for_review_not_completion","retry":"explicit_only","continuation":"bounded_same_session","resume":"explicit_authorization_standard_session_load"}}),
    )
}
pub fn configure(state: &AppState, body: Settings, actor: &str) -> ApiResult<Value> {
    if body
        .session_mode
        .as_ref()
        .is_some_and(|m| m.trim().is_empty() || m.len() > 128 || m.chars().any(char::is_control))
    {
        return Err(ApiError::bad(
            "Use an offered ACP session mode ID of 1–128 bytes, or null for the engine default",
        ));
    }
    if !(1..=4).contains(&body.concurrency) {
        return Err(ApiError::bad("Choose 1–4 concurrent runs"));
    }
    if body.enabled
        && (state.instance_config().backend.is_none()
            || body.approved_backend_sha256.as_deref() != Some(backend_hash(state).as_str()))
    {
        return Err(ApiError::bad(
            "Review and approve the current backend for local background work before enabling execution",
        ));
    }
    let value = {
        let db = state.db.lock().unwrap();
        let old = settings_db(&db)?;
        if old["revision"] != body.expected_revision {
            return Err(ApiError::conflict(old));
        }
        let value = json!({"revision":body.expected_revision+1,"enabled":body.enabled,"paused":body.paused,"concurrency":body.concurrency,"approved_backend_sha256":body.approved_backend_sha256,"session_mode":body.session_mode,"updated_by":actor,"updated_at":now_ms()});
        work::write(&db, "scheduler", "settings", &value)?;
        value
    };
    state.event("scheduler.configured",json!({"actor":actor,"revision":value["revision"],"enabled":value["enabled"],"action":if body.paused {"paused"}else{"resumed"}}))?;
    Ok(value)
}
pub fn inspect_work(state: &AppState, id: &str, binding: Option<&Value>) -> ApiResult<Value> {
    work::get_work(state, id, binding)?;
    let db = state.db.lock().unwrap();
    let runs = runs_db(&db, Some(id), false)?;
    Ok(json!({"work_id":id,"runs":runs.into_iter().take(100).collect::<Vec<_>>(),"limit":100}))
}
fn require_text(text: &str) -> ApiResult<()> {
    if text.trim().is_empty() || text.len() > 4096 {
        Err(ApiError::bad("A reason of 1–4096 bytes is required"))
    } else {
        Ok(())
    }
}
fn authorized_work(db: &Connection, id: &str, revision: u64, owner: &str) -> ApiResult<Value> {
    let w = work::read(db, "work", id)?.ok_or_else(ApiError::missing)?;
    if w["revision"] != revision {
        return Err(ApiError::conflict(json!({"work_revision":w["revision"]})));
    }
    if w["owner_agent_id"] != owner
        || !["accepted", "in_progress"].contains(&w["state"].as_str().unwrap_or(""))
    {
        return Err(ApiError::bad(
            "Only this agent's accepted or in-progress responsibilities can be queued",
        ));
    }
    Ok(w)
}
struct QueueGrant<'a> {
    work_id: &'a str,
    work_revision: u64,
    request_id: &'a str,
    budget: &'a Budget,
    actor: &'a str,
    retry: Option<&'a Value>,
    request_hash: &'a str,
    resume: bool,
}
fn new_run(state: &AppState, db: &Connection, grant: QueueGrant<'_>) -> ApiResult<Value> {
    let QueueGrant {
        work_id: id,
        work_revision: revision,
        request_id,
        budget,
        actor,
        retry,
        request_hash,
        resume,
    } = grant;
    budget.validate()?;
    if request_id.is_empty()
        || request_id.len() > 128
        || !request_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c))
    {
        return Err(ApiError::bad(
            "Use a unique request_id of up to 128 letters, numbers, hyphens, dots, colons or underscores",
        ));
    }
    let prior = work::read(db, "execution_requests", request_id)?;
    if let Some(prior) = prior {
        if prior["hash"] != request_hash {
            return Err(ApiError::bad(
                "request_id was already used with different content or authority",
            ));
        }
        return work::read(db, RUNS, prior["run_id"].as_str().unwrap())?
            .ok_or_else(ApiError::missing);
    }
    let config = settings_db(db)?;
    if config["enabled"] != true || config["approved_backend_sha256"] != backend_hash(state) {
        return Err(ApiError::bad(
            "Enable and approve the current backend before authorizing a run",
        ));
    }
    let w = authorized_work(db, id, revision, &state.instance_config().instance_id)?;
    let active = runs_db(db, None, true)?;
    if active.len() >= MAX_QUEUE {
        return Err(ApiError::bad(
            "The queue already contains 64 pending or active runs",
        ));
    }
    if active.iter().any(|r| r["work_id"] == id) {
        return Err(ApiError::bad(
            "This responsibility already has a pending or active run",
        ));
    }
    if runs_db(db, Some(id), false)?
        .iter()
        .any(|r| r["state"] == "needs_reconciliation")
    {
        return Err(ApiError::bad(
            "Inspect and reconcile the interrupted run before authorizing any more work",
        ));
    }
    if resume
        && retry.is_none_or(|r| {
            r["load_session_supported"] != true
                || !r["session_id"].is_string()
                || r["backend_sha256"] != backend_hash(state)
        })
    {
        return Err(ApiError::bad(
            "This engine cannot safely reload this saved session; authorize a fresh retry instead",
        ));
    }
    let at = now_ms();
    let rid = uuid::Uuid::new_v4().to_string();
    let value = json!({"schema":"agent-instance.work-execution/1","id":rid,"revision":1,"work_id":id,"work_revision":revision,"work_snapshot":context::execution_snapshot(&w),"work_fingerprint":work_fingerprint(&w),"audience":w["audience"],"source":w["source"],"authorized_by":actor,"authorized_at":at,"request_id":request_id,"budget":budget,"session_mode":config["session_mode"],"mode_applied":false,"backend_sha256":backend_hash(state),"state":"queued","reason":null,"created_at":at,"updated_at":at,"started_at":null,"finished_at":null,"prompt_may_have_run":false,"session_id":if resume {retry.map(|r|r["session_id"].clone())}else{None},"execution_chain_id":if resume {retry.and_then(|r|r.get("execution_chain_id").or_else(||r.get("id"))).cloned()}else{Some(json!(rid))},"resume_session":resume,"load_session_supported":false,"turns":[],"turns_started":0,"turns_completed":0,"progress":null,"previous_report":retry.map(|r|r["report"].clone()),"report":"","report_sha256":null,"report_trust":"agent_report_unverified","tool_calls":0,"output_bytes":0,"reconciliation":null,"retry_of":retry.map(|r|r["id"].clone()),"continuation_reason":retry.and_then(|r|r.get("retry_reason")).cloned()});
    work::write(db, RUNS, &rid, &value)?;
    work::write(
        db,
        "execution_requests",
        request_id,
        &json!({"run_id":rid,"hash":request_hash}),
    )?;
    Ok(value)
}
pub fn command(state: &AppState, work_id: &str, body: Command, actor: &str) -> ApiResult<Value> {
    let resume = matches!(&body, Command::Resume { .. });
    let request_hash =
        auth::digest(&json!({"work_id":work_id,"actor":actor,"command":body}).to_string());
    let result = {
        let db = state.db.lock().unwrap();
        let tx = db.unchecked_transaction()?;
        let result = match body {
            Command::Queue {
                expected_work_revision,
                request_id,
                budget,
            } => new_run(
                state,
                &tx,
                QueueGrant {
                    work_id,
                    work_revision: expected_work_revision,
                    request_id: &request_id,
                    budget: &budget,
                    actor,
                    retry: None,
                    request_hash: &request_hash,
                    resume: false,
                },
            )?,
            Command::Cancel {
                run_id,
                expected_revision,
                reason,
            } => {
                require_text(&reason)?;
                let mut r = matching(&tx, work_id, &run_id, expected_revision)?;
                if !ACTIVE.contains(&r["state"].as_str().unwrap_or("")) {
                    return Err(ApiError::bad(
                        "Only a pending or active run can be cancelled",
                    ));
                }
                r["cancel_requested_by"] = json!(actor);
                r["cancel_note"] = json!(reason);
                if r["state"] == "queued" {
                    terminal(&mut r, "cancelled", "cancelled_before_start");
                } else {
                    r["state"] = json!("cancelling");
                    touch(&mut r);
                }
                work::write(&tx, RUNS, &run_id, &r)?;
                r
            }
            Command::Reconcile {
                run_id,
                expected_revision,
                note,
                allow_retry,
            } => {
                require_text(&note)?;
                let mut r = matching(&tx, work_id, &run_id, expected_revision)?;
                if r["state"] != "needs_reconciliation" {
                    return Err(ApiError::bad("Only an uncertain run needs reconciliation"));
                }
                r["reconciliation"] = json!({"actor":actor,"at":now_ms(),"note":note,"allow_retry":allow_retry,"trust":"operator_report","side_effects_independently_verified":false});
                terminal(&mut r, "reconciled", "operator_review_recorded");
                work::write(&tx, RUNS, &run_id, &r)?;
                r
            }
            Command::Resume {
                run_id,
                expected_revision,
                expected_work_revision,
                request_id,
                budget,
                reason,
            }
            | Command::Retry {
                run_id,
                expected_revision,
                expected_work_revision,
                request_id,
                budget,
                reason,
            } => {
                require_text(&reason)?;
                // A replay of this API request returns its existing run even after its parent revision changes.
                if let Some(prior) = work::read(&tx, "execution_requests", &request_id)? {
                    if prior["hash"] != request_hash {
                        return Err(ApiError::bad(
                            "request_id was already used with different content or authority",
                        ));
                    }
                    work::read(&tx, RUNS, prior["run_id"].as_str().unwrap())?
                        .ok_or_else(ApiError::missing)?
                } else {
                    let mut previous = matching(&tx, work_id, &run_id, expected_revision)?;
                    if ![
                        "failed_before_prompt",
                        "cancelled",
                        "reported",
                        "blocked",
                        "paused_limit",
                        "reconciled",
                    ]
                    .contains(&previous["state"].as_str().unwrap_or(""))
                        || (previous["state"] == "reconciled"
                            && previous["reconciliation"]["allow_retry"] != true)
                    {
                        return Err(ApiError::bad(
                            "This run is not ready to retry; uncertain execution needs explicit reconciliation",
                        ));
                    }
                    if resume
                        && !["reported", "blocked", "paused_limit"]
                            .contains(&previous["state"].as_str().unwrap_or(""))
                    {
                        return Err(ApiError::bad(
                            "Resume requires a cleanly finished turn; uncertain attempts must be reconciled and retried in a fresh session",
                        ));
                    }
                    previous["retry_reason"] = json!(reason);
                    new_run(
                        state,
                        &tx,
                        QueueGrant {
                            work_id,
                            work_revision: expected_work_revision,
                            request_id: &request_id,
                            budget: &budget,
                            actor,
                            retry: Some(&previous),
                            request_hash: &request_hash,
                            resume,
                        },
                    )?
                }
            }
        };
        tx.commit()?;
        result
    };
    event(state, &result)?;
    Ok(result)
}
fn matching(db: &Connection, work_id: &str, run_id: &str, revision: u64) -> ApiResult<Value> {
    let r = work::read(db, RUNS, run_id)?
        .filter(|r| r["work_id"] == work_id)
        .ok_or_else(ApiError::missing)?;
    if r["revision"] != revision {
        return Err(ApiError::conflict(json!({"revision":r["revision"]})));
    }
    Ok(r)
}
fn read_auth(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    auth::check(state, headers, "work:read", false)
        .or_else(|_| auth::check(state, headers, "work", false))
        .or_else(|_| auth::check(state, headers, "work:execute", false))?;
    Ok(())
}
pub async fn get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    read_auth(&state, &headers)?;
    Ok(Json(inspect(&state)?))
}
pub async fn put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Settings>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "work:execute", true)?.actor;
    Ok(Json(configure(&state, body, &actor)?))
}
pub async fn get_execution(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    read_auth(&state, &headers)?;
    Ok(Json(inspect_work(&state, &id, None)?))
}
pub async fn post_execution(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Command>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "work:execute", true)?.actor;
    Ok(Json(command(&state, &id, body, &actor)?))
}

fn work_fingerprint(w: &Value) -> String {
    auth::digest(&context::execution_snapshot(w).to_string())
}
/// Called only after the service's exclusive ownership lock is held. CLI reads do not recover jobs.
pub fn recover(state: &AppState) -> ApiResult<usize> {
    let changed = {
        let db = state.db.lock().unwrap();
        let tx = db.unchecked_transaction()?;
        let mut changed = Vec::new();
        for mut r in runs_db(&tx, None, true)? {
            if r["state"] == "queued" {
                continue;
            }
            let uncertain = r["prompt_may_have_run"] == true;
            r["report_sha256"] = json!(auth::digest(r["report"].as_str().unwrap_or("")));
            terminal(
                &mut r,
                if uncertain {
                    "needs_reconciliation"
                } else {
                    "failed_before_prompt"
                },
                "service_restarted",
            );
            work::write(&tx, RUNS, r["id"].as_str().unwrap(), &r)?;
            changed.push(r);
        }
        tx.commit()?;
        changed
    };
    for r in &changed {
        event(state, r)?;
    }
    Ok(changed.len())
}
fn claim(state: &AppState, running: usize) -> ApiResult<Option<Value>> {
    if !state.enabled() || !state.instance_config().setup_complete {
        return Ok(None);
    }
    let value = {
        let db = state.db.lock().unwrap();
        let tx = db.unchecked_transaction()?;
        let config = settings_db(&tx)?;
        if config["enabled"] != true
            || config["paused"] == true
            || running >= config["concurrency"].as_u64().unwrap_or(1) as usize
        {
            return Ok(None);
        }
        let Some(mut r) = runs_db(&tx, None, true)?
            .into_iter()
            .find(|r| r["state"] == "queued")
        else {
            return Ok(None);
        };
        let id = r["work_id"].as_str().unwrap();
        let w = work::read(&tx, "work", id)?;
        let invalid = w.as_ref().is_none_or(|w| {
            work_fingerprint(w) != r["work_fingerprint"]
                || !["accepted", "in_progress"].contains(&w["state"].as_str().unwrap_or(""))
        });
        if invalid
            || r["backend_sha256"] != backend_hash(state)
            || config["approved_backend_sha256"] != r["backend_sha256"]
        {
            terminal(&mut r, "failed_before_prompt", "authorization_changed");
        } else {
            r["state"] = json!("starting");
            r["started_at"] = json!(now_ms());
            touch(&mut r);
        }
        work::write(&tx, RUNS, r["id"].as_str().unwrap(), &r)?;
        tx.commit()?;
        r
    };
    event(state, &value)?;
    Ok(Some(value))
}
/// The caller owns stop/await. Dropping a browser or its token has no effect on already authorized work.
pub fn start(
    state: Arc<AppState>,
) -> ApiResult<(oneshot::Sender<()>, tokio::task::JoinHandle<()>)> {
    recover(&state)?;
    let (stop, mut stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut jobs = JoinSet::new();
        let mut tick = tokio::time::interval(Duration::from_millis(200));
        loop {
            tokio::select! {
                _=&mut stopped=>break,
                _=tick.tick()=>{
                    match claim(&state,jobs.len()) {
                        Ok(Some(r)) if r["state"]=="starting"=>{
                            let state=state.clone();
                            jobs.spawn(async move {run(state,r).await;});
                        },
                        Ok(_)=>{},
                        Err(_)=>{let _=state.event("scheduler.error",json!({"reason":"queue_unavailable"}));}
                    }
                },
                _=jobs.join_next(),if !jobs.is_empty()=>{}
            }
        }
        jobs.abort_all();
        while jobs.join_next().await.is_some() {}
        let _ = recover(&state);
    });
    Ok((stop, task))
}

fn current(state: &AppState, id: &str) -> ApiResult<Value> {
    state.get_record(RUNS, id)?.ok_or_else(ApiError::missing)
}
fn update(state: &AppState, id: &str, f: impl FnOnce(&mut Value)) -> ApiResult<Value> {
    let db = state.db.lock().unwrap();
    let mut r = work::read(&db, RUNS, id)?.ok_or_else(ApiError::missing)?;
    f(&mut r);
    touch(&mut r);
    work::write(&db, RUNS, id, &r)?;
    Ok(r)
}
fn interruption(state: &AppState, r: &Value) -> Option<&'static str> {
    if r["state"] == "cancelling" {
        return Some("cancel_requested");
    }
    if !state.enabled() {
        return Some("runtime_stopped");
    }
    let config = match state.get_record("scheduler", "settings") {
        Ok(Some(value)) => value,
        _ => return Some("state_unavailable"),
    };
    if config["enabled"] != true {
        return Some("scheduler_disabled");
    }
    if r["backend_sha256"] != backend_hash(state)
        || config["approved_backend_sha256"] != r["backend_sha256"]
    {
        return Some("backend_changed");
    }
    match work::get_work(state, r["work_id"].as_str()?, None) {
        Ok(w)
            if work_fingerprint(&w) == r["work_fingerprint"]
                && ["accepted", "in_progress"].contains(&w["state"].as_str().unwrap_or("")) =>
        {
            None
        }
        _ => Some("work_changed"),
    }
}
struct Client {
    connection: Option<amplifier_acp_connection::Connection>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}
impl Drop for Client {
    fn drop(&mut self) {
        self.connection.take();
        self.task.abort();
    }
}
impl Client {
    async fn send(&self, value: Value) -> Result<(), &'static str> {
        self.connection
            .as_ref()
            .ok_or("backend_disconnected")?
            .sender
            .send(value)
            .map_err(|_| "backend_disconnected")
    }
    fn request(&self, method: &str, params: Value) -> Result<Value, &'static str> {
        self.connection
            .as_ref()
            .ok_or("backend_disconnected")?
            .sender
            .request(method, params)
            .map_err(|_| "backend_disconnected")
    }
    async fn stop(&mut self, sid: &Value) {
        if let Some(sid) = sid.as_str() {
            let _ = self
                .send(json!({"method":"session/cancel","params":{"sessionId":sid}}))
                .await;
        }
        self.connection.take();
        let _ = tokio::time::timeout(Duration::from_secs(2), &mut self.task).await;
    }
}
async fn response(
    state: &AppState,
    run_id: &str,
    client: &mut Client,
    request_id: Value,
    is_prompt: bool,
    deadline: tokio::time::Instant,
    tool_ids: &mut BTreeSet<String>,
) -> Result<Value, &'static str> {
    let mut answer = crate::answer::Answer::default();
    let mut prefix: Option<String> = None;
    loop {
        let r = current(state, run_id).map_err(|_| "state_unavailable")?;
        if let Some(reason) = interruption(state, &r) {
            return Err(reason);
        }
        let frame = tokio::select! {_ = tokio::time::sleep_until(deadline)=>return Err("time_budget"),_ = tokio::time::sleep(Duration::from_millis(100))=>continue,frame=client.connection.as_mut().ok_or("backend_disconnected")?.recv()=>frame.map_err(|_|"backend_disconnected")?.ok_or("backend_disconnected")?};
        if let Some(method) = frame["method"].as_str() {
            if let Some(id) = frame.get("id") {
                let denied = if method == "session/request_permission" {
                    json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"cancelled"}}})
                } else {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Background execution offers no client tools or interactive approvals"}})
                };
                client.send(denied).await?;
                let _ = state.event(
                    "scheduler.permission_denied",
                    json!({"id":run_id,"method":method}),
                );
                return Err("permission_required");
            }
            if method == "session/update" {
                let u = &frame["params"]["update"];
                if u["sessionUpdate"] == "current_mode_update"
                    && r["mode_applied"] == true
                    && r["session_mode"].is_string()
                    && u["currentModeId"] != r["session_mode"]
                {
                    return Err("session_mode_changed");
                }
                if is_prompt
                    && u["sessionUpdate"] == "agent_message_chunk"
                    && u["content"]["type"] == "text"
                {
                    let text = u["content"]["text"].as_str().unwrap_or("");
                    let count = r["output_bytes"].as_u64().unwrap_or(0) as usize;
                    if text.len()
                        > (r["budget"]["max_output_bytes"].as_u64().unwrap_or(0) as usize)
                            .saturating_sub(count)
                    {
                        return Err("output_budget");
                    }
                    let prefix =
                        prefix.get_or_insert_with(|| r["report"].as_str().unwrap_or("").to_owned());
                    answer.observe(u).map_err(|_| "output_budget")?;
                    update(state, run_id, |r| {
                        r["report"] = json!(format!("{}{}", prefix, answer.text()));
                        r["output_bytes"] = json!(count + text.len());
                    })
                    .map_err(|_| "state_unavailable")?;
                }
                if is_prompt && u["sessionUpdate"] == "tool_call" {
                    if let Some(id) = u["toolCallId"].as_str() {
                        tool_ids.insert(format!("{request_id}:{id}"));
                    } else {
                        return Err("invalid_tool_update");
                    }
                    update(state, run_id, |r| r["tool_calls"] = json!(tool_ids.len()))
                        .map_err(|_| "state_unavailable")?;
                    if tool_ids.len() > r["budget"]["max_tool_calls"].as_u64().unwrap_or(0) as usize
                    {
                        return Err("tool_budget");
                    }
                }
            }
        } else if frame["id"] == request_id {
            if frame.get("error").is_some() {
                return Err("backend_error");
            }
            return frame.get("result").cloned().ok_or("invalid_response");
        }
    }
}
fn mark_prompt(state: &AppState, id: &str) -> ApiResult<Value> {
    let value = {
        let db = state.db.lock().unwrap();
        let tx = db.unchecked_transaction()?;
        let mut r = work::read(&tx, RUNS, id)?.ok_or_else(ApiError::missing)?;
        if !["starting", "running"].contains(&r["state"].as_str().unwrap_or("")) {
            return Err(ApiError::bad("Run was cancelled before the prompt"));
        }
        let wid = r["work_id"].as_str().unwrap().to_owned();
        let mut w = work::read(&tx, "work", &wid)?.ok_or_else(ApiError::missing)?;
        if work_fingerprint(&w) != r["work_fingerprint"]
            || !["accepted", "in_progress"].contains(&w["state"].as_str().unwrap_or(""))
        {
            return Err(ApiError::bad("Work changed before execution"));
        }
        if w["state"] == "accepted" {
            work::mutate(&mut w, work::Action::Start, "scheduler")?;
            work::record_change(&mut w, "scheduler", "background_started")?;
            work::write(&tx, "work", &wid, &w)?;
        }
        r["prompt_may_have_run"] = json!(true);
        r["turns_started"] = json!(r["turns_started"].as_u64().unwrap_or(0) + 1);
        r["state"] = json!("running");
        touch(&mut r);
        work::write(&tx, RUNS, id, &r)?;
        tx.commit()?;
        r
    };
    event(state, &value)?;
    Ok(value)
}
// A tiny text convention keeps continuation compatible with arbitrary ACP agents.
// It can only spend the already-authorized budget or stop; it never grants new tools.
fn turn_status(text: &str, max_turns: usize) -> Value {
    if let Some(line) = text.lines().rev().find(|l| !l.trim().is_empty())
        && let Some(raw) = line.trim().strip_prefix("AGENT_WORK_STATUS ")
        && let Ok(v) = serde_json::from_str::<Value>(raw)
        && ["continue", "blocked", "review"].contains(&v["status"].as_str().unwrap_or(""))
        && v["summary"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty() && s.len() <= 4096)
    {
        return json!({"status":v["status"],"summary":v["summary"]});
    }
    if max_turns == 1 {
        json!({"status":"review","summary":"A single-turn report is ready for review"})
    } else {
        json!({"status":"blocked","summary":"The agent did not return a valid continuation status. Inspect the report before continuing."})
    }
}
fn finish(state: &AppState, id: &str, result: Result<Value, &'static str>) -> ApiResult<()> {
    let value = {
        let db = state.db.lock().unwrap();
        let tx = db.unchecked_transaction()?;
        let mut r = work::read(&tx, RUNS, id)?.ok_or_else(ApiError::missing)?;
        let wid = r["work_id"].as_str().unwrap().to_owned();
        let mut w = work::read(&tx, "work", &wid)?.ok_or_else(ApiError::missing)?;
        let unchanged =
            work_fingerprint(&w) == r["work_fingerprint"] && w["state"] == "in_progress";
        let clean = matches!(&result,Ok(v) if v["stopReason"]=="end_turn")
            && r["state"] != "cancelling"
            && unchanged
            && !r["report"].as_str().unwrap_or("").trim().is_empty();
        if clean {
            r["stop_reason"] = json!("end_turn");
            let disposition = result
                .as_ref()
                .ok()
                .and_then(|v| v["workDisposition"].as_str())
                .unwrap_or("review");
            terminal(
                &mut r,
                match disposition {
                    "blocked" => "blocked",
                    "continue" => "paused_limit",
                    _ => "reported",
                },
                match disposition {
                    "blocked" => "agent_reported_blocker",
                    "continue" => "turn_budget",
                    _ => "ready_for_review",
                },
            );
            // The record contains the actual returned text, never a claim that its assertions were checked.
            let reference = format!(
                "urn:agent-instance:{}:execution:{}",
                state.instance_config().instance_id,
                id
            );
            if w["evidence"].as_array().is_some_and(|e| e.len() < 100) {
                w["evidence"].as_array_mut().unwrap().push(json!({"id":uuid::Uuid::new_v4().to_string(),"label":"Background execution report","uri":reference,"kind":"source","at":now_ms(),"recorded_by":"scheduler","trust":"agent_report_unverified","execution_id":id}));
            }
            w["verification"] = Value::Null;
            w["progress"] = r["progress"]
                .as_str()
                .map(|s| json!(s))
                .unwrap_or_else(|| json!("Execution report returned"));
            w["state"] = json!(match disposition {
                "blocked" => "blocked",
                "continue" => "in_progress",
                _ => "review",
            });
            if disposition == "blocked" {
                w["blocker"] = w["progress"].clone();
            }
            work::record_change(&mut w, "scheduler", "background_reported")?;
            work::write(&tx, "work", &wid, &w)?;
        } else {
            let reason = match &result {
                Err(e) => *e,
                Ok(_) => {
                    if r["state"] == "cancelling" {
                        "cancel_requested"
                    } else if unchanged {
                        "incomplete_turn"
                    } else {
                        "work_changed"
                    }
                }
            };
            let status = if r["prompt_may_have_run"] == true {
                "needs_reconciliation"
            } else if reason == "cancel_requested" {
                "cancelled"
            } else {
                "failed_before_prompt"
            };
            if let Ok(v) = &result {
                r["stop_reason"] = v["stopReason"].clone();
            }
            terminal(&mut r, status, reason);
        }
        r["report_sha256"] = json!(auth::digest(r["report"].as_str().unwrap_or("")));
        work::write(&tx, RUNS, id, &r)?;
        tx.commit()?;
        r
    };
    event(state, &value)?;
    Ok(())
}
async fn run(state: Arc<AppState>, r: Value) {
    let id = r["id"].as_str().unwrap();
    let budget: Budget = serde_json::from_value(r["budget"].clone()).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(budget.max_seconds);
    let (tx, incoming) = mpsc::channel(16);
    let (outgoing, rx) = mpsc::channel(16);
    let relay_state = state.clone();
    let principal = format!(
        "background:{}",
        r["execution_chain_id"].as_str().unwrap_or(id)
    );
    let task =
        tokio::spawn(async move { acp::relay(relay_state, principal, incoming, outgoing).await });
    let connection = match amplifier_acp_connection::Connection::client_messages(rx, tx).await {
        Ok(connection) => connection,
        Err(_) => {
            task.abort();
            let _ = finish(&state, id, Err("backend_disconnected"));
            return;
        }
    };
    let mut client = Client {
        connection: Some(connection),
        task,
    };
    let mut tools = BTreeSet::new();
    let result=async{
        let request=client.request("initialize",json!({"protocolVersion":1,"clientInfo":{"name":"agent-instance-background","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":{}}))?;
        let init=response(&state,id,&mut client,request,false,deadline,&mut tools).await?;
        if init["protocolVersion"]!=1 {return Err("unsupported_protocol");}
        let load_supported=init["agentCapabilities"]["loadSession"]==true;
        update(&state,id,|r|r["load_session_supported"]=json!(load_supported)).map_err(|_|"state_unavailable")?;
        let cfg=state.instance_config();let backend=cfg.backend.ok_or("backend_missing")?;
        let mut session_params=json!({"cwd":expand(&state.home,&backend.working_directory),"mcpServers":[]});
        let method=if r["resume_session"]==true {if !load_supported{return Err("session_load_unavailable");}session_params["sessionId"]=r["session_id"].clone();"session/load"}else{"session/new"};
        let request=client.request(method,session_params)?;
        let session=response(&state,id,&mut client,request,false,deadline,&mut tools).await?;
        let sid=if r["resume_session"]==true {r["session_id"].as_str()}else{session["sessionId"].as_str()}.filter(|s|!s.is_empty()).ok_or("invalid_session")?;
        update(&state,id,|r|r["session_id"]=json!(sid)).map_err(|_|"state_unavailable")?;
        if let Some(mode)=r["session_mode"].as_str() {
            let offered=session["modes"]["availableModes"].as_array().is_some_and(|modes|modes.iter().any(|m|m["id"]==mode));
            if !offered {return Err("unsupported_session_mode");}
            let request=client.request("session/set_mode",json!({"sessionId":sid,"modeId":mode}))?;
            response(&state,id,&mut client,request,false,deadline,&mut tools).await?;
            update(&state,id,|r|r["mode_applied"]=json!(true)).map_err(|_|"state_unavailable")?;
        }
        for turn in 0..budget.max_turns {
            let latest=current(&state,id).map_err(|_|"state_unavailable")?;
            if let Some(reason)=interruption(&state,&latest){return Err(reason);}
            let offset=latest["report"].as_str().unwrap_or("").len();
            mark_prompt(&state,id).map_err(|_|"work_changed")?;
            let prompt=if turn==0 {context::execution_prompt(&r)} else {format!("Continue this same authorized responsibility from the previous turn. Do not repeat completed actions. Remaining turns: {}. The original time/output/tool budgets still apply across all turns. Stop for missing information or permission. End with the required AGENT_WORK_STATUS line: continue, blocked, or review. Your latest reported progress: {}",budget.max_turns-turn,latest["progress"])};
            let request=client.request("session/prompt",json!({"sessionId":sid,"prompt":[{"type":"text","text":prompt}]}))?;
            let mut result=response(&state,id,&mut client,request,true,deadline,&mut tools).await?;
            if result["stopReason"]!="end_turn" {return Ok(result);}
            let latest=current(&state,id).map_err(|_|"state_unavailable")?;
            let text=latest["report"].as_str().unwrap_or("").get(offset..).unwrap_or("");
            let status=turn_status(text,budget.max_turns);
            update(&state,id,|r|{
                r["turns_completed"]=json!(turn+1);
                r["progress"]=status["summary"].clone();
                if !r["turns"].is_array(){r["turns"]=json!([]);}
                r["turns"].as_array_mut().unwrap().push(json!({"number":turn+1,"completed_at":now_ms(),"disposition":status["status"],"summary":status["summary"],"report_offset":offset,"report_end":latest["report"].as_str().unwrap_or("").len(),"trust":"agent_report_unverified"}));
            }).map_err(|_|"state_unavailable")?;
            let last=current(&state,id).map_err(|_|"state_unavailable")?;
            let _=event(&state,&last);
            result["workDisposition"]=status["status"].clone();
            if status["status"]!="continue" || turn+1==budget.max_turns {return Ok(result);}
        }
        Err("turn_budget")
    }.await;
    let latest = current(&state, id).unwrap_or(r.clone());
    client.stop(&latest["session_id"]).await;
    if finish(&state, id, result).is_err() {
        // A full work history or failed review transition must not leave a dead worker marked active.
        if let Ok(failed) = update(&state, id, |r| {
            let status = if r["prompt_may_have_run"] == true {
                "needs_reconciliation"
            } else {
                "failed_before_prompt"
            };
            terminal(r, status, "result_not_persisted");
        }) {
            let _ = event(&state, &failed);
        }
        let _ = state.event(
            "scheduler.error",
            json!({"id":id,"reason":"result_not_persisted"}),
        );
    }
}
