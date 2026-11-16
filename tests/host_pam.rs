#![cfg(unix)]
use agent_instance::{auth, config::HostPamConfig, host, server, state::AppState};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request},
};
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixListener,
};
use tower::ServiceExt;

struct Fixture {
    dir: tempfile::TempDir,
    state: Arc<AppState>,
    router: Router,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        host::init(
            dir.path(),
            "Host login test".into(),
            None,
            "127.0.0.1".into(),
            0,
        )
        .unwrap();
        let state = Arc::new(AppState::open(dir.path().to_owned()).unwrap());
        Self {
            router: server::router(state.clone()),
            dir,
            state,
        }
    }
    fn bridge(&self, name: &str) -> (UnixListener, HostPamConfig) {
        let path = self.dir.path().join(name);
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        (
            listener,
            HostPamConfig {
                owner: "host-owner".into(),
                socket: path.to_str().unwrap().into(),
            },
        )
    }
    fn configure(&self, bridge: Option<HostPamConfig>) {
        let mut config = self.state.instance_config();
        config.server.host_pam = bridge;
        let revision = self.state.config.read().unwrap().revision;
        server::replace_config(&self.state, revision, config, "test-owner").unwrap();
    }
    async fn login(&self, owner: &str, password: &str) -> (u16, HeaderMap, Value) {
        login(self.router.clone(), owner, password).await
    }
    fn cookie_headers(&self, headers: &HeaderMap) -> HeaderMap {
        let mut result = HeaderMap::new();
        result.insert(
            "cookie",
            headers["set-cookie"]
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .parse()
                .unwrap(),
        );
        result
    }
}
async fn login(router: Router, owner: &str, password: &str) -> (u16, HeaderMap, Value) {
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("host", "localhost")
                .header("origin", "https://localhost")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"username":owner,"password":password}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    (status, headers, body)
}
async fn serve(listener: UnixListener, response: &'static [u8]) -> Value {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    let request: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(request.as_object().unwrap().len(), 2);
    socket.get_mut().write_all(response).await.unwrap();
    request
}

#[tokio::test]
async fn external_login_preserves_resource_owner_csrf_and_private_protocol() {
    let f = Fixture::new();
    let resource_owner = f.state.instance_config().owner;
    let (listener, bridge) = f.bridge("owner.sock");
    f.configure(Some(bridge));
    let worker = tokio::spawn(serve(listener, b"OK\n"));
    let (status, headers, body) = f.login("host-owner", "test\n密码").await;
    assert_eq!(status, 200);
    let request = worker.await.unwrap();
    assert_eq!(
        request,
        json!({"username":"host-owner","password":"test\n密码"})
    );
    assert_eq!(body["owner"], "host-owner");
    assert_eq!(f.state.instance_config().owner, resource_owner);
    let cookie_for_session = f.cookie_headers(&headers);
    let axum::Json(session) =
        auth::session(axum::extract::State(f.state.clone()), cookie_for_session)
            .await
            .unwrap();
    assert_eq!(session["owner"], "host-owner");
    let control = std::fs::read_to_string(f.dir.path().join("secrets/control-token")).unwrap();
    let mut bearer = HeaderMap::new();
    bearer.insert(
        "authorization",
        format!("Bearer {control}").parse().unwrap(),
    );
    let axum::Json(discovery) = auth::session(axum::extract::State(f.state.clone()), bearer)
        .await
        .unwrap();
    assert_eq!(discovery["owner"], resource_owner);

    let mut cookie = f.cookie_headers(&headers);
    assert_eq!(
        auth::check(&f.state, &cookie, "admin", false)
            .unwrap()
            .actor,
        "host-owner"
    );
    assert!(auth::check(&f.state, &cookie, "admin", true).is_err());
    cookie.insert(
        "x-csrf-token",
        body["csrf"].as_str().unwrap().parse().unwrap(),
    );
    cookie.insert("host", "localhost".parse().unwrap());
    cookie.insert("origin", "https://elsewhere".parse().unwrap());
    assert!(auth::check(&f.state, &cookie, "admin", true).is_err());
    cookie.insert("origin", "https://localhost".parse().unwrap());
    assert!(auth::check(&f.state, &cookie, "admin", true).is_ok());
    let events = f
        .state
        .db
        .lock()
        .unwrap()
        .prepare("SELECT data FROM events")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("\n");
    assert!(!events.contains("密码"));
    assert!(!events.contains("test\\n"));
}

