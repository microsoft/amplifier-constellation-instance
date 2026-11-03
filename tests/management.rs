use agent_instance::{
    auth, host, server,
    state::{AppState, now_ms},
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use rusqlite::params;
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
            "Management fixture".into(),
            None,
            "127.0.0.1".into(),
            0,
        )
        .unwrap();
        let state = Arc::new(AppState::open(dir.path().to_owned()).unwrap());
        let token = std::fs::read_to_string(dir.path().join("secrets/control-token")).unwrap();
        let router = server::router(state.clone());
        Self {
            _dir: dir,
            state,
            router,
            token,
        }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        token: Option<&str>,
    ) -> (u16, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("/api/v1/{path}"))
            .header("host", "localhost");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let response = self
            .router
            .clone()
            .oneshot(
                request
                    .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status().as_u16();
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({"raw":String::from_utf8_lossy(&bytes)}));
        (status, value)
    }
    async fn admin(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.request(method, path, body, Some(&self.token)).await
    }
}

#[tokio::test]
async fn credentials_enforce_resource_boundaries_and_revocation() {
    let f = Fixture::new();
    for path in [
        "status",
        "config",
        "secrets",
        "tokens",
        "profiles/identity",
        "connections",
        "sessions",
        "events",
        "activity",
        "backend",
        "backend/account",
        "backend/account/login",
        "views",
    ] {
        assert_eq!(f.request("GET", path, None, None).await.0, 401, "{path}");
    }
    let (_, grant) = f
        .admin(
            "POST",
            "tokens",
            Some(json!({"name":"Observer","scopes":["observe"],"expires_in_seconds":600})),
        )
        .await;
    let token = grant["token"].as_str().unwrap();
    assert_eq!(f.request("GET", "status", None, Some(token)).await.0, 200);
    assert_eq!(f.request("GET", "backend", None, Some(token)).await.0, 200);
    for path in ["backend/install", "backend/account/login"] {
        assert_eq!(
            f.request("POST", path, Some(json!({})), Some(token))
                .await
                .0,
            403
        );
        assert_eq!(f.request("POST", path, Some(json!({})), None).await.0, 401);
    }
    for path in ["backend/account", "backend/account/login"] {
        assert_eq!(f.request("GET", path, None, Some(token)).await.0, 403);
    }
    for path in ["config", "secrets", "tokens", "profiles/identity", "views"] {
        assert_eq!(
            f.request("GET", path, None, Some(token)).await.0,
            403,
            "{path}"
        );
    }
    assert_eq!(
        f.request(
            "POST",
            "lifecycle",
            Some(json!({"action":"start"})),
            Some(token)
        )
        .await
        .0,
        403
    );
    let (_, metadata) = f.admin("GET", "tokens", None).await;
    assert!(!metadata.to_string().contains(token));
    assert_eq!(
        f.admin(
            "DELETE",
            &format!("tokens/{}", grant["id"].as_str().unwrap()),
            None
        )
        .await
        .0,
        200
    );
    assert_eq!(f.request("GET", "status", None, Some(token)).await.0, 401);
}

