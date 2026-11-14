//! Instance-owned responsibilities. No peer database or implicit sharing. Execution is separately authorized.
use crate::{
    auth,
    error::{ApiError, ApiResult},
    state::{AppState, now_ms},
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};

mod completion;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Audience {
    Owner,
    Conversation {
        provider_id: String,
        conversation_id: String,
    },
}
impl Audience {
    pub(crate) fn validate(&self) -> ApiResult<()> {
        if let Self::Conversation {
            provider_id,
            conversation_id,
        } = self
        {
            text(provider_id, 128)?;
            text(conversation_id, 512)?;
        }
        Ok(())
    }
    pub fn for_binding(binding: &Value) -> Self {
        if binding["owner"] == true {
            Self::Owner
        } else {
            Self::Conversation {
                provider_id: binding["provider_id"].as_str().unwrap_or_default().into(),
                conversation_id: binding["conversation_id"]
                    .as_str()
                    .unwrap_or_default()
                    .into(),
            }
        }
    }
    pub(crate) fn accessible(&self, binding: Option<&Value>) -> bool {
        binding.is_none_or(|b| b["owner"]==true || matches!(self, Self::Conversation{provider_id,conversation_id} if b["provider_id"]==*provider_id && b["conversation_id"]==*conversation_id))
    }
}
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Template {
    pub id: String,
    pub version: String,
    pub name: String,
    pub instructions: String,
}
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub id: String,
    pub name: String,
    pub domain: String,
    pub purpose: String,
    pub template_id: Option<String>,
    pub audience: Audience,
    #[serde(default = "yes")]
    pub active: bool,
}
fn yes() -> bool {
    true
}
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Relationship {
    pub id: String,
    pub label: String,
    pub peer_agent_id: String,
    pub assignment_ids: Vec<String>,
    #[serde(default)]
    pub send: bool,
    #[serde(default)]
    pub receive: bool,
    #[serde(default)]
    pub allow_details: bool,
    #[serde(default = "yes")]
    pub active: bool,
}
#[derive(Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Organization {
    #[serde(default)]
    pub templates: Vec<Template>,
    #[serde(default)]
    pub assignments: Vec<Assignment>,
    #[serde(default)]
    pub relationships: Vec<Relationship>,
}
fn text(s: &str, max: usize) -> ApiResult<()> {
    if s.trim().is_empty() || s.len() > max {
        Err(ApiError::bad(format!("Text must contain 1–{max} bytes")))
    } else {
        Ok(())
    }
}
fn id(s: &str) -> ApiResult<()> {
    if !crate::config::valid_name(s) {
        Err(ApiError::bad(
            "Use 1–80 letters, digits, hyphens or underscores for local identifiers",
        ))
    } else {
        Ok(())
    }
}
fn decode<T: serde::de::DeserializeOwned>(v: Value) -> ApiResult<T> {
    serde_json::from_value(v).map_err(|e| ApiError::bad(e.to_string()))
}
pub(crate) fn read(db: &Connection, collection: &str, id: &str) -> ApiResult<Option<Value>> {
    let raw: Option<String> = db
        .query_row(
            "SELECT value FROM records WHERE collection=?1 AND id=?2",
            params![collection, id],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|s| {
        serde_json::from_str(&s).map_err(|_| ApiError::bad("Invalid stored responsibility record"))
    })
    .transpose()
}
pub(crate) fn write(db: &Connection, collection: &str, id: &str, v: &Value) -> ApiResult<()> {
    db.execute("INSERT INTO records(collection,id,value,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(collection,id) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",params![collection,id,v.to_string(),now_ms()])?;
    Ok(())
}
fn organization_db(db: &Connection) -> ApiResult<Value> {
    Ok(read(db,"organization","current")?.unwrap_or_else(||json!({"schema":"agent-instance.organization/1","revision":0,"organization":Organization::default()})))
}
pub fn organization(state: &AppState) -> ApiResult<Value> {
    let mut v = organization_db(&state.db.lock().unwrap())?;
    v["individual"] =
        json!({"id":state.instance_config().instance_id,"name":state.instance_config().name});
    Ok(v)
}
fn validate_org(org: &Organization, individual: &str) -> ApiResult<()> {
    if org.templates.len() > 100 || org.assignments.len() > 100 || org.relationships.len() > 100 {
        return Err(ApiError::bad("At most 100 of each organization record"));
    }
    let mut ids = BTreeSet::new();
    for t in &org.templates {
        id(&t.id)?;
        text(&t.name, 200)?;
        text(&t.version, 120)?;
        text(&t.instructions, 16 * 1024)?;
        if !ids.insert(&t.id) {
            return Err(ApiError::bad("Duplicate template"));
        }
    }
    let mut assignments = BTreeSet::new();
    for a in &org.assignments {
        id(&a.id)?;
        text(&a.name, 200)?;
        text(&a.domain, 120)?;
        text(&a.purpose, 8192)?;
        a.audience.validate()?;
        if !assignments.insert(&a.id) {
            return Err(ApiError::bad("Duplicate assignment"));
        }
        if a.template_id.as_ref().is_some_and(|i| !ids.contains(i)) {
            return Err(ApiError::bad("Unknown assignment template"));
        }
    }
    let mut relationships = BTreeSet::new();
    for r in &org.relationships {
        id(&r.id)?;
        text(&r.label, 200)?;
        text(&r.peer_agent_id, 512)?;
        if r.peer_agent_id == individual
            || !relationships.insert(&r.id)
            || r.assignment_ids.is_empty()
            || r.assignment_ids.iter().any(|a| !assignments.contains(a))
        {
            return Err(ApiError::bad(
                "Relationships need a distinct peer, unique ID and explicit existing assignments",
            ));
        }
    }
    Ok(())
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrganizationUpdate {
    expected_revision: u64,
    organization: Organization,
}
pub async fn organization_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    read_auth(&state, &headers)?;
    Ok(Json(organization(&state)?))
}
fn read_auth(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    if auth::check(state, headers, "work:read", false).is_err() {
        auth::check(state, headers, "work", false)?;
    }
    Ok(())
}
pub async fn organization_put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<OrganizationUpdate>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "admin", true)?.actor;
    validate_org(&body.organization, &state.instance_config().instance_id)?;
    let value = {
        let db = state.db.lock().unwrap();
        let old = organization_db(&db)?;
        if old["revision"] != body.expected_revision {
            return Err(ApiError::conflict(json!({"revision":old["revision"]})));
        }
        let previous: Organization = decode(old["organization"].clone())?;
        // Work and its original audience remain stable when roles evolve. Archive used assignments instead of changing their boundary.
        for a in &previous.assignments {
            let count:i64=db.query_row("SELECT count(*) FROM records WHERE collection='work' AND json_extract(value,'$.assignment_id')=?1",[&a.id],|r|r.get(0))?;
            if count > 0
                && !body
                    .organization
                    .assignments
                    .iter()
                    .any(|next| next.id == a.id && next.audience == a.audience)
            {
                return Err(ApiError::bad(
                    "An assignment used by work cannot be deleted or change audience; archive it instead",
                ));
            }
        }
        let value = json!({"schema":"agent-instance.organization/1","revision":body.expected_revision+1,"organization":body.organization,"updated_at":now_ms(),"updated_by":actor});
        write(&db, "organization", "current", &value)?;
        value
    };
    state.event(
        "organization.changed",
        json!({"revision":value["revision"],"actor":actor}),
    )?;
    Ok(Json(value))
}
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewWork {
    pub title: String,
    pub goal: String,
    pub assignment_id: Option<String>,
    pub audience: Option<Audience>,
    pub source: Option<Source>,
    pub due_at: Option<i64>,
    pub request_id: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub provider_id: String,
    pub conversation_id: String,
    pub message_id: Option<String>,
    pub url: Option<String>,
}
pub(crate) fn source_validate(source: &Source) -> ApiResult<()> {
    text(&source.provider_id, 128)?;
    text(&source.conversation_id, 512)?;
    if let Some(v) = &source.message_id {
        text(v, 512)?;
    }
    if let Some(v) = &source.url {
        let u = reqwest::Url::parse(v)
            .map_err(|_| ApiError::bad("Source URL must be absolute HTTP or HTTPS"))?;
        if !["https", "http"].contains(&u.scheme())
            || !u.username().is_empty()
            || u.password().is_some()
        {
            return Err(ApiError::bad("Invalid source URL"));
        }
    }
    Ok(())
}
fn work_value(
    state: &AppState,
    db: &Connection,
    body: NewWork,
    actor: &str,
    binding: Option<&Value>,
) -> ApiResult<Value> {
    text(&body.title, 240)?;
    text(&body.goal, 16 * 1024)?;
    let audience = body.audience.unwrap_or_else(|| {
        binding
            .map(Audience::for_binding)
            .unwrap_or(Audience::Owner)
    });
    audience.validate()?;
    if !audience.accessible(binding) {
        return Err(ApiError::forbidden());
    }
    let mut assignment = Value::Null;
    let mut template = Value::Null;
    if let Some(assignment_id) = &body.assignment_id {
        let org: Organization = decode(organization_db(db)?["organization"].clone())?;
        let a = org
            .assignments
            .iter()
            .find(|a| a.id == *assignment_id && a.active && a.audience.accessible(binding))
            .ok_or_else(|| ApiError::bad("Choose an active assignment"))?;
        if a.audience != audience {
            return Err(ApiError::bad("Work must keep its assignment's audience"));
        }
        assignment = json!(a);
        template = json!(
            a.template_id
                .as_ref()
                .and_then(|id| org.templates.iter().find(|t| t.id == *id))
        );
    }
    if let Some(s) = &body.source {
        source_validate(s)?;
        if let Audience::Conversation {
            provider_id,
            conversation_id,
        } = &audience
            && (*provider_id != s.provider_id || *conversation_id != s.conversation_id)
        {
            return Err(ApiError::bad("Source must belong to this work's audience"));
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let at = now_ms();
    Ok(
        json!({"schema":"agent-instance.work/1","id":id,"revision":1,"owner_agent_id":state.instance_config().instance_id,"title":body.title,"goal":body.goal,"assignment_id":body.assignment_id,"assignment":assignment,"template":template,"audience":audience,"source":body.source,"due_at":body.due_at,"state":"proposed","progress":"","blocker":null,"evidence":[],"verification":null,"delegations":[],"upstream":null,"created_at":at,"updated_at":at,"created_by":actor,"creation_context":binding.map(|b|json!({"provider_id":b["provider_id"],"conversation_id":b["conversation_id"],"actor_id":b["actor_id"],"owner":b["owner"],"authority":"verified_session_binding"})),"history":[{"at":at,"actor":actor,"action":"created","revision":1}]}),
    )
}
pub fn create_work(
    state: &AppState,
    body: NewWork,
    actor: &str,
    binding: Option<&Value>,
) -> ApiResult<Value> {
    create_work_with_input(state, body, actor, binding, None)
}

fn creation_binding(binding: Option<&Value>) -> Value {
    binding.map_or(Value::Null, |b| {
        json!({"provider_id":b["provider_id"],"conversation_id":b["conversation_id"],"thread_id":b["thread_id"],"actor_id":b["actor_id"],"owner":b["owner"]})
    })
}

fn create_work_with_input(
    state: &AppState,
    body: NewWork,
    actor: &str,
    binding: Option<&Value>,
    input: Option<&Value>,
) -> ApiResult<Value> {
    if let Some(request_id) = &body.request_id {
        text(request_id, 512)?;
    }
    // Keep the original request fingerprint separate from the mutable work.
    // The key identifies one creation, not every responsibility from a message.
    let creation = body.request_id.as_ref().map(|request_id| {
        // A renewed room grant may name another participant. Current access is
        // checked separately; it must not turn recovery of the same originating
        // input and creation request into another responsibility.
        let origin = input.map_or_else(
            || json!({"binding":creation_binding(binding)}),
            |i| json!({"principal":i["principal"],"binding":creation_binding(Some(&i["binding"]))}),
        );
        let input_fingerprint = input.map(|i| {
            json!({"principal":i["principal"],"sessionId":i["sessionId"],"inputId":i["inputId"],"sources":i["sources"],"binding":creation_binding(Some(&i["binding"]))})
        });
        json!({"request_id":request_id,
            "key":auth::digest(&json!([actor,origin,request_id]).to_string()),
            "fingerprint":auth::digest(&json!({"request":body,"source_input":input_fingerprint}).to_string())})
    });
    let value = {
        let mut db = state.db.lock().unwrap();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(creation) = &creation {
            let previous: Option<String> = tx
                .query_row(
                    "SELECT value FROM records WHERE collection='work' AND json_extract(value,'$.creation_request.key')=?1 LIMIT 1",
                    [creation["key"].as_str().unwrap()],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(raw) = previous {
                let previous: Value = serde_json::from_str(&raw)
                    .map_err(|_| ApiError::bad("Invalid stored responsibility record"))?;
                accessible(&previous, binding)?;
                if previous["creation_request"]["fingerprint"] != creation["fingerprint"] {
                    return Err(ApiError::conflict(json!({"error":"work_request_changed"})));
                }
                return Ok(previous);
            }
        }
        let mut value = work_value(state, &tx, body, actor, binding)?;
        if let Some(creation) = creation {
            value["creation_request"] = creation;
        }
        if let Some(input) = input {
            value["source_input"] = input.clone();
        }
        write(&tx, "work", value["id"].as_str().unwrap(), &value)?;
        tx.commit()?;
        value
    };
    state.event("work.created", json!({"id":value["id"],"actor":actor}))?;
    Ok(value)
}
fn accessible(v: &Value, binding: Option<&Value>) -> ApiResult<()> {
    let a: Audience = decode(v["audience"].clone())?;
    if !a.accessible(binding) {
        return Err(ApiError::missing());
    }
    Ok(())
}
pub fn get_work(state: &AppState, id: &str, binding: Option<&Value>) -> ApiResult<Value> {
    let value = read(&state.db.lock().unwrap(), "work", id)?.ok_or_else(ApiError::missing)?;
    accessible(&value, binding)?;
    Ok(value)
}
pub fn list_work(state: &AppState, binding: Option<&Value>) -> ApiResult<Value> {
    let items: Vec<_> = state
        .list_records("work")?
        .into_iter()
        .filter(|v| accessible(v, binding).is_ok())
        .collect();
    let org: Organization = decode(organization(state)?["organization"].clone())?;
    let assignments: Vec<_> = org
        .assignments
        .into_iter()
        .filter(|a| a.active && a.audience.accessible(binding))
        .collect();
    Ok(
        json!({"schema":"agent-instance.work-list/1","items":items,"assignments":assignments,"bounded":true,"limit":1000,"observed_at":now_ms()}),
    )
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub expected_revision: u64,
    pub command: Action,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    FreezeBrief {
        acceptance_criteria: Vec<String>,
        reviewer_actors: Vec<String>,
        source: Option<Source>,
    },
    ArtifactVersion {
        artifact: String,
        label: String,
        uri: String,
        version: String,
        sha256: String,
    },
    ReviewArtifacts {
        brief_sha256: String,
        artifact_set_sha256: String,
        outcome: String,
        criterion_indices: Vec<usize>,
        evidence_ids: Vec<String>,
        note: String,
    },
    Close {
        brief_sha256: String,
        artifact_set_sha256: String,
        note: String,
    },
    TransferRequester {
        requester_actor: String,
        note: String,
    },
    Accept,
    Start,
    Block {
        reason: String,
    },
    Resume,
    Progress {
        summary: String,
    },
    Amend {
        title: String,
        goal: String,
    },
    Evidence {
        label: String,
        uri: String,
        kind: String,
    },
    Review,
    Verify {
        outcome: String,
        evidence_ids: Vec<String>,
        note: String,
    },
    Complete,
    Cancel {
        reason: String,
    },
    Reopen {
        reason: String,
    },
}
fn evidence_uri(uri: &str) -> ApiResult<()> {
    text(uri, 2048)?;
    let u = reqwest::Url::parse(uri)
        .map_err(|_| ApiError::bad("Evidence needs an absolute http(s), file or urn reference"))?;
    if !["https", "http", "file", "urn"].contains(&u.scheme())
        || !u.username().is_empty()
        || u.password().is_some()
    {
        return Err(ApiError::bad("Unsupported evidence reference"));
    }
    Ok(())
}
pub(crate) fn mutate(value: &mut Value, action: Action, actor: &str) -> ApiResult<&'static str> {
    let current = value["state"].as_str().unwrap_or_default().to_owned();
    let active = !matches!(current.as_str(), "completed" | "cancelled");
    let invalid = || ApiError::bad("This transition is not valid for the work's current state");
    let name = match action {
        Action::FreezeBrief {
            acceptance_criteria,
            reviewer_actors,
            source,
        } if active => {
            if let Some(source) = source {
                source_validate(&source)?;
                if let Audience::Conversation {
                    provider_id,
                    conversation_id,
                } = decode(value["audience"].clone())?
                    && (source.provider_id != provider_id
                        || source.conversation_id != conversation_id)
                {
                    return Err(ApiError::bad(
                        "The brief's source must remain within this work's audience",
                    ));
                }
                value["source"] = json!(source);
            }
            completion::freeze(value, acceptance_criteria, reviewer_actors, actor)?;
            "brief_frozen"
        }
        Action::ArtifactVersion {
            artifact,
            label,
            uri,
            version,
            sha256,
        } if active => {
            completion::artifact(value, artifact, label, uri, version, sha256, actor)?;
            "artifact_version_recorded"
        }
        Action::ReviewArtifacts {
            brief_sha256,
            artifact_set_sha256,
            outcome,
            criterion_indices,
            evidence_ids,
            note,
        } if current == "review" => {
            completion::review(
                value,
                completion::ReviewReport {
                    brief_sha256,
                    artifact_set_sha256,
                    outcome,
                    criterion_indices,
                    evidence_ids,
                    note,
                },
                actor,
            )?;
            "artifact_reviewed"
        }
        Action::Close {
            brief_sha256,
            artifact_set_sha256,
            note,
        } if current == "review" => {
            completion::close(value, &brief_sha256, &artifact_set_sha256, note, actor)?;
            "requester_closed"
        }
        Action::TransferRequester {
            requester_actor,
            note,
        } if active => {
            completion::transfer_requester(value, requester_actor, note, actor)?;
            "requester_transferred"
        }
        Action::Accept if current == "proposed" => {
            value["state"] = json!("accepted");
            "accepted"
        }
        Action::Start if matches!(current.as_str(), "accepted" | "review") => {
            completion::ensure_current(value)?;
            value["state"] = json!("in_progress");
            value["verification"] = Value::Null;
            "started"
        }
        Action::Block { reason } if active && current != "proposed" => {
            text(&reason, 4096)?;
            value["blocker"] = json!(reason);
            value["state"] = json!("blocked");
            value["verification"] = Value::Null;
            "blocked"
        }
        Action::Resume if current == "blocked" => {
            value["state"] = json!("in_progress");
            value["blocker"] = Value::Null;
            value["verification"] = Value::Null;
            "resumed"
        }
        Action::Progress { summary } if active => {
            text(&summary, 8192)?;
            value["progress"] = json!(summary);
            value["verification"] = Value::Null;
            "progress"
        }
        Action::Amend { title, goal } if active => {
            text(&title, 240)?;
            text(&goal, 16 * 1024)?;
            value["title"] = json!(title);
            value["goal"] = json!(goal);
            value["verification"] = Value::Null;
            if current == "review" {
                value["state"] = json!("in_progress");
            }
            "amended"
        }
        Action::Evidence { label, uri, kind } if active => {
            text(&label, 500)?;
            evidence_uri(&uri)?;
            if !["artifact", "test", "review", "source"].contains(&kind.as_str()) {
                return Err(ApiError::bad("Unknown evidence kind"));
            }
            if value["evidence"].as_array().unwrap().len() >= 100 {
                return Err(ApiError::bad(
                    "At most 100 evidence references per work item",
                ));
            }
            value["evidence"].as_array_mut().unwrap().push(json!({"id":uuid::Uuid::new_v4().to_string(),"label":label,"uri":uri,"kind":kind,"recorded_by":actor,"at":now_ms(),"trust":"recorded_reference"}));
            value["verification"] = Value::Null;
            "evidence_added"
        }
        Action::Review if matches!(current.as_str(), "accepted" | "in_progress") => {
            value["state"] = json!("review");
            "submitted_for_review"
        }
        Action::Verify {
            outcome,
            evidence_ids,
            note,
        } if current == "review" => {
            if completion::enabled(value) {
                return Err(ApiError::bad(
                    "Use review_artifacts against this work's frozen brief and exact artifact set",
                ));
            }
            text(&note, 4096)?;
            if !["pass", "fail"].contains(&outcome.as_str())
                || evidence_ids.is_empty()
                || evidence_ids.iter().any(|id| {
                    !value["evidence"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|e| e["id"] == *id)
                })
            {
                return Err(ApiError::bad(
                    "Verification needs pass/fail and existing evidence IDs",
                ));
            }
            value["verification"] = json!({"outcome":outcome,"evidence_ids":evidence_ids,"note":note,"recorded_by":actor,"at":now_ms(),"authority":"authenticated_report","independently_verified":false});
            "verification_recorded"
        }
        Action::Complete if current == "review" && value["verification"]["outcome"] == "pass" => {
            if completion::enabled(value) {
                return Err(ApiError::bad(
                    "This work requires explicit requester closure through close",
                ));
            }
            value["state"] = json!("completed");
            "completed"
        }
        Action::Cancel { reason } if active => {
            text(&reason, 4096)?;
            value["progress"] = json!(reason);
            value["state"] = json!("cancelled");
            "cancelled"
        }
        Action::Reopen { reason } if !active => {
            text(&reason, 4096)?;
            value["progress"] = json!(reason);
            value["verification"] = Value::Null;
            value["blocker"] = Value::Null;
            value["state"] = json!("accepted");
            "reopened"
        }
        _ => return Err(invalid()),
    };
    Ok(name)
}
pub fn command_work(
    state: &AppState,
    id: &str,
    body: Command,
    actor: &str,
    binding: Option<&Value>,
) -> ApiResult<Value> {
    let (value, action) = {
        let db = state.db.lock().unwrap();
        let mut value = read(&db, "work", id)?.ok_or_else(ApiError::missing)?;
        accessible(&value, binding)?;
        if binding.is_some()
            && matches!(
                &body.command,
                Action::FreezeBrief { .. }
                    | Action::Close { .. }
                    | Action::TransferRequester { .. }
            )
        {
            return Err(ApiError::bad(
                "Freezing requester policy and closing work require the authenticated management principal, not a conversation tool session",
            ));
        }
        if value["revision"] != body.expected_revision {
            return Err(ApiError::conflict(json!({"revision":value["revision"]})));
        }
        let action = mutate(&mut value, body.command, actor)?;
        record_change(&mut value, actor, action)?;
        write(&db, "work", id, &value)?;
        (value, action)
    };
    state.event(
        "work.changed",
        json!({"id":id,"revision":value["revision"],"action":action,"actor":actor}),
    )?;
    Ok(value)
}
pub(crate) fn record_change(value: &mut Value, actor: &str, action: &str) -> ApiResult<()> {
    if value["history"].as_array().unwrap().len() >= 2000 {
        return Err(ApiError::bad(
            "This work item has reached its 2000 change limit; create a follow-up",
        ));
    }
    let revision = value["revision"].as_u64().unwrap() + 1;
    completion::changed(value, action)?;
    let at = now_ms();
    value["revision"] = json!(revision);
    value["updated_at"] = json!(at);
    value["history"]
        .as_array_mut()
        .unwrap()
        .push(json!({"at":at,"actor":actor,"action":action,"revision":revision}));
    Ok(())
}
pub async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    read_auth(&state, &headers)?;
    Ok(Json(list_work(&state, None)?))
}
pub async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    read_auth(&state, &headers)?;
    Ok(Json(get_work(&state, &id, None)?))
}
pub async fn create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<NewWork>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "work", true)?.actor;
    Ok(Json(create_work(&state, body, &actor, None)?))
}
pub async fn command(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Command>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "work", true)?.actor;
    if let Action::TransferRequester {
        requester_actor, ..
    } = &body.command
    {
        auth::check(&state, &headers, "admin", true)?;
        if requester_actor != &state.instance_config().owner {
            let id = requester_actor.strip_prefix("token:").ok_or_else(|| {
                ApiError::bad("Choose the OS owner or a current management token principal")
            })?;
            let scopes: Option<String> = state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT scopes FROM tokens WHERE id=?1 AND expires_at>?2",
                    params![id, now_ms()],
                    |row| row.get(0),
                )
                .optional()?;
            let scopes: Vec<String> =
                serde_json::from_str(&scopes.unwrap_or_default()).unwrap_or_default();
            if !scopes.iter().any(|s| s == "admin" || s == "work") {
                return Err(ApiError::bad(
                    "The new requester token needs current work or admin authority",
                ));
            }
        }
    }
    Ok(Json(command_work(&state, &id, body, &actor, None)?))
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Export {
    expected_revision: u64,
    relationship_id: String,
    disclosure: String,
    summary: String,
}
fn relationship(
    db: &Connection,
    id: &str,
    assignment: &str,
    peer: Option<&str>,
    send: bool,
    details: bool,
) -> ApiResult<Relationship> {
    let org: Organization = decode(organization_db(db)?["organization"].clone())?;
    let r = org
        .relationships
        .into_iter()
        .find(|r| r.id == id && r.active)
        .ok_or_else(ApiError::forbidden)?;
    if !r.assignment_ids.iter().any(|id| id == assignment)
        || peer.is_some_and(|p| p != r.peer_agent_id)
        || if send { !r.send } else { !r.receive }
        || (details && !r.allow_details)
    {
        return Err(ApiError::forbidden());
    }
    Ok(r)
}
pub async fn export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Export>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "work", true)?.actor;
    text(&body.summary, 4096)?;
    if !["summary", "details"].contains(&body.disclosure.as_str()) {
        return Err(ApiError::bad("Choose summary or details"));
    }
    let packet = {
        let mut db = state.db.lock().unwrap();
        let tx = db.transaction()?;
        let mut work = read(&tx, "work", &id)?.ok_or_else(ApiError::missing)?;
        if work["revision"] != body.expected_revision {
            return Err(ApiError::conflict(json!({"revision":work["revision"]})));
        }
        if work["upstream"].is_object() {
            return Err(ApiError::bad(
                "Imported material cannot be forwarded; request a separate grant from its source",
            ));
        }
        let r = relationship(
            &tx,
            &body.relationship_id,
            work["assignment_id"].as_str().unwrap_or_default(),
            None,
            true,
            body.disclosure == "details",
        )?;
        let export_id = uuid::Uuid::new_v4().to_string();
        let packet = json!({"schema":"agent-instance.delegation/1","id":export_id,"source_agent_id":state.instance_config().instance_id,"source_work_id":id,"source_revision":body.expected_revision,"target_agent_id":r.peer_agent_id,"relationship_id":r.id,"source_audience":work["audience"],"source":work["source"],"disclosure":body.disclosure,"summary":body.summary,"details":if body.disclosure=="details" {json!({"title":work["title"],"goal":work["goal"],"evidence":work["evidence"]})}else{Value::Null},"sharing":{"forwarding":false,"scope":"named_recipient_only"},"created_at":now_ms(),"created_by":actor});
        work["delegations"].as_array_mut().unwrap().push(json!({"id":export_id,"target_agent_id":r.peer_agent_id,"relationship_id":r.id,"created_at":now_ms(),"delivery":"not_confirmed"}));
        record_change(&mut work, &actor, "delegation_prepared")?;
        write(&tx, "work", &id, &work)?;
        write(&tx, "work_exports", &export_id, &packet)?;
        tx.commit()?;
        packet
    };
    state.event("work.exported", json!({"id":id,"actor":actor}))?;
    Ok(Json(packet))
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Import {
    relationship_id: String,
    assignment_id: String,
    packet: Packet,
}
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Packet {
    schema: String,
    id: String,
    source_agent_id: String,
    source_work_id: String,
    source_revision: u64,
    target_agent_id: String,
    relationship_id: String,
    source_audience: Audience,
    source: Option<Source>,
    disclosure: String,
    summary: String,
    details: Option<Details>,
    sharing: Sharing,
    created_at: i64,
    created_by: String,
}
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Details {
    title: String,
    goal: String,
    evidence: Vec<Value>,
}
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sharing {
    forwarding: bool,
    scope: String,
}
pub async fn import(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Import>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "work", true)?.actor;
    let p = &body.packet;
    if p.schema != "agent-instance.delegation/1"
        || p.target_agent_id != state.instance_config().instance_id
        || p.sharing.forwarding
        || p.sharing.scope != "named_recipient_only"
        || !["summary", "details"].contains(&p.disclosure.as_str())
        || (p.disclosure == "details") != p.details.is_some()
    {
        return Err(ApiError::bad("Invalid delegation envelope or recipient"));
    }
    uuid::Uuid::parse_str(&p.id).map_err(|_| ApiError::bad("Invalid delegation ID"))?;
    text(&p.source_agent_id, 512)?;
    text(&p.source_work_id, 512)?;
    text(&p.summary, 4096)?;
    p.source_audience.validate()?;
    if let Some(s) = &p.source {
        source_validate(s)?;
    }
    if let Some(d) = &p.details {
        text(&d.title, 240)?;
        text(&d.goal, 16 * 1024)?;
        if d.evidence.len() > 100 || serde_json::to_string(&d.evidence).unwrap().len() > 64 * 1024 {
            return Err(ApiError::bad("Too much imported evidence"));
        }
    }
    let fingerprint = auth::digest(&serde_json::to_string(p).unwrap());
    let key = format!("{}:{}", p.source_agent_id, p.id);
    let result = {
        let mut db = state.db.lock().unwrap();
        let tx = db.transaction()?;
        relationship(
            &tx,
            &body.relationship_id,
            &body.assignment_id,
            Some(&p.source_agent_id),
            false,
            p.disclosure == "details",
        )?;
        if let Some(old) = read(&tx, "work_imports", &key)? {
            if old["fingerprint"] != fingerprint {
                return Err(ApiError::bad(
                    "Delegation ID was reused with different contents",
                ));
            }
            let item = read(&tx, "work", old["work_id"].as_str().unwrap())?
                .ok_or_else(ApiError::missing)?;
            return Ok(Json(json!({"duplicate":true,"work":item})));
        }
        let org: Organization = decode(organization_db(&tx)?["organization"].clone())?;
        let assignment = org
            .assignments
            .iter()
            .find(|a| a.id == body.assignment_id && a.active)
            .ok_or_else(|| ApiError::bad("Choose an active assignment"))?;
        // An explicitly received packet begins private, proposed work; its claims are never executable instructions or completion proof.
        if assignment.audience != Audience::Owner {
            return Err(ApiError::bad(
                "Receive delegated material into an owner-only assignment; publication needs separate review",
            ));
        }
        let mut item = work_value(
            &state,
            &tx,
            NewWork {
                title: p
                    .details
                    .as_ref()
                    .map(|d| d.title.clone())
                    .unwrap_or_else(|| p.summary.chars().take(120).collect()),
                goal: p
                    .details
                    .as_ref()
                    .map(|d| d.goal.clone())
                    .unwrap_or_else(|| p.summary.clone()),
                assignment_id: Some(body.assignment_id),
                audience: Some(assignment.audience.clone()),
                source: None,
                due_at: None,
                request_id: None,
            },
            &actor,
            None,
        )?;
        item["upstream"] = json!({"envelope":p,"imported_by":actor,"source_identity":"claimed_by_envelope","authenticated_transport":false,"trust":"unverified_remote_report","forwarding":false});
        let id = item["id"].as_str().unwrap();
        write(&tx, "work", id, &item)?;
        write(
            &tx,
            "work_imports",
            &key,
            &json!({"fingerprint":fingerprint,"work_id":id}),
        )?;
        tx.commit()?;
        item
    };
    state.event("work.imported", json!({"id":result["id"],"actor":actor}))?;
    Ok(Json(json!({"duplicate":false,"work":result})))
}

