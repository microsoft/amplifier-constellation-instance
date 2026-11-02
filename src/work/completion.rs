//! Optional completion policy. Exact references and authenticated reports are
//! recorded here; artifact fetching, execution and content verification are not.
use super::{ApiError, ApiResult, Value, auth, evidence_uri, json, now_ms, text};
use std::collections::{BTreeMap, BTreeSet};

const SCHEMA: &str = "agent-instance.completion-contract/1";

pub(super) fn enabled(work: &Value) -> bool {
    work["completion_contract"].is_object()
}

fn snapshot(work: &Value) -> Value {
    json!({"title":work["title"],"goal":work["goal"],"source":work["source"],
        "audience":work["audience"],"assignment":work["assignment"],"template":work["template"]})
}

fn hash(value: &Value) -> String {
    auth::digest(&value.to_string())
}

pub(super) fn ensure_current(work: &Value) -> ApiResult<()> {
    if enabled(work) && work["completion_contract"]["brief"]["snapshot"] != snapshot(work) {
        return Err(ApiError::bad(
            "The responsibility changed after its brief was frozen; its requester must freeze a new brief",
        ));
    }
    Ok(())
}

fn artifact_set(work: &Value) -> Value {
    let mut latest = BTreeMap::new();
    for evidence in work["evidence"].as_array().into_iter().flatten() {
        if let Some(key) = evidence["artifact"].as_str() {
            latest.insert(
                key,
                json!({"artifact":key,"evidence_id":evidence["id"],
                "uri":evidence["uri"],"version":evidence["version"],"sha256":evidence["sha256"],
                "recorded_by":evidence["recorded_by"]}),
            );
        }
    }
    let items = json!(latest.into_values().collect::<Vec<_>>());
    json!({"sha256":hash(&items),"artifacts":items})
}

pub(super) fn freeze(
    work: &mut Value,
    criteria: Vec<String>,
    reviewers: Vec<String>,
    actor: &str,
) -> ApiResult<()> {
    if work["source"].is_null()
        || (work["source"]["message_id"].is_null() && work["source"]["url"].is_null())
    {
        return Err(ApiError::bad(
            "A completion brief needs a specific source message or URL on the responsibility",
        ));
    }
    if criteria.is_empty() || criteria.len() > 32 || reviewers.is_empty() || reviewers.len() > 16 {
        return Err(ApiError::bad(
            "Supply 1–32 acceptance criteria and 1–16 distinct reviewer principals",
        ));
    }
    for criterion in &criteria {
        text(criterion, 2000)?;
    }
    for reviewer in &reviewers {
        text(reviewer, 200)?;
    }
    if reviewers.iter().collect::<BTreeSet<_>>().len() != reviewers.len() {
        return Err(ApiError::bad(
            "Each reviewer principal may appear only once",
        ));
    }
    let previous = work["completion_contract"].clone();
    if enabled(work) && previous["requester_actor"] != actor {
        return Err(ApiError::forbidden());
    }
    let mut history = previous["brief_versions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if history.len() >= 32 {
        return Err(ApiError::bad(
            "This work has reached 32 frozen briefs; create a follow-up",
        ));
    }
    let version = history.len() + 1;
    let body = json!({"schema":SCHEMA,"version":version,"snapshot":snapshot(work),
        "acceptance_criteria":criteria,"reviewer_actors":reviewers,"requester_actor":actor});
    let mut brief = body.clone();
    brief["sha256"] = json!(hash(&body));
    brief["frozen_at"] = json!(now_ms());
    history.push(brief.clone());
    work["completion_contract"] = json!({"schema":SCHEMA,"brief":brief,"brief_versions":history,
        "requester_actor":actor,"requester_transfers":previous["requester_transfers"].as_array().cloned().unwrap_or_default(),
        "artifact_set":artifact_set(work),"review_round":previous["review_round"].as_u64().unwrap_or(0)+1,
        "reviews":previous["reviews"].as_array().cloned().unwrap_or_default(),"closure":null});
    work["verification"] = Value::Null;
    if work["state"] == "review" {
        work["state"] = json!("in_progress");
    }
    Ok(())
}