#[tokio::test]
async fn setup_is_revisioned_and_identity_cannot_be_reassigned() {
    let f = Fixture::new();
    let (_, doc) = f.admin("GET", "config", None).await;
    let mut config = doc["config"].clone();
    config["server"]["public_url"] = json!("https://user:private@agent.example");
    assert_eq!(
        f.admin("POST", "config/validate", Some(config.clone()))
            .await
            .0,
        400
    );
    config["server"]["public_url"] = Value::Null;
    config["name"] = json!("Changed");
    assert_eq!(
        f.admin(
            "PUT",
            "config",
            Some(json!({"expected_revision":1,"config":config}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        f.admin(
            "PUT",
            "config",
            Some(json!({"expected_revision":1,"config":config}))
        )
        .await
        .0,
        409
    );
    config["owner"] = json!("another-user");
    assert_eq!(
        f.admin(
            "PUT",
            "config",
            Some(json!({"expected_revision":2,"config":config}))
        )
        .await
        .0,
        400
    );
    config["owner"] = doc["config"]["owner"].clone();
    config["enabled"] = json!(true);
    assert_eq!(
        f.admin(
            "PUT",
            "config",
            Some(json!({"expected_revision":2,"config":config}))
        )
        .await
        .0,
        400
    );
    assert_eq!(f.state.instance_config().name, "Changed");
    assert!(!f.state.enabled());
}

#[tokio::test]
async fn views_support_remote_control_conflicts_and_render_acknowledgements() {
    let f = Fixture::new();
    let (_, grant) = f
        .admin(
            "POST",
            "tokens",
            Some(json!({"name":"Voice UI","scopes":["ui"],"expires_in_seconds":600})),
        )
        .await;
    let token = grant["token"].as_str().unwrap();
    let (status, view) = f
        .request(
            "POST",
            "views",
            Some(json!({"label":"A real display"})),
            Some(token),
        )
        .await;
    assert_eq!(status, 200);
    let path = format!("views/{}", view["id"].as_str().unwrap());
    let (status, changed) = f
        .request(
            "POST",
            &format!("{path}/commands"),
            Some(json!({"expected_revision":1,"command":{"type":"navigate","route":"profiles"}})),
            Some(token),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(changed["state"]["route"], "profiles");
    assert!(changed["ack"].is_null());
    assert_eq!(
        f.request(
            "POST",
            &format!("{path}/ack"),
            Some(json!({"revision":1,"status":"applied"})),
            Some(token)
        )
        .await
        .0,
        409
    );
    let (_, ack) = f
        .request(
            "POST",
            &format!("{path}/ack"),
            Some(json!({"revision":2,"status":"applied"})),
            Some(token),
        )
        .await;
    assert_eq!(ack["ack"]["revision"], 2);
    assert_eq!(
        f.request(
            "POST",
            &format!("{path}/commands"),
            Some(json!({"expected_revision":1,"command":{"type":"navigate","route":"dashboard"}})),
            Some(token)
        )
        .await
        .0,
        409
    );
    assert_eq!(f.request("POST",&format!("{path}/commands"),Some(json!({"expected_revision":2,"command":{"type":"set_draft","path":"access.password","value":"never-store-this"}})),Some(token)).await.0,400);
    assert_eq!(
        f.request("GET", "profiles/communication", None, Some(token))
            .await
            .0,
        403
    );
    let (_, events) = f.request("GET", "events", None, Some(token)).await;
    assert!(
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["kind"].as_str().unwrap().starts_with("view."))
    );
}

#[tokio::test]
async fn secret_and_profile_content_stay_out_of_observation_streams() {
    let f = Fixture::new();
    let secret = "private-provider-test-secret";
    let profile = "Private identity details for the test";
    assert_eq!(
        f.admin("PUT", "secrets/provider", Some(json!({"value":secret})))
            .await
            .0,
        200
    );
    assert_eq!(
        f.admin("PUT", "profiles/identity", Some(json!({"text":profile})))
            .await
            .0,
        200
    );
    assert_eq!(
        f.admin(
            "PUT",
            "secrets/control-token",
            Some(json!({"value":"invalid-overwrite"}))
        )
        .await
        .0,
        400
    );
    let (_, secrets) = f.admin("GET", "secrets", None).await;
    assert!(!secrets.to_string().contains(secret));
    assert!(!secrets.to_string().contains("control-token"));
    let (_, events) = f.admin("GET", "events", None).await;
    assert!(!events.to_string().contains(secret));
    assert!(!events.to_string().contains(profile));
    assert!(!events.to_string().contains(&f.token));
}

#[tokio::test]
async fn cookie_mutations_require_csrf_and_same_origin() {
    let f = Fixture::new();
    let token = "test-browser-session";
    let csrf = "test-csrf";
    let owner = f.state.instance_config().owner;
    f.state
        .db
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO browser_sessions(hash,owner,csrf,expires_at) VALUES(?1,?2,?3,?4)",
            params![auth::digest(token), owner, csrf, now_ms() + 60000],
        )
        .unwrap();
    let cookie = format!(
        "ai_{}={token}",
        f.state.instance_config().instance_id.replace('-', "")
    );
    for (csrf_header, origin, expected) in [
        (None, None, 403),
        (Some(csrf), Some("https://untrusted.example"), 403),
        (Some(csrf), Some("https://localhost"), 200),
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v1/views")
            .header("host", "localhost")
            .header("cookie", &cookie)
            .header("content-type", "application/json");
        if let Some(csrf) = csrf_header {
            request = request.header("x-csrf-token", csrf);
        }
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let response = f
            .router
            .clone()
            .oneshot(request.body(Body::from("{\"label\":\"browser\"}")).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected);
    }
    let h2_request = Request::builder()
        .method("POST")
        .uri("https://localhost/api/v1/views")
        .version(axum::http::Version::HTTP_2)
        .header("cookie", "asa_other_connector=another-app-session")
        .header("cookie", &cookie)
        .header("content-type", "application/json")
        .header("origin", "https://localhost")
        .header("x-csrf-token", csrf)
        .body(Body::from("{\"label\":\"HTTP/2 browser\"}"))
        .unwrap();
    let response = f.router.clone().oneshot(h2_request).await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
}

fn persona_package(label: &str) -> Value {
    let communication = format!("Speak with {label} cadence.");
    let working = format!("Decide using {label} priorities.");
    json!({"schema":"twin-profile/1","manifest":{
        "schema":"twin-profile/1","instruction_format":2,
        "subject_key":"00000000-0000-4000-8000-000000000001",
        "revision":uuid::Uuid::new_v4().to_string(),
        "raw_evidence_included":false,"identity_included":false,
        "required_context":["communication.md","working-style.md"],
        "files":{"communication.md":auth::digest(&communication),"working-style.md":auth::digest(&working)}
    },"files":{"communication.md":communication,"working-style.md":working}})
}

#[tokio::test]
async fn persona_package_apply_edit_restore_and_restart_preserve_individual_identity() {
    let f = Fixture::new();
    f.admin(
        "PUT",
        "profiles/identity",
        Some(json!({"text":"Private identity remains separate"})),
    )
    .await;
    assert_eq!(
        f.admin("GET", "profile-package", None).await.1["revision"],
        0
    );
    let package = persona_package("first");
    let (code, first) = f
        .admin(
            "POST",
            "profile-package",
            Some(json!({"expected_revision":0,"package":package})),
        )
        .await;
    assert_eq!(code, 200, "{first}");
    assert_eq!(
        first["active"]["source"]["revision"],
        package["manifest"]["revision"]
    );
    assert_eq!(first["previous"][0]["kind"], "baseline");
    let context = f.state.profile_text().unwrap();
    assert!(context.contains("first cadence") && context.contains("first priorities"));
    assert!(context.contains("Private identity remains separate"));
    assert!(!context.contains("twin-profile/1"));
    assert_eq!(
        f.admin("GET", "profiles/communication", None).await.1["text"],
        package["files"]["communication.md"]
    );
    let (stale, _) = f
        .admin(
            "POST",
            "profile-package",
            Some(json!({"expected_revision":0,"package":persona_package("stale")})),
        )
        .await;
    assert_eq!(stale, 409);
    assert!(!f.state.profile_text().unwrap().contains("stale"));
    let (code, _) = f
        .admin(
            "PUT",
            "profiles/communication",
            Some(json!({"text":"Edited by owner"})),
        )
        .await;
    assert_eq!(code, 200);
    let current = f.admin("GET", "profile-package", None).await.1;
    assert_eq!(current["revision"], 2);
    assert_eq!(current["active"]["kind"], "manual");
    assert!(current["active"]["source"].is_null());
    assert!(f.state.profile_text().unwrap().contains("Edited by owner"));
    let (code, restored) = f
        .admin(
            "POST",
            "profile-package/restore",
            Some(json!({"expected_revision":2,"id":first["active"]["id"]})),
        )
        .await;
    assert_eq!(code, 200, "{restored}");
    assert_eq!(restored["revision"], 3);
    assert_eq!(restored["active"]["id"], first["active"]["id"]);
    let reopened = AppState::open(f.state.home.clone()).unwrap();
    assert_eq!(reopened.profile_text().unwrap(), context);
    assert_eq!(
        agent_instance::profiles::status(&reopened).unwrap()["revision"],
        3
    );
    let (code, _) = f
        .admin(
            "POST",
            "profile-package/restore",
            Some(json!({"expected_revision":3,"id":first["previous"][0]["id"]})),
        )
        .await;
    assert_eq!(code, 200);
    assert_eq!(
        f.admin("GET", "profiles/communication", None).await.1["text"],
        ""
    );
    assert!(
        f.state
            .profile_text()
            .unwrap()
            .contains("Private identity remains separate")
    );
    let events = f.admin("GET", "events", None).await.1.to_string();
    assert!(!events.contains("first cadence"));
    assert!(!events.contains("Edited by owner"));
}

#[tokio::test]
async fn persona_packages_reject_corruption_private_identity_and_partial_replacement() {
    let f = Fixture::new();
    f.admin(
        "PUT",
        "profiles/communication",
        Some(json!({"text":"Original voice"})),
    )
    .await;
    let before = f.admin("GET", "profile-package", None).await.1;
    let mut mutations = vec![];
    let good = persona_package("new");
    let mut p = good.clone();
    p["files"]["communication.md"] = json!("Tampered text");
    mutations.push(p);
    let mut p = good.clone();
    p["manifest"]["identity_included"] = json!(true);
    mutations.push(p);
    let mut p = good.clone();
    p["manifest"]["raw_evidence_included"] = json!(true);
    mutations.push(p);
    let mut p = good.clone();
    p["files"]["../identity.md"] = json!("Overwrite identity");
    mutations.push(p);
    let mut p = good.clone();
    p["files"]
        .as_object_mut()
        .unwrap()
        .remove("working-style.md");
    mutations.push(p);
    let mut p = good.clone();
    p["files"]["manifest.json"] = json!("{}");
    mutations.push(p);
    let mut p = good.clone();
    p["manifest"]["instruction_format"] = json!(3);
    mutations.push(p);
    let mut p = good.clone();
    p["manifest"]["instruction_format"] = json!("future");
    mutations.push(p);
    for package in mutations {
        let (code, result) = f
            .admin(
                "POST",
                "profile-package",
                Some(json!({"expected_revision":1,"package":package})),
            )
            .await;
        assert_eq!(code, 400, "{result}");
        assert_eq!(f.admin("GET", "profile-package", None).await.1, before);
        assert!(f.state.profile_text().unwrap().contains("Original voice"));
    }
    let result = f
        .admin(
            "POST",
            "profile-package/restore",
            Some(json!({"expected_revision":1,"id":"../../config.json"})),
        )
        .await;
    assert_eq!(result.0, 400);
}

#[tokio::test]
async fn persona_grant_cannot_read_identity_credentials_or_change_execution() {
    let f = Fixture::new();
    let (_, grant) = f.admin("POST", "tokens", Some(json!({"name":"Persona Workshop","scopes":["observe","profiles"],"expires_in_seconds":600}))).await;
    let token = grant["token"].as_str().unwrap();
    assert_eq!(f.request("GET", "status", None, Some(token)).await.0, 200);
    assert_eq!(
        f.request("GET", "profile-package", None, Some(token))
            .await
            .0,
        200
    );
    for path in [
        "profiles/identity",
        "profiles/communication",
        "config",
        "secrets",
        "tokens",
    ] {
        assert_eq!(
            f.request("GET", path, None, Some(token)).await.0,
            403,
            "{path}"
        );
    }
    assert_eq!(
        f.request(
            "POST",
            "lifecycle",
            Some(json!({"action":"stop"})),
            Some(token)
        )
        .await
        .0,
        403
    );
    assert_eq!(
        f.request(
            "POST",
            "profile-package",
            Some(json!({"expected_revision":0,"package":persona_package("scoped")})),
            Some(token)
        )
        .await
        .0,
        200
    );
    let (_, observer) = f
        .admin(
            "POST",
            "tokens",
            Some(json!({"name":"Observe only","scopes":["observe"],"expires_in_seconds":600})),
        )
        .await;
    assert_eq!(
        f.request("GET", "profile-package", None, observer["token"].as_str())
            .await
            .0,
        403
    );
    f.admin(
        "DELETE",
        &format!("tokens/{}", grant["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(
        f.request("GET", "profile-package", None, Some(token))
            .await
            .0,
        401
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_prompt_readers_always_receive_one_complete_profile_pair() {
    let f = Fixture::new();
    f.admin(
        "POST",
        "profile-package",
        Some(json!({"expected_revision":0,"package":persona_package("alpha")})),
    )
    .await;
    let state = f.state.clone();
    let reader = std::thread::spawn(move || {
        for _ in 0..250 {
            let text = state.profile_text().unwrap();
            assert_eq!(
                text.contains("alpha cadence"),
                text.contains("alpha priorities")
            );
            assert_eq!(
                text.contains("beta cadence"),
                text.contains("beta priorities")
            );
            assert!(text.contains("alpha cadence") || text.contains("beta cadence"));
        }
    });
    for revision in 1..12 {
        let label = if revision % 2 == 1 { "beta" } else { "alpha" };
        let (code, _) = f
            .admin(
                "POST",
                "profile-package",
                Some(json!({"expected_revision":revision,"package":persona_package(label)})),
            )
            .await;
        assert_eq!(code, 200);
    }
    reader.join().unwrap();
}

#[tokio::test]
async fn agent_portrait_is_authorized_revisioned_and_survives_restart() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let f = Fixture::new();
    assert_eq!(f.request("GET", "identity", None, None).await.0, 401);
    let (status, before) = f.request("GET", "identity", None, Some(&f.token)).await;
    assert_eq!(status, 200);
    assert_eq!(before["avatar"]["kind"], "generated");
    assert!(before["avatar"].get("data").is_none());
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(640, 480, image::Rgba([120, 100, 80, 255]))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let body = json!({"expected_revision":0,"media_type":"image/png","data":STANDARD.encode(bytes.into_inner())});
    assert_eq!(
        f.request("PUT", "identity/avatar", Some(body.clone()), None)
            .await
            .0,
        401
    );
    let (status, after) = f
        .request("PUT", "identity/avatar", Some(body.clone()), Some(&f.token))
        .await;
    assert_eq!(status, 200);
    assert_eq!(after["avatar"]["kind"], "uploaded");
    assert_eq!(after["avatar_revision"], 1);
    assert_ne!(before["revision"], after["revision"]);
    assert_eq!(
        f.request("PUT", "identity/avatar", Some(body), Some(&f.token))
            .await
            .0,
        409
    );
    let restarted = AppState::open(f.state.home.clone()).unwrap();
    assert_eq!(
        agent_instance::identity::document(&restarted, false).unwrap(),
        after
    );
    let (status, removed) = f
        .request(
            "DELETE",
            "identity/avatar?expected_revision=1",
            None,
            Some(&f.token),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(removed["avatar"]["kind"], "generated");
    assert_eq!(removed["avatar_revision"], 2);
    assert_eq!(removed["avatar"]["sha256"], before["avatar"]["sha256"]);
    assert_eq!(
        f.request(
            "DELETE",
            "identity/avatar?expected_revision=1",
            None,
            Some(&f.token)
        )
        .await
        .0,
        409
    );
    assert!(
        !f.request("GET", "events", None, Some(&f.token))
            .await
            .1
            .to_string()
            .contains("iVBOR")
    );
}

#[tokio::test]
async fn connected_tools_config_schema_and_explicit_tokens_use_existing_management_surface() {
    let f = Fixture::new();
    let (_, mut document) = f.admin("GET", "config", None).await;
    document["config"]["tool_connections"] = json!([{
        "name":"project-tools", "url":"https://tools.example/mcp",
        "header_secrets":{"Authorization":"project-auth"}
    }]);
    let (status, _) = f
        .admin(
            "PUT",
            "secrets/project-auth",
            Some(json!({"value":"Bearer private-tool-value"})),
        )
        .await;
    assert_eq!(status, 200);
    let (status, saved) = f
        .admin(
            "PUT",
            "config",
            Some(json!({"expected_revision":document["revision"],"config":document["config"]})),
        )
        .await;
    assert_eq!(status, 200);
    assert!(!saved.to_string().contains("private-tool-value"));
    assert_eq!(
        saved["config"]["tool_connections"][0]["header_secrets"]["Authorization"],
        "project-auth"
    );
    let (status, _) = f.admin("DELETE", "secrets/project-auth", None).await;
    assert_eq!(
        status, 400,
        "Referenced tool credential cannot be deleted accidentally"
    );
    let (_, schema) = f.admin("GET", "schema", None).await;
    assert!(schema["properties"]["tool_connections"].is_object());
    let (_, spec) = f.admin("GET", "openapi.json", None).await;
    assert!(spec.to_string().contains("tools:use"));
    let (_,token)=f.admin("POST","tokens",Some(json!({"name":"Tool-enabled experience","scopes":["acp","tools:use"],"expires_in_seconds":3600}))).await;
    assert!(token["token"].is_string());
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", f.token).parse().unwrap(),
    );
    assert!(!auth::explicit_scope(&f.state, &headers, "tools:use"));
    headers.insert(
        "authorization",
        format!("Bearer {}", token["token"].as_str().unwrap())
            .parse()
            .unwrap(),
    );
    assert!(auth::explicit_scope(&f.state, &headers, "tools:use"));
    for path in ["config", "status", "events", "sessions"] {
        let (_, data) = f.admin("GET", path, None).await;
        assert!(
            !data.to_string().contains("private-tool-value"),
            "{path} leaked a credential"
        );
    }
}