#[tokio::test]
async fn strict_responses_unavailable_socket_and_wrong_owner_fail_closed() {
    for (i, response) in [
        b"NO\n".as_slice(),
        b"OK\nextra",
        b"OK",
        b"OK\r\n",
        b"{\"ok\":true}\n",
    ]
    .into_iter()
    .enumerate()
    {
        let f = Fixture::new();
        let (listener, bridge) = f.bridge(&format!("{i}.sock"));
        f.configure(Some(bridge));
        let task = tokio::spawn(serve(listener, response));
        assert_eq!(f.login("host-owner", "test").await.0, 401);
        task.await.unwrap();
    }
    let f = Fixture::new();
    let (listener, bridge) = f.bridge("wrong.sock");
    f.configure(Some(bridge.clone()));
    assert_eq!(f.login("another-owner", "test").await.0, 401);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
    drop(listener);
    std::fs::remove_file(&bridge.socket).unwrap();
    assert_eq!(f.login("host-owner", "test").await.0, 401);
}

#[tokio::test]
async fn broad_file_and_symlink_sockets_are_refused_without_connecting_or_repair() {
    let f = Fixture::new();
    let (listener, mut bridge) = f.bridge("private.sock");
    std::fs::set_permissions(&bridge.socket, std::fs::Permissions::from_mode(0o660)).unwrap();
    f.configure(Some(bridge.clone()));
    assert_eq!(f.login("host-owner", "test").await.0, 401);
    assert_eq!(
        std::fs::metadata(&bridge.socket)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o660
    );
    std::fs::set_permissions(&bridge.socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = f.dir.path().join("alias.sock");
    std::os::unix::fs::symlink(&bridge.socket, &link).unwrap();
    bridge.socket = link.to_str().unwrap().into();
    f.configure(Some(bridge));
    assert_eq!(f.login("host-owner", "test").await.0, 401);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn changed_authentication_epoch_rejects_inflight_success_even_after_change_back() {
    let f = Fixture::new();
    let (listener, bridge) = f.bridge("blocked.sock");
    f.configure(Some(bridge.clone()));
    let router = f.router.clone();
    let pending = tokio::spawn(async move { login(router, "host-owner", "test").await });
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    f.configure(None);
    f.configure(Some(bridge));
    socket.get_mut().write_all(b"OK\n").await.unwrap();
    drop(socket);
    assert_eq!(pending.await.unwrap().0, 401);
    assert_eq!(
        f.state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM browser_sessions", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn cookies_survive_unrelated_changes_but_not_auth_changes_or_offline_restart_changes() {
    let f = Fixture::new();
    let (listener, bridge) = f.bridge("session.sock");
    f.configure(Some(bridge.clone()));
    let task = tokio::spawn(serve(listener, b"OK\n"));
    let (_, headers, _) = f.login("host-owner", "test").await;
    task.await.unwrap();
    let cookie = f.cookie_headers(&headers);
    let mut config = f.state.instance_config();
    config.name = "Renamed".into();
    let revision = f.state.config.read().unwrap().revision;
    server::replace_config(&f.state, revision, config, "test-owner").unwrap();
    assert!(auth::check(&f.state, &cookie, "admin", false).is_ok());
    let reopened = AppState::open(f.dir.path().to_owned()).unwrap();
    assert!(auth::check(&reopened, &cookie, "admin", false).is_ok());
    drop(reopened);
    let mut offline = f.state.config.read().unwrap().clone();
    offline.config.server.host_pam.as_mut().unwrap().owner = "new-owner".into();
    std::fs::write(
        f.dir.path().join("config.json"),
        serde_json::to_vec(&offline).unwrap(),
    )
    .unwrap();
    let reopened = AppState::open(f.dir.path().to_owned()).unwrap();
    assert!(auth::check(&reopened, &cookie, "admin", false).is_err());
    assert!(auth::check(&f.state, &cookie, "admin", false).is_err());
}

#[tokio::test]
async fn managed_auth_change_revokes_cookie_and_existing_cooldown_still_applies() {
    let f = Fixture::new();
    let (listener, bridge) = f.bridge("managed.sock");
    f.configure(Some(bridge));
    let task = tokio::spawn(serve(listener, b"OK\n"));
    let (_, headers, _) = f.login("host-owner", "test").await;
    task.await.unwrap();
    let cookie = f.cookie_headers(&headers);
    f.configure(None);
    assert!(auth::check(&f.state, &cookie, "admin", false).is_err());
    f.state
        .db
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO login_attempts(key,failures,locked_until) VALUES('owner',5,?1)",
            [agent_instance::state::now_ms() + 60_000],
        )
        .unwrap();
    assert_eq!(f.login("host-owner", "test").await.0, 429);
}

#[test]
fn bridge_configuration_is_optional_fixed_and_strict() {
    let f = Fixture::new();
    let config = f.state.instance_config();
    assert!(config.server.host_pam.is_none());
    let serialized = serde_json::to_value(&config).unwrap();
    assert!(serialized["server"].get("host_pam").is_none());
    for value in [
        json!({"owner":"x","socket":"relative"}),
        json!({"owner":" x","socket":"/tmp/s"}),
        json!({"owner":"x\n","socket":"/tmp/s"}),
        json!({"owner":"x","socket":"/tmp/s","service":"other"}),
    ] {
        let mut document = serialized.clone();
        document["server"]["host_pam"] = value;
        assert!(
            match serde_json::from_value::<agent_instance::config::InstanceConfig>(document) {
                Err(_) => true,
                Ok(c) => c.validate().is_err(),
            }
        );
    }
}

#[tokio::test]
async fn two_pending_bridge_attempts_keep_the_existing_concurrency_limit() {
    let f = Fixture::new();
    let (listener, bridge) = f.bridge("concurrent.sock");
    f.configure(Some(bridge));
    let router = f.router.clone();
    let first = tokio::spawn(async move { login(router, "host-owner", "one").await });
    let (a, _) = listener.accept().await.unwrap();
    let mut a = BufReader::new(a);
    let mut first_line = String::new();
    a.read_line(&mut first_line).await.unwrap();
    let router = f.router.clone();
    let second = tokio::spawn(async move { login(router, "host-owner", "two").await });
    let (b, _) = listener.accept().await.unwrap();
    let mut b = BufReader::new(b);
    let mut second_line = String::new();
    b.read_line(&mut second_line).await.unwrap();
    assert_eq!(f.login("host-owner", "three").await.0, 429);
    a.get_mut().write_all(b"OK\n").await.unwrap();
    b.get_mut().write_all(b"OK\n").await.unwrap();
    drop(a);
    drop(b);
    assert_eq!(first.await.unwrap().0, 200);
    assert_eq!(second.await.unwrap().0, 200);
    assert_eq!(f.state.login_gate.available_permits(), 2);
}

#[test]
fn legacy_local_cookie_survives_initial_binding_migration() {
    let f = Fixture::new();
    let owner = f.state.instance_config().owner;
    let cookie = "synthetic-existing-cookie";
    {
        let db = f.state.db.lock().unwrap();
        db.execute(
            "DELETE FROM records WHERE collection='authentication' AND id='binding'",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO browser_sessions(hash,owner,csrf,expires_at) VALUES(?1,?2,'csrf',?3)",
            rusqlite::params![
                auth::digest(cookie),
                owner,
                agent_instance::state::now_ms() + 60000
            ],
        )
        .unwrap();
    }
    let reopened = AppState::open(f.dir.path().to_owned()).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        "cookie",
        format!(
            "ai_{}={cookie}",
            reopened.instance_config().instance_id.replace('-', "")
        )
        .parse()
        .unwrap(),
    );
    assert_eq!(
        auth::check(&reopened, &headers, "admin", false)
            .unwrap()
            .actor,
        owner
    );
}

#[tokio::test]
async fn requester_transfer_uses_signin_principal_without_changing_existing_work_or_os_owner() {
    use agent_instance::work;
    for external in [false, true] {
        let f = Fixture::new();
        let os_owner = f.state.instance_config().owner;
        if external {
            let (_, bridge) = f.bridge("transfer.sock");
            f.configure(Some(bridge));
        }
        let principal = if external { "host-owner" } else { &os_owner };
        let token = std::fs::read_to_string(f.dir.path().join("secrets/control-token")).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        let record = work::create_work(
            &f.state,
            serde_json::from_value(json!({"title":"Test work","goal":"Test scope","source":{"provider_id":"fixture","conversation_id":"room","message_id":"one"}})).unwrap(),
            "original-requester",
            None,
        )
        .unwrap();
        let id = record["id"].as_str().unwrap().to_owned();
        let frozen=work::command_work(&f.state,&id,serde_json::from_value(json!({"expected_revision":record["revision"],"command":{"type":"freeze_brief","acceptance_criteria":["Scoped result"],"reviewer_actors":["reviewer"]}})).unwrap(),"original-requester",None).unwrap();
        if external {
            let old_owner_command = serde_json::from_value(json!({"expected_revision":frozen["revision"],"command":{"type":"transfer_requester","requester_actor":os_owner,"note":"Not the current login principal"}})).unwrap();
            let result = work::command(
                axum::extract::State(f.state.clone()),
                headers.clone(),
                axum::extract::Path(id.clone()),
                axum::Json(old_owner_command),
            )
            .await;
            assert_eq!(result.unwrap_err().0.as_u16(), 400);
        }
        let command=serde_json::from_value(json!({"expected_revision":frozen["revision"],"command":{"type":"transfer_requester","requester_actor":principal,"note":"Explicit transfer"}})).unwrap();
        let axum::Json(changed) = work::command(
            axum::extract::State(f.state.clone()),
            headers,
            axum::extract::Path(id),
            axum::Json(command),
        )
        .await
        .unwrap();
        assert_eq!(changed["completion_contract"]["requester_actor"], principal);
        assert_eq!(
            changed["completion_contract"]["brief"]["requester_actor"],
            "original-requester"
        );
        assert_eq!(f.state.instance_config().owner, os_owner);
    }
}
