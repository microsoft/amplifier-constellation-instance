use agent_instance::{
    auth, host, profile_use, profiles, server,
    state::{AppState, now_ms},
    work,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use std::sync::Arc;
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
        host::init(
            dir.path(),
            "Work fixture".into(),
            None,
            "127.0.0.1".into(),
            0,
        )
        .unwrap();
        let state = Arc::new(AppState::open(dir.path().to_owned()).unwrap());
        let token = std::fs::read_to_string(dir.path().join("secrets/control-token")).unwrap();
        let router = server::router(state.clone());
        Self {
            dir,
            state,
            router,
            token,
        }
    }
    async fn req(
        &self,
        method: &str,
        path: &str,
        body: Value,
        token: Option<&str>,
    ) -> (u16, Value) {
        let mut r = Request::builder()
            .method(method)
            .uri(format!("/api/v1/{path}"))
            .header("host", "localhost")
            .header("content-type", "application/json");
        if let Some(t) = token {
            r = r.header("authorization", format!("Bearer {t}"));
        }
        let response = self
            .router
            .clone()
            .oneshot(
                r.body(Body::from(if body.is_null() {
                    String::new()
                } else {
                    body.to_string()
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status().as_u16();
        let b = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }
    async fn api(&self, method: &str, path: &str, body: Value) -> Value {
        let (status, v) = self.req(method, path, body, Some(&self.token)).await;
        assert_eq!(status, 200, "{path} {v}");
        v
    }
    async fn command(&self, item: &Value, command: Value) -> Value {
        self.api(
            "POST",
            &format!("work/{}/commands", item["id"].as_str().unwrap()),
            json!({"expected_revision":item["revision"],"command":command}),
        )
        .await
    }
    async fn org(&self, peer: &str, details: bool) -> Value {
        self.api("PUT","organization",json!({"expected_revision":0,"organization":{"templates":[{"id":"engineer","version":"1","name":"Engineer","instructions":"Check the actual changed artifact before reporting."}],"assignments":[assignment("work"),assignment("personal")],"relationships":[{"id":"coordinator","label":"Coordinator","peer_agent_id":peer,"assignment_ids":["work"],"send":true,"receive":true,"allow_details":details}]}})).await
    }
    async fn create(&self) -> Value {
        self.api("POST","work",json!({"title":"Check changed output","goal":"A real check of the output","assignment_id":"work","audience":{"kind":"owner"}})).await
    }
}
fn assignment(id: &str) -> Value {
    json!({"id":id,"name":id,"domain":id,"purpose":"Own this responsibility","template_id":"engineer","audience":{"kind":"owner"}})
}

#[tokio::test]
async fn creation_retry_recovers_current_work_without_repeating_or_replacing_it() {
    let f = Fixture::new();
    let request = json!({"title":"First deliverable","goal":"Verify the result","request_id":"create-one",
        "source":{"provider_id":"slack","conversation_id":"room","message_id":"message-one"}});
    let created = f.api("POST", "work", request.clone()).await;
    assert_eq!(created["state"], "proposed");
    let accepted = f.command(&created, json!({"type":"accept"})).await;
    let amended = f.command(&accepted, json!({"type":"amend","title":"Corrected deliverable","goal":"Use the corrected scope"})).await;
    let recovered = f.api("POST", "work", request.clone()).await;
    assert_eq!(recovered, amended);
    assert_eq!(recovered["history"].as_array().unwrap().len(), 3);
    let mut changed = request.clone();
    changed["goal"] = json!("Another creation with the same key is a conflict");
    let (status, _) = f.req("POST", "work", changed, Some(&f.token)).await;
    assert_eq!(status, 409);

    let mut separate = request.clone();
    separate["request_id"] = json!("create-two");
    let second = f.api("POST", "work", separate).await;
    assert_ne!(created["id"], second["id"]);
    assert_eq!(created["source"], second["source"]);
    assert_eq!(
        f.api("GET", "work", Value::Null).await["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let other = auth::issue_token(&f.state, "Other creator", &["work".into()], 3600).unwrap();
    let (status, other_work) = f
        .req("POST", "work", request, other["token"].as_str())
        .await;
    assert_eq!(status, 200);
    assert_ne!(other_work["id"], created["id"]);
    let legacy = json!({"title":"Legacy caller","goal":"No request key was supplied"});
    let first = f.api("POST", "work", legacy.clone()).await;
    assert_ne!(first["id"], f.api("POST", "work", legacy).await["id"]);
}

#[test]
fn concurrent_creation_retries_commit_one_existing_work_record() {
    let f = Fixture::new();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let results = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..8).map(|_| {
            let state = f.state.clone();
            let barrier = barrier.clone();
            scope.spawn(move || {
                let request = serde_json::from_value(json!({"title":"One operation","goal":"One creation despite concurrent retries","request_id":"same-creation"})).unwrap();
                barrier.wait();
                work::create_work(&state, request, "owner", None).unwrap()
            })
        }).collect();
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(results.iter().all(|item| item == &results[0]));
    assert_eq!(f.state.list_records("work").unwrap().len(), 1);
}

#[test]
fn source_linked_creation_requires_a_verified_input_and_keeps_one_to_many_provenance() {
    let f = Fixture::new();
    let binding =
        json!({"owner":false,"provider_id":"slack","conversation_id":"room","actor_id":"person"});
    let input = json!({"principal":"token:connector","sessionId":"session-one","inputId":"input-one",
        "sources":[{"id":"message-one","revision":"revision-one","occurredAt":now_ms()}],"binding":binding});
    let request = json!({"title":"First part","goal":"Use the original request","source_input_id":"input-one","request_id":"part-one"});
    assert!(work::mcp_call(&f.state, &binding, "work_create", &request).is_err());
    let mut no_key = request.clone();
    no_key.as_object_mut().unwrap().remove("request_id");
    assert!(
        work::mcp_call_with_input(&f.state, &binding, "work_create", &no_key, Some(&input))
            .is_err()
    );
    let mut wrong_input = input.clone();
    wrong_input["inputId"] = json!("another-input");
    assert!(
        work::mcp_call_with_input(
            &f.state,
            &binding,
            "work_create",
            &request,
            Some(&wrong_input)
        )
        .is_err()
    );
    let mut injected = request.clone();
    injected["source_input"] = input.clone();
    assert!(
        work::mcp_call_with_input(&f.state, &binding, "work_create", &injected, Some(&input))
            .is_err()
    );

    let first =
        work::mcp_call_with_input(&f.state, &binding, "work_create", &request, Some(&input))
            .unwrap();
    assert_eq!(first["state"], "proposed");
    assert_eq!(first["source"]["message_id"], "message-one");
    assert_eq!(first["source_input"], input);
    let accepted = work::mcp_call(
        &f.state,
        &binding,
        "work_command",
        &json!({"id":first["id"],"expected_revision":1,"command":{"type":"accept"}}),
    )
    .unwrap();
    let retried =
        work::mcp_call_with_input(&f.state, &binding, "work_create", &request, Some(&input))
            .unwrap();
    assert_eq!(retried, accepted);
    let mut refreshed = binding.clone();
    refreshed["label"] = json!("Current display name");
    refreshed["observed_at"] = json!(now_ms());
    let refreshed_retry =
        work::mcp_call_with_input(&f.state, &refreshed, "work_create", &request, Some(&input))
            .unwrap();
    assert_eq!(refreshed_retry, accepted);
    assert_eq!(refreshed_retry["source_input"], input);
    let mut second_request = request.clone();
    second_request["request_id"] = json!("part-two");
    let second = work::mcp_call_with_input(
        &f.state,
        &binding,
        "work_create",
        &second_request,
        Some(&input),
    )
    .unwrap();
    assert_ne!(first["id"], second["id"]);
    assert_eq!(first["source_input"], second["source_input"]);
    assert_eq!(f.state.list_records("work").unwrap().len(), 2);

    let mut changed_revision = input.clone();
    changed_revision["sources"][0]["revision"] = json!("revision-two");
    assert_eq!(
        work::mcp_call_with_input(
            &f.state,
            &binding,
            "work_create",
            &request,
            Some(&changed_revision)
        )
        .unwrap_err()
        .0,
        409
    );
    let mut other_binding = binding.clone();
    other_binding["conversation_id"] = json!("other-room");
    assert!(
        work::mcp_call_with_input(
            &f.state,
            &other_binding,
            "work_create",
            &request,
            Some(&input)
        )
        .is_err()
    );
    assert_eq!(
        work::get_work(
            &f.state,
            first["id"].as_str().unwrap(),
            Some(&other_binding)
        )
        .unwrap_err()
        .0,
        404
    );
    let mut other_input = input.clone();
    other_input["binding"] = other_binding.clone();
    let other = work::mcp_call_with_input(
        &f.state,
        &other_binding,
        "work_create",
        &request,
        Some(&other_input),
    )
    .unwrap();
    assert_ne!(other["id"], first["id"]);

    let mut other_actor = binding.clone();
    other_actor["actor_id"] = json!("another-person");
    let participant_work = work::mcp_call_with_input(
        &f.state,
        &other_actor,
        "work_create",
        &request,
        Some(&input),
    )
    .unwrap();
    assert_eq!(participant_work, accepted);
    assert_eq!(
        participant_work["source_input"]["binding"]["actor_id"],
        "person"
    );
    assert_eq!(participant_work["creation_context"]["actor_id"], "person");
    let mut participant_request = request.clone();
    participant_request["request_id"] = json!("new-participant-creation");
    let participant_creation = work::mcp_call_with_input(
        &f.state,
        &other_actor,
        "work_create",
        &participant_request,
        Some(&input),
    )
    .unwrap();
    assert_ne!(participant_creation["id"], first["id"]);
    assert_eq!(participant_creation["source_input"], input);
    assert_eq!(
        participant_creation["creation_context"]["actor_id"],
        "another-person"
    );
    let mut changed_by_participant = request.clone();
    changed_by_participant["goal"] = json!("Changed intent is not a retry");
    assert_eq!(
        work::mcp_call_with_input(
            &f.state,
            &other_actor,
            "work_create",
            &changed_by_participant,
            Some(&input)
        )
        .unwrap_err()
        .0,
        409
    );

    let unlinked = json!({"title":"Unlinked work","goal":"Per-caller operation","request_id":"unlinked-creation"});
    let original_actor_work = work::mcp_call(&f.state, &binding, "work_create", &unlinked).unwrap();
    let other_actor_work =
        work::mcp_call(&f.state, &other_actor, "work_create", &unlinked).unwrap();
    assert_ne!(original_actor_work["id"], other_actor_work["id"]);

    let private_binding = json!({"owner":true,"actor_id":"owner-one"});
    let private_input = json!({"principal":"owner","sessionId":"private-session","inputId":"input-one",
        "sources":input["sources"],"binding":private_binding});
    let another_owner = json!({"owner":true,"actor_id":"owner-two"});
    assert!(
        work::mcp_call_with_input(
            &f.state,
            &another_owner,
            "work_create",
            &request,
            Some(&private_input)
        )
        .is_err()
    );
    assert!(
        work::mcp_call_with_input(
            &f.state,
            &private_binding,
            "work_create",
            &request,
            Some(&input)
        )
        .is_err()
    );

    let mut several = input.clone();
    several["inputId"] = json!("input-several");
    several["sources"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"message-two","revision":"revision-one","occurredAt":now_ms()}));
    let several_request = json!({"title":"Combined input","goal":"Preserve every original source","source_input_id":"input-several","request_id":"combined-part"});
    let combined = work::mcp_call_with_input(
        &f.state,
        &binding,
        "work_create",
        &several_request,
        Some(&several),
    )
    .unwrap();
    assert!(combined["source"]["message_id"].is_null());
    assert_eq!(combined["source_input"]["sources"], several["sources"]);
}

fn review_command(item: &Value, outcome: &str) -> Value {
    json!({"type":"review_artifacts", "brief_sha256":item["completion_contract"]["brief"]["sha256"],
        "artifact_set_sha256":item["completion_contract"]["artifact_set"]["sha256"], "outcome":outcome,
        "criterion_indices":[0,1], "evidence_ids":[item["evidence"][1]["id"]],
        "note":"Inspected these exact delivered bytes and checked both acceptance criteria."})
}

fn close_command(item: &Value) -> Value {
    json!({"type":"close", "brief_sha256":item["completion_contract"]["brief"]["sha256"],
        "artifact_set_sha256":item["completion_contract"]["artifact_set"]["sha256"],
        "note":"The requested result is accepted for this frozen scope and artifact version."})
}

async fn command_as(f: &Fixture, item: &Value, token: &str, action: Value, expected: u16) -> Value {
    let (status, result) = f
        .req(
            "POST",
            &format!("work/{}/commands", item["id"].as_str().unwrap()),
            json!({"expected_revision":item["revision"],"command":action}),
            Some(token),
        )
        .await;
    assert_eq!(status, expected, "{result}");
    result
}

#[tokio::test]
async fn frozen_brief_exact_artifacts_group_review_and_requester_closure_stay_bound() {
    let f = Fixture::new();
    let first = auth::issue_token(&f.state, "Peer reviewer one", &["work".into()], 3600).unwrap();
    let second = auth::issue_token(&f.state, "Peer reviewer two", &["work".into()], 3600).unwrap();
    let first_actor = format!("token:{}", first["id"].as_str().unwrap());
    let second_actor = format!("token:{}", second["id"].as_str().unwrap());
    let first_token = first["token"].as_str().unwrap();
    let second_token = second["token"].as_str().unwrap();
    let mut w = f.api("POST","work",json!({"title":"Deliver a checked product","goal":"Deliver the agreed file",
        "source":{"provider_id":"workshop","conversation_id":"design","message_id":"request-1","url":"https://example.test/design#request-1"}})).await;
    w=f.command(&w,json!({"type":"freeze_brief","acceptance_criteria":["Contains the agreed message","Ends with a newline"],
        "reviewer_actors":[first_actor,second_actor]})).await;
    assert_eq!(
        w["completion_contract"]["schema"],
        "agent-instance.completion-contract/1"
    );
    let first_brief = w["completion_contract"]["brief"].clone();
    w = f.command(&w, json!({"type":"accept"})).await;
    w = f.command(&w, json!({"type":"start"})).await;
    let artifact = f.dir.path().join("workspace/delivered.txt");
    std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
    std::fs::write(&artifact, "Agreed message\n").unwrap();
    let bytes = std::fs::read_to_string(&artifact).unwrap();
    assert!(bytes.ends_with('\n'));
    w=f.command(&w,json!({"type":"artifact_version","artifact":"delivery","label":"Delivered file",
        "uri":reqwest::Url::from_file_path(&artifact).unwrap().to_string(),"version":"1","sha256":auth::digest(&bytes)})).await;
    w=f.command(&w,json!({"type":"evidence","label":"Exact file checks","uri":"urn:fixture:content-and-newline:1","kind":"test"})).await;
    w = f.command(&w, json!({"type":"review"})).await;
    command_as(&f, &w, &f.token, review_command(&w, "pass"), 403).await;
    let mut incomplete = review_command(&w, "pass");
    incomplete["criterion_indices"] = json!([0]);
    command_as(&f, &w, first_token, incomplete, 400).await;
    let mut artifact_is_not_a_check = review_command(&w, "pass");
    artifact_is_not_a_check["evidence_ids"] = json!([w["evidence"][0]["id"]]);
    command_as(&f, &w, first_token, artifact_is_not_a_check, 400).await;
    command_as(&f,&w,&f.token,json!({"type":"verify","outcome":"pass","evidence_ids":[w["evidence"][1]["id"]],"note":"Bypass strict review"}),400).await;
    w = command_as(&f, &w, first_token, review_command(&w, "pass"), 200).await;
    command_as(&f, &w, &f.token, close_command(&w), 400).await;
    w = command_as(&f, &w, second_token, review_command(&w, "pass"), 200).await;
    assert_eq!(w["verification"]["outcome"], "pass");
    assert_eq!(w["verification"]["independently_verified"], false);
    let stale_close = close_command(&w);
    std::fs::write(&artifact, "Agreed replacement\n").unwrap();
    w=f.command(&w,json!({"type":"artifact_version","artifact":"delivery","label":"Replacement",
        "uri":reqwest::Url::from_file_path(&artifact).unwrap().to_string(),"version":"2","sha256":auth::digest("Agreed replacement\n")})).await;
    assert_eq!(
        w["completion_contract"]["artifact_set"]["artifacts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        w["completion_contract"]["reviews"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(w["verification"].is_null());
    command_as(&f, &w, &f.token, stale_close, 409).await;
    command_as(&f, &w, &f.token, close_command(&w), 400).await;
    w = command_as(&f, &w, first_token, review_command(&w, "pass"), 200).await;
    w = command_as(&f, &w, second_token, review_command(&w, "fail"), 200).await;
    command_as(&f, &w, &f.token, close_command(&w), 400).await;
    w = command_as(&f, &w, second_token, review_command(&w, "pass"), 200).await;
    command_as(&f, &w, &f.token, json!({"type":"complete"}), 400).await;
    command_as(&f, &w, first_token, close_command(&w), 403).await;
    w = f.command(&w, close_command(&w)).await;
    assert_eq!(w["state"], "completed");
    assert_eq!(
        w["completion_contract"]["closure"]["artifact_set_sha256"],
        w["completion_contract"]["artifact_set"]["sha256"]
    );
    assert_eq!(
        w["completion_contract"]["closure"]["human_identity_inferred"],
        false
    );
    let reopened = AppState::open(f.dir.path().to_owned()).unwrap();
    assert_eq!(
        work::get_work(&reopened, w["id"].as_str().unwrap(), None).unwrap(),
        w
    );
    w = f
        .command(&w, json!({"type":"reopen","reason":"New requirement"}))
        .await;
    assert!(w["completion_contract"]["closure"].is_null());
    w = f
        .command(
            &w,
            json!({"type":"amend","title":"Changed scope","goal":"Now deliver the new agreement"}),
        )
        .await;
    command_as(&f, &w, &f.token, json!({"type":"start"}), 400).await;
    let refreeze = json!({"type":"freeze_brief","acceptance_criteria":["New agreement","Checked version"],"reviewer_actors":[first_actor,second_actor]});
    command_as(&f, &w, first_token, refreeze.clone(), 403).await;
    w = f.command(&w, refreeze).await;
    assert_eq!(w["completion_contract"]["brief_versions"][0], first_brief);
    assert_eq!(
        w["completion_contract"]["brief_versions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    w = f.command(&w, json!({"type":"start"})).await;
    w = f.command(&w, json!({"type":"review"})).await;
    command_as(&f, &w, &f.token, close_command(&w), 400).await;
}

#[tokio::test]
async fn completion_policy_rejects_missing_source_self_review_and_conversation_closure() {
    let f = Fixture::new();
    f.org("peer", false).await;
    let mut missing = f.create().await;
    let reviewer = auth::issue_token(&f.state, "Reviewer", &["work".into()], 3600).unwrap();
    let actor = format!("token:{}", reviewer["id"].as_str().unwrap());
    let token = reviewer["token"].as_str().unwrap();
    let mut freeze = json!({"type":"freeze_brief","acceptance_criteria":["One","Two"],"reviewer_actors":[actor]});
    command_as(&f, &missing, &f.token, freeze.clone(), 400).await;
    missing = f
        .api(
            "POST",
            "work",
            json!({"title":"Checked output","goal":"Check output",
        "source":{"provider_id":"web","conversation_id":"room"}}),
        )
        .await;
    freeze["source"] = json!({"provider_id":"web","conversation_id":"room","message_id":"one"});
    let binding =
        json!({"owner":true,"provider_id":"web","conversation_id":"room","actor_id":"requester"});
    let command = serde_json::from_value::<work::Command>(
        json!({"expected_revision":missing["revision"],"command":freeze}),
    )
    .unwrap();
    assert!(
        work::command_work(
            &f.state,
            missing["id"].as_str().unwrap(),
            command,
            "conversation-tool",
            Some(&binding)
        )
        .is_err()
    );
    let mut w = f.command(&missing, freeze.clone()).await;
    w = f.command(&w, json!({"type":"accept"})).await;
    command_as(&f,&w,&f.token,json!({"type":"artifact_version","artifact":"app","label":"App","uri":"urn:fixture:app","version":"1","sha256":"invalid"}),400).await;
    w=command_as(&f,&w,token,json!({"type":"artifact_version","artifact":"app","label":"App","uri":"urn:fixture:app","version":"1","sha256":auth::digest("artifact")}),200).await;
    w = f
        .command(
            &w,
            json!({"type":"evidence","label":"Checks","uri":"urn:fixture:tests","kind":"test"}),
        )
        .await;
    w = f.command(&w, json!({"type":"review"})).await;
    command_as(&f, &w, token, review_command(&w, "pass"), 400).await;
    let command = serde_json::from_value::<work::Command>(
        json!({"expected_revision":w["revision"],"command":close_command(&w)}),
    )
    .unwrap();
    assert!(
        work::command_work(
            &f.state,
            w["id"].as_str().unwrap(),
            command,
            "conversation-tool",
            Some(&binding)
        )
        .is_err()
    );
    let new_requester =
        auth::issue_token(&f.state, "Replacement requester", &["work".into()], 3600).unwrap();
    let new_actor = format!("token:{}", new_requester["id"].as_str().unwrap());
    let transfer = json!({"type":"transfer_requester","requester_actor":new_actor,"note":"Explicit recovery after rotating the requester's credential"});
    command_as(&f, &w, token, transfer.clone(), 403).await;
    command_as(&f,&w,&f.token,json!({"type":"transfer_requester","requester_actor":"token:missing","note":"Unknown identity"}),400).await;
    let previous_brief = w["completion_contract"]["brief"].clone();
    w = f.command(&w, transfer).await;
    assert_eq!(w["completion_contract"]["requester_actor"], new_actor);
    assert_eq!(w["completion_contract"]["brief"], previous_brief);
    assert_eq!(
        w["completion_contract"]["requester_transfers"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    command_as(&f, &w, &f.token, freeze.clone(), 403).await;
    w = command_as(
        &f,
        &w,
        new_requester["token"].as_str().unwrap(),
        freeze,
        200,
    )
    .await;
    assert_eq!(
        w["completion_contract"]["brief"]["requester_actor"],
        new_actor
    );
}

#[tokio::test]
async fn work_survives_restart_and_requires_review_evidence_and_revisions() {
    let f = Fixture::new();
    f.org("peer", false).await;
    let mut w = f.create().await;
    let path = format!("work/{}/commands", w["id"].as_str().unwrap());
    assert_eq!(
        f.req(
            "POST",
            &path,
            json!({"expected_revision":1,"command":{"type":"complete"}}),
            Some(&f.token)
        )
        .await
        .0,
        400
    );
    w = f.command(&w, json!({"type":"accept"})).await;
    assert_eq!(w["state"], "accepted");
    assert_eq!(
        f.req(
            "POST",
            &path,
            json!({"expected_revision":1,"command":{"type":"start"}}),
            Some(&f.token)
        )
        .await
        .0,
        409
    );
    w = f.command(&w, json!({"type":"start"})).await;
    w = f
        .command(&w, json!({"type":"block","reason":"Missing fixture"}))
        .await;
    assert_eq!(w["state"], "blocked");
    w = f.command(&w, json!({"type":"resume"})).await;
    w=f.command(&w,json!({"type":"evidence","label":"Executed checks","uri":"file:///tmp/check-result.json","kind":"test"})).await;
    w = f.command(&w, json!({"type":"review"})).await;
    assert_eq!(
        f.req(
            "POST",
            &path,
            json!({"expected_revision":w["revision"],"command":{"type":"complete"}}),
            Some(&f.token)
        )
        .await
        .0,
        400
    );
    w=f.command(&w,json!({"type":"verify","outcome":"pass","evidence_ids":[w["evidence"][0]["id"]],"note":"Checked the referenced output"})).await;
    assert_eq!(w["verification"]["independently_verified"], false);
    w = f
        .command(
            &w,
            json!({"type":"amend","title":"Changed requirement","goal":"Now need a second result"}),
        )
        .await;
    assert!(w["verification"].is_null());
    assert_eq!(w["state"], "in_progress");
    w = f.command(&w, json!({"type":"review"})).await;
    w=f.command(&w,json!({"type":"verify","outcome":"pass","evidence_ids":[w["evidence"][0]["id"]],"note":"Rechecked revised requirement"})).await;
    w = f.command(&w, json!({"type":"complete"})).await;
    let reopened = AppState::open(f.dir.path().to_owned()).unwrap();
    let stored = work::get_work(&reopened, w["id"].as_str().unwrap(), None).unwrap();
    assert_eq!(stored, w);
    assert_eq!(stored["state"], "completed");
    let events = f.state.list_records("work").unwrap();
    assert_eq!(events.len(), 1);
    let raw = f.api("GET", "events", Value::Null).await.to_string();
    assert!(!raw.contains("Rechecked revised requirement"));
    assert!(!raw.contains("file:///tmp"));
}

#[tokio::test]
async fn scopes_and_conversation_audiences_are_enforced_even_for_tool_mutations() {
    let f = Fixture::new();
    f.org("peer", false).await;
    let owner = f.create().await;
    let read = auth::issue_token(&f.state, "read", &["work:read".into()], 3600).unwrap();
    let read = read["token"].as_str().unwrap();
    let write = auth::issue_token(&f.state, "writer", &["work".into()], 3600).unwrap();
    let write = write["token"].as_str().unwrap();
    let observe = auth::issue_token(&f.state, "observer", &["observe".into()], 3600).unwrap();
    let observe = observe["token"].as_str().unwrap();
    assert_eq!(f.req("GET", "work", Value::Null, None).await.0, 401);
    assert_eq!(
        f.req("GET", "work", Value::Null, Some(observe)).await.0,
        403
    );
    assert_eq!(f.req("GET", "work", Value::Null, Some(read)).await.0, 200);
    assert_eq!(
        f.req("POST", "work", json!({"title":"x","goal":"y"}), Some(read))
            .await
            .0,
        403
    );
    assert_eq!(
        f.req(
            "PUT",
            "organization",
            json!({"expected_revision":1,"organization":{}}),
            Some(write)
        )
        .await
        .0,
        403
    );
    let room =
        json!({"owner":false,"provider_id":"slack","conversation_id":"C-room","actor_id":"U-a"});
    let other = json!({"owner":false,"provider_id":"slack","conversation_id":"C-other"});
    assert_eq!(
        work::mcp_call(&f.state, &room, "work_list", &json!({})).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        work::get_work(&f.state, owner["id"].as_str().unwrap(), Some(&room))
            .unwrap_err()
            .0,
        404
    );
    assert!(
        work::mcp_call(
            &f.state,
            &room,
            "work_create",
            &json!({"title":"Bad","goal":"attempt to widen","audience":{"kind":"owner"}})
        )
        .is_err()
    );
    let w = work::mcp_call(
        &f.state,
        &room,
        "work_create",
        &json!({"title":"Room work","goal":"Shared only here"}),
    )
    .unwrap();
    assert_eq!(w["audience"]["conversation_id"], "C-room");
    assert_eq!(w["source"]["conversation_id"], "C-room");
    assert_eq!(w["creation_context"]["actor_id"], "U-a");
    assert_eq!(
        work::mcp_call(&f.state, &room, "work_list", &json!({})).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        work::mcp_call(
            &f.state,
            &other,
            "work_command",
            &json!({"id":w["id"],"expected_revision":1,"command":{"type":"accept"}})
        )
        .is_err()
    );
    let accepted = work::mcp_call(
        &f.state,
        &room,
        "work_command",
        &json!({"id":w["id"],"expected_revision":1,"command":{"type":"accept"}}),
    )
    .unwrap();
    assert_eq!(accepted["state"], "accepted");
    assert!(
        accepted["created_by"]
            .as_str()
            .unwrap()
            .starts_with("agent:")
    );
}

#[tokio::test]
async fn explicit_delegation_projects_fields_deduplicates_and_never_grants_transitive_access() {
    let sender = Fixture::new();
    let receiver = Fixture::new();
    let s_id = sender.state.instance_config().instance_id;
    let r_id = receiver.state.instance_config().instance_id;
    sender.org(&r_id, false).await;
    receiver.org(&s_id, false).await;
    let mut w = sender.create().await;
    let path = format!("work/{}/export", w["id"].as_str().unwrap());
    assert_eq!(sender.req("POST",&path,json!({"expected_revision":1,"relationship_id":"coordinator","disclosure":"details","summary":"Availability only"}),Some(&sender.token)).await.0,403);
    let packet=sender.api("POST",&path,json!({"expected_revision":1,"relationship_id":"coordinator","disclosure":"summary","summary":"Available for a small check"})).await;
    assert!(packet["details"].is_null());
    assert!(!packet.to_string().contains("A real check of the output"));
    assert_eq!(packet["source_agent_id"], s_id);
    assert_eq!(packet["source_work_id"], w["id"]);
    assert_eq!(packet["sharing"]["forwarding"], false);
    let request = json!({"relationship_id":"coordinator","assignment_id":"work","packet":packet});
    let received = receiver.api("POST", "work/import", request.clone()).await;
    let imported = &received["work"];
    assert_eq!(imported["state"], "proposed");
    assert_eq!(imported["upstream"]["trust"], "unverified_remote_report");
    assert!(imported["verification"].is_null());
    assert_eq!(
        receiver.api("POST", "work/import", request.clone()).await["duplicate"],
        true
    );
    let mut tampered = request.clone();
    tampered["packet"]["summary"] = json!("Different content under the same envelope ID");
    assert_eq!(
        receiver
            .req("POST", "work/import", tampered, Some(&receiver.token))
            .await
            .0,
        400
    );
    let mut wrong = request.clone();
    wrong["assignment_id"] = json!("personal");
    assert_eq!(
        receiver
            .req("POST", "work/import", wrong, Some(&receiver.token))
            .await
            .0,
        403
    );
    let path = format!("work/{}/export", imported["id"].as_str().unwrap());
    assert_eq!(receiver.req("POST",&path,json!({"expected_revision":1,"relationship_id":"coordinator","disclosure":"summary","summary":"Forward this"}),Some(&receiver.token)).await.0,400);
    // The sender remains owner, with no automatic handoff, delivery or completion claim.
    w = sender
        .api(
            "GET",
            &format!("work/{}", w["id"].as_str().unwrap()),
            Value::Null,
        )
        .await;
    assert_eq!(w["state"], "proposed");
    assert_eq!(w["delegations"][0]["delivery"], "not_confirmed");
    let restart = AppState::open(receiver.dir.path().to_owned()).unwrap();
    assert_eq!(
        work::list_work(&restart, None).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn assignment_snapshots_survive_template_edits_and_audiences_cannot_widen() {
    let f = Fixture::new();
    let mut org = f.org("peer", false).await;
    let w = f.create().await;
    org["organization"]["templates"][0]["instructions"] = json!("New discipline instructions");
    f.api(
        "PUT",
        "organization",
        json!({"expected_revision":1,"organization":org["organization"]}),
    )
    .await;
    let stored = f
        .api(
            "GET",
            &format!("work/{}", w["id"].as_str().unwrap()),
            Value::Null,
        )
        .await;
    assert_eq!(
        stored["template"]["instructions"],
        "Check the actual changed artifact before reporting."
    );
    assert_eq!(
        f.create().await["template"]["instructions"],
        "New discipline instructions"
    );
    org["organization"]["assignments"][0]["audience"] =
        json!({"kind":"conversation","provider_id":"slack","conversation_id":"C-other"});
    assert_eq!(
        f.req(
            "PUT",
            "organization",
            json!({"expected_revision":2,"organization":org["organization"]}),
            Some(&f.token)
        )
        .await
        .0,
        400
    );
}

#[test]
fn profile_use_selects_latest_started_turn_not_latest_finished_or_reloaded_session() {
    let f = Fixture::new();
    std::fs::write(
        f.dir.path().join("profiles/communication.md"),
        "Use concrete examples.",
    )
    .unwrap();
    let unknown = profile_use::inspect(&f.state, &Default::default()).unwrap();
    assert_eq!(unknown["status"], "unknown");
    let (text, package) = profiles::context(&f.state).unwrap();
    let hash = auth::digest(&text);
    let at = now_ms();
    profile_use::started(
        &f.state,
        "older",
        "s-old",
        "conn",
        at - 1000,
        1,
        (&hash, &package),
    )
    .unwrap();
    profile_use::started(&f.state, "newer", "s-new", "conn", at, 2, (&hash, &package)).unwrap();
    profile_use::finished(&f.state, "older", "finished_last").unwrap();
    let status = profile_use::inspect(&f.state, &Default::default()).unwrap();
    assert_eq!(status["relevant_turn"]["id"], "newer");
    assert_eq!(status["status"], "supplied_current");
    assert_eq!(status["behavior_quality"], "not_evaluated");
    let selected = profile_use::inspect(
        &f.state,
        &profile_use::Selection {
            session_id: Some("s-old".into()),
            max_age_seconds: None,
        },
    )
    .unwrap();
    assert_eq!(selected["relevant_turn"]["id"], "older");
    std::fs::write(
        f.dir.path().join("profiles/identity.md"),
        "Changed identity guidance",
    )
    .unwrap();
    assert_eq!(
        profile_use::inspect(&f.state, &Default::default()).unwrap()["status"],
        "supplied_previous"
    );
    profile_use::started(
        &f.state,
        "stale",
        "stale-s",
        "conn",
        at - 100_000,
        3,
        (&hash, &package),
    )
    .unwrap();
    assert_eq!(
        profile_use::inspect(
            &f.state,
            &profile_use::Selection {
                session_id: None,
                max_age_seconds: Some(60)
            }
        )
        .unwrap()["status"],
        "stale"
    );
    f.state
        .set_record(
            "profile_turns",
            "missing",
            json!({"id":"missing","started_sequence":4,"started_at":at}),
        )
        .unwrap();
    assert_eq!(
        profile_use::inspect(&f.state, &Default::default()).unwrap()["status"],
        "partial"
    );
}

#[tokio::test]
async fn profile_use_distinguishes_empty_guidance_from_local_profiles_and_retains_history() {
    let f = Fixture::new();
    let (code, initial) = f
        .req("GET", "profile-use", Value::Null, Some(&f.token))
        .await;
    assert_eq!(code, 200);
    assert_eq!(initial["status"], "not_configured");
    assert_eq!(initial["installed"]["configured"], false);
    assert!(initial["installed"]["package"]["id"].is_null());
    assert!(initial["relevant_turn"].is_null());
    let (empty, package) = profiles::context(&f.state).unwrap();
    profile_use::started(
        &f.state,
        "empty-turn",
        "session",
        "connection",
        now_ms(),
        1,
        (&auth::digest(&empty), &package),
    )
    .unwrap();
    let (_, empty_turn) = f
        .req("GET", "profile-use", Value::Null, Some(&f.token))
        .await;
    assert_eq!(empty_turn["status"], "not_configured");
    assert!(empty_turn["content_matches"].is_null());
    assert!(empty_turn["version_matches"].is_null());
    assert_eq!(empty_turn["relevant_turn"]["id"], "empty-turn");

    // Legacy local guidance has no imported package; it still counts as configured.
    std::fs::write(
        f.dir.path().join("profiles/communication.md"),
        "Speak concisely.",
    )
    .unwrap();
    let (text, package) = profiles::context(&f.state).unwrap();
    assert!(package["id"].is_null());
    let hash = auth::digest(&text);
    profile_use::started(
        &f.state,
        "local-turn",
        "session",
        "connection",
        now_ms(),
        2,
        (&hash, &package),
    )
    .unwrap();
    let current = profile_use::inspect(&f.state, &Default::default()).unwrap();
    assert_eq!(current["installed"]["configured"], true);
    assert_eq!(current["status"], "supplied_current");
    assert_eq!(current["content_matches"], true);

    // An empty saved version must not imply a persona, nor erase earlier evidence.
    profiles::write(&f.state, "communication", "").unwrap();
    profiles::write(&f.state, "working-style", " \n\t ").unwrap();
    profiles::write(&f.state, "identity", " \n ").unwrap();
    let cleared = profile_use::inspect(&f.state, &Default::default()).unwrap();
    assert!(!cleared["installed"]["package"]["id"].is_null());
    assert_eq!(cleared["status"], "not_configured");
    assert_eq!(cleared["installed"]["profile_sha256"], auth::digest(""));
    assert_eq!(cleared["relevant_turn"]["id"], "local-turn");
    assert_eq!(cleared["relevant_turn"]["profile_sha256"], hash);
    assert!(cleared["content_matches"].is_null());
    assert!(cleared["version_matches"].is_null());

    let identity_only = Fixture::new();
    profiles::write(&identity_only.state, "identity", "Local identity guidance.").unwrap();
    let local = profile_use::inspect(&identity_only.state, &Default::default()).unwrap();
    assert!(local["installed"]["package"]["id"].is_null());
    assert_eq!(local["installed"]["configured"], true);
    assert_eq!(local["status"], "unknown");
}