pub fn orientation(state: &AppState, binding: &Value) -> ApiResult<Value> {
    let all = list_work(state, Some(binding))?;
    let items: Vec<Value> = all["items"].as_array().unwrap().iter()
        .filter(|item| !["completed", "cancelled"].contains(&item["state"].as_str().unwrap_or("")))
        .take(12).map(|item| json!({"id":item["id"],"title":item["title"],"state":item["state"],"revision":item["revision"],"assignment_id":item["assignment_id"]})).collect();
    Ok(
        json!({"items":items,"bounded":true,"instructions":"Use work_get for the goal, captured role instructions and evidence. Record explicitly authorized work and actual progress with work tools. Accepting a record does not authorize execution. Background execution requires a separate owner grant; work_execution inspects its actual state. Do not claim completion without checking the artifact; a saved reference or another agent's report is not independent proof."}),
    )
}
pub fn mcp_tools() -> Vec<Value> {
    vec![
        json!({"name":"work_execution","description":"Inspect authorized background runs and their reports for one accessible responsibility. Reports are unverified; this tool cannot authorize, retry, or approve work.","inputSchema":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}}),
        json!({"name":"work_list","description":"Find your durable responsibilities and assignments accessible to this conversation. This is local work state, not a scheduler or proof of external completion.","inputSchema":{"type":"object","properties":{},"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}}),
        json!({"name":"work_get","description":"Read one accessible responsibility, its goal, captured role instructions, progress, evidence and recorded review.","inputSchema":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}}),
        json!({"name":"work_create","description":"Record a proposed responsibility explicitly authorized by the user. Creating a record does not execute work or make a promise on the user's behalf. Audience is automatically restricted to this conversation (or private owner context). Use a stable request_id for one creation so a lost response can be recovered without duplicate work. To link a registered original input, supply its source_input_id and a request_id; never invent input identities. Different responsibilities from the same input need different request IDs.","inputSchema":{"type":"object","properties":{"title":{"type":"string"},"goal":{"type":"string"},"assignment_id":{"type":"string"},"request_id":{"type":"string","maxLength":512},"source_input_id":{"type":"string","maxLength":512}},"required":["title","goal"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":false}}),
        json!({"name":"work_command","description":"Update accessible work using expected_revision. Commands: accept/start/block(reason)/resume/progress(summary)/amend(title,goal)/evidence(label,uri,kind artifact|test|review|source)/review/verify(outcome pass|fail,evidence_ids,note)/complete/cancel(reason)/reopen(reason). Optional completion_contract requires artifact_version(artifact,label,uri,version,sha256), then review_artifacts(brief_sha256,artifact_set_sha256,outcome,criterion_indices,evidence_ids,note) by its named reviewers. Requester freeze_brief and close require management authentication, not this conversation tool. Inspect work_get for current hashes, review round and zero-based acceptance criteria. Never invent execution or approval; evidence and reviews are authenticated reports, not independently verified by this store.","inputSchema":{"type":"object","properties":{"id":{"type":"string"},"expected_revision":{"type":"integer"},"command":{"type":"object"}},"required":["id","expected_revision","command"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":false}}),
    ]
}
pub fn mcp_call(state: &AppState, binding: &Value, name: &str, args: &Value) -> ApiResult<Value> {
    mcp_call_with_input(state, binding, name, args, None)
}

/// `input` is resolved by the authenticated context lease, never supplied by a tool caller.
pub fn mcp_call_with_input(
    state: &AppState,
    binding: &Value,
    name: &str,
    args: &Value,
    input: Option<&Value>,
) -> ApiResult<Value> {
    let actor = format!("agent:{}", state.instance_config().instance_id);
    match name {
        "work_execution" => crate::scheduler::inspect_work(
            state,
            args["id"]
                .as_str()
                .ok_or_else(|| ApiError::bad("id required"))?,
            Some(binding),
        ),
        "work_list" => list_work(state, Some(binding)),
        "work_get" => get_work(
            state,
            args["id"]
                .as_str()
                .ok_or_else(|| ApiError::bad("id required"))?,
            Some(binding),
        ),
        "work_create" => {
            let mut body = args.clone();
            if body.get("audience").is_some()
                || body.get("source").is_some()
                || body.get("source_input").is_some()
            {
                return Err(ApiError::bad(
                    "Work tool audience is derived from this session",
                ));
            }
            let source_input_id = body
                .as_object_mut()
                .and_then(|o| o.remove("source_input_id"));
            let input = if let Some(id) = source_input_id {
                let id = id
                    .as_str()
                    .ok_or_else(|| ApiError::bad("source_input_id must be a string"))?;
                text(id, 512)?;
                if body["request_id"].as_str().is_none() {
                    return Err(ApiError::bad(
                        "Linking an original input requires a creation request_id",
                    ));
                }
                let input = input
                    .filter(|i| {
                        i["inputId"] == id && crate::context::same_binding(&i["binding"], binding)
                    })
                    .ok_or_else(|| {
                        ApiError::bad(
                            "Original input is not registered for this authenticated session",
                        )
                    })?;
                Some(input)
            } else {
                None
            };
            body["audience"] = json!(Audience::for_binding(binding));
            if binding["provider_id"].is_string() && binding["conversation_id"].is_string() {
                let message_id = input
                    .and_then(|i| i["sources"].as_array())
                    .filter(|sources| sources.len() == 1)
                    .map(|sources| &sources[0]["id"]);
                body["source"] = json!({"provider_id":binding["provider_id"],"conversation_id":binding["conversation_id"],"message_id":message_id,"url":null});
            }
            create_work_with_input(state, decode(body)?, &actor, Some(binding), input)
        }
        "work_command" => {
            let mut body = args.clone();
            let id = body
                .as_object_mut()
                .and_then(|o| o.remove("id"))
                .and_then(|v| v.as_str().map(str::to_owned))
                .ok_or_else(|| ApiError::bad("id required"))?;
            command_work(state, &id, decode(body)?, &actor, Some(binding))
        }
        _ => Err(ApiError::bad("Unknown work tool")),
    }
}