pub(super) fn artifact(
    work: &mut Value,
    key: String,
    label: String,
    uri: String,
    version: String,
    sha256: String,
    actor: &str,
) -> ApiResult<()> {
    if !enabled(work) {
        return Err(ApiError::bad(
            "Freeze an acceptance brief before recording versioned artifacts",
        ));
    }
    text(&key, 120)?;
    text(&label, 500)?;
    text(&version, 200)?;
    evidence_uri(&uri)?;
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(ApiError::bad(
            "Artifact SHA-256 must contain 64 lowercase hexadecimal characters",
        ));
    }
    let evidence = work["evidence"].as_array_mut().unwrap();
    if evidence.len() >= 100 {
        return Err(ApiError::bad(
            "At most 100 evidence references per work item",
        ));
    }
    evidence.push(
        json!({"id":uuid::Uuid::new_v4().to_string(),"artifact":key,"label":label,"uri":uri,
        "kind":"artifact","version":version,"sha256":sha256,"recorded_by":actor,"at":now_ms(),
        "trust":"reported_content_hash","content_verified_by_store":false}),
    );
    Ok(())
}

fn target(work: &Value, brief_hash: &str, artifact_hash: &str) -> ApiResult<()> {
    if !enabled(work) {
        return Err(ApiError::bad(
            "This responsibility has no completion contract",
        ));
    }
    ensure_current(work)?;
    let current = artifact_set(work);
    if work["completion_contract"]["brief"]["sha256"] != brief_hash
        || current["sha256"] != artifact_hash
    {
        return Err(ApiError::conflict(
            json!({"brief_sha256":work["completion_contract"]["brief"]["sha256"],"artifact_set_sha256":current["sha256"]}),
        ));
    }
    if current["artifacts"].as_array().is_none_or(Vec::is_empty) {
        return Err(ApiError::bad(
            "Record at least one exact artifact version before review",
        ));
    }
    Ok(())
}

pub(super) struct ReviewReport {
    pub brief_sha256: String,
    pub artifact_set_sha256: String,
    pub outcome: String,
    pub criterion_indices: Vec<usize>,
    pub evidence_ids: Vec<String>,
    pub note: String,
}

pub(super) fn review(work: &mut Value, report: ReviewReport, actor: &str) -> ApiResult<()> {
    let ReviewReport {
        brief_sha256,
        artifact_set_sha256,
        outcome,
        criterion_indices,
        evidence_ids,
        note,
    } = report;
    target(work, &brief_sha256, &artifact_set_sha256)?;
    let brief = &work["completion_contract"]["brief"];
    if !brief["reviewer_actors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == actor)
    {
        return Err(ApiError::forbidden());
    }
    if artifact_set(work)["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["recorded_by"] == actor)
    {
        return Err(ApiError::bad(
            "A required reviewer must be distinct from every artifact-recording principal",
        ));
    }
    text(&note, 4096)?;
    let mut indices = criterion_indices.clone();
    indices.sort_unstable();
    if !["pass", "fail"].contains(&outcome.as_str())
        || indices
            != (0..brief["acceptance_criteria"].as_array().unwrap().len()).collect::<Vec<_>>()
    {
        return Err(ApiError::bad(
            "Review must report pass/fail and cover each zero-based acceptance criterion exactly once",
        ));
    }
    if evidence_ids.is_empty()
        || evidence_ids.len() > 100
        || evidence_ids.iter().collect::<BTreeSet<_>>().len() != evidence_ids.len()
        || evidence_ids.iter().any(|id| {
            !work["evidence"].as_array().unwrap().iter().any(|e| {
                e["id"] == *id
                    && ["test", "review"].contains(&e["kind"].as_str().unwrap_or_default())
            })
        })
    {
        return Err(ApiError::bad(
            "Review needs distinct existing test or review evidence IDs",
        ));
    }
    let round = work["completion_contract"]["review_round"].clone();
    let reviews = work["completion_contract"]["reviews"]
        .as_array_mut()
        .unwrap();
    if reviews.len() >= 128 {
        return Err(ApiError::bad(
            "This work has reached 128 review reports; create a follow-up",
        ));
    }
    reviews.push(
        json!({"id":uuid::Uuid::new_v4().to_string(),"brief_sha256":brief_sha256,
        "artifact_set_sha256":artifact_set_sha256,"review_round":round,"outcome":outcome,
        "criterion_indices":criterion_indices,"evidence_ids":evidence_ids,"note":note,
        "recorded_by":actor,"at":now_ms(),"authority":"authenticated_principal_report",
        "independently_verified":false}),
    );
    work["verification"] = json!({"outcome":if all_pass(work) {"pass"} else {"pending_or_failed"},
        "brief_sha256":brief_sha256,"artifact_set_sha256":artifact_set_sha256,"review_round":round,
        "authority":"authenticated_group_reports","independently_verified":false});
    Ok(())
}

fn all_pass(work: &Value) -> bool {
    let contract = &work["completion_contract"];
    let brief = &contract["brief"];
    brief["reviewer_actors"]
        .as_array()
        .is_some_and(|reviewers| {
            reviewers.iter().all(|actor| {
                contract["reviews"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .rev()
                    .find(|r| {
                        r["recorded_by"] == *actor
                            && r["brief_sha256"] == brief["sha256"]
                            && r["artifact_set_sha256"] == artifact_set(work)["sha256"]
                            && r["review_round"] == contract["review_round"]
                    })
                    .is_some_and(|r| r["outcome"] == "pass")
            })
        })
}

pub(super) fn close(
    work: &mut Value,
    brief_hash: &str,
    artifact_hash: &str,
    note: String,
    actor: &str,
) -> ApiResult<()> {
    target(work, brief_hash, artifact_hash)?;
    if work["completion_contract"]["requester_actor"] != actor {
        return Err(ApiError::forbidden());
    }
    if !all_pass(work) {
        return Err(ApiError::bad(
            "All named reviewers must pass the current brief and artifact versions before requester closure",
        ));
    }
    text(&note, 4096)?;
    work["completion_contract"]["closure"] = json!({"brief_sha256":brief_hash,"artifact_set_sha256":artifact_hash,
        "review_round":work["completion_contract"]["review_round"],"note":note,"recorded_by":actor,
        "at":now_ms(),"authority":"authenticated_requester_principal","human_identity_inferred":false});
    work["state"] = json!("completed");
    Ok(())
}

/// Only the administrative management handler may call this path. It records
/// explicit recovery/reassignment without altering an already frozen brief.
pub(super) fn transfer_requester(
    work: &mut Value,
    requester: String,
    note: String,
    actor: &str,
) -> ApiResult<()> {
    if !enabled(work) {
        return Err(ApiError::bad(
            "This responsibility has no completion contract",
        ));
    }
    text(&requester, 200)?;
    text(&note, 4096)?;
    let previous = work["completion_contract"]["requester_actor"].clone();
    let transfers = work["completion_contract"]["requester_transfers"]
        .as_array_mut()
        .unwrap();
    if transfers.len() >= 32 {
        return Err(ApiError::bad(
            "This responsibility has reached 32 requester transfers; create a follow-up",
        ));
    }
    transfers.push(
        json!({"from":previous,"to":requester,"recorded_by":actor,"note":note,"at":now_ms(),
        "authority":"authenticated_admin_reassignment"}),
    );
    work["completion_contract"]["requester_actor"] = json!(requester);
    Ok(())
}

pub(super) fn changed(work: &mut Value, action: &str) -> ApiResult<()> {
    if !enabled(work) {
        return Ok(());
    }
    let set = artifact_set(work);
    work["completion_contract"]["artifact_set"] = set;
    if ![
        "brief_frozen",
        "artifact_reviewed",
        "requester_closed",
        "submitted_for_review",
        "accepted",
    ]
    .contains(&action)
    {
        let round = work["completion_contract"]["review_round"]
            .as_u64()
            .ok_or_else(|| ApiError::bad("Invalid stored completion contract"))?;
        work["completion_contract"]["review_round"] = json!(round + 1);
        work["completion_contract"]["closure"] = Value::Null;
        work["verification"] = Value::Null;
    }
    Ok(())
}
