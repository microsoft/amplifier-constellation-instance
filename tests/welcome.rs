use agent_instance::{host, server, state::AppState, welcome};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

fn fixture() -> (tempfile::TempDir, Arc<AppState>) {
    let dir = tempfile::tempdir().unwrap();
    host::init(
        dir.path(),
        "TLS fixture".into(),
        None,
        "127.0.0.1".into(),
        0,
    )
    .unwrap();
    let state = Arc::new(AppState::open(dir.path().to_owned()).unwrap());
    *state.addresses.write().unwrap() = json!({"urls":["https://127.0.0.1:44333"],"welcome_urls":["http://127.0.0.1:44334"],"effective_server":{"port":44333}});
    (dir, state)
}

#[tokio::test]
async fn public_help_exposes_only_public_certificates_and_never_authentication() {
    let (dir, state) = fixture();
    let http = welcome::router(state.clone());
    for uri in [
        "/",
        "/welcome",
        "/welcome/app.js",
        "/welcome/style.css",
        "/welcome/info",
    ] {
        let response = http
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{uri}");
        let body = String::from_utf8(
            to_bytes(response.into_body(), 100_000)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!body.contains("PRIVATE KEY"));
        assert!(!body.contains(&dir.path().to_string_lossy().to_string()));
        assert!(
            !body.contains(
                &std::fs::read_to_string(dir.path().join("secrets/control-token")).unwrap()
            )
        );
    }
    for (method, uri) in [
        ("POST", "/api/v1/auth/login"),
        ("GET", "/api/v1/status"),
        ("GET", "/api/v1/config"),
        ("GET", "/tls/root-ca.key"),
        ("GET", "/welcome/key.pem"),
        ("GET", "/welcome/../../tls/key.pem"),
    ] {
        let response = http
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 404, "{uri}");
    }
    let cert = http
        .oneshot(
            Request::builder()
                .uri("/welcome/root.crt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        cert.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment; filename=")
    );
    assert_eq!(
        String::from_utf8(to_bytes(cert.into_body(), 100_000).await.unwrap().to_vec())
            .unwrap()
            .replace("\r\n", "\n"),
        std::fs::read_to_string(dir.path().join("tls/root-ca.crt"))
            .unwrap()
            .replace("\r\n", "\n")
    );
    let https = server::router(state.clone());
    let response = https
        .oneshot(
            Request::builder()
                .uri("/api/v1/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn browser_probe_is_public_and_only_allows_our_welcome_origins() {
    let (_dir, state) = fixture();
    for (origin, allowed) in [
        ("http://127.0.0.1:44334", true),
        ("https://unrelated.example", false),
    ] {
        let r = server::router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/welcome/check")
                    .header("origin", origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(
            r.headers().contains_key("access-control-allow-origin"),
            allowed
        );
        assert!(!r.headers().contains_key("access-control-allow-credentials"));
        let data: Value =
            serde_json::from_slice(&to_bytes(r.into_body(), 10000).await.unwrap()).unwrap();
        assert_eq!(data.as_object().unwrap().len(), 2);
    }
}

#[test]
fn certificates_validate_and_keep_the_same_authority_when_addresses_change() {
    let (dir, state) = fixture();
    let root = std::fs::read(dir.path().join("tls/root-ca.crt")).unwrap();
    let leaf = std::fs::read(dir.path().join("tls/cert.pem")).unwrap();
    let verify = || {
        let result = std::process::Command::new("openssl")
            .args(["verify", "-x509_strict", "-CAfile"])
            .arg(dir.path().join("tls/root-ca.crt"))
            .args(["-purpose", "sslserver", "-verify_ip", "127.0.0.1"])
            .arg(dir.path().join("tls/cert.pem"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    verify();
    welcome::prepare(
        dir.path(),
        &state.instance_config().instance_id,
        vec!["127.0.0.1".into(), "new-address.test".into()],
    )
    .unwrap();
    assert_eq!(
        root,
        std::fs::read(dir.path().join("tls/root-ca.crt")).unwrap()
    );
    assert_ne!(
        leaf,
        std::fs::read(dir.path().join("tls/cert.pem")).unwrap()
    );
    verify();
    let leaf = std::fs::read(dir.path().join("tls/cert.pem")).unwrap();
    welcome::prepare(
        dir.path(),
        &state.instance_config().instance_id,
        vec!["new-address.test".into(), "127.0.0.1".into()],
    )
    .unwrap();
    assert_eq!(
        leaf,
        std::fs::read(dir.path().join("tls/cert.pem")).unwrap()
    );
}

#[test]
fn renewal_uses_persisted_authority_and_repairs_invalid_profile_two_leaf() {
    use rcgen::{CertificateParams, DnType, Issuer, KeyPair};
    for old_profile in [1, 2] {
        let (dir, _state) = fixture();
        let tls = dir.path().join("tls");
        let root = std::fs::read(tls.join("root-ca.crt")).unwrap();
        let root_key = std::fs::read(tls.join("root-ca.key")).unwrap();
        let new_id = uuid::Uuid::new_v4().to_string();
        let verify = || {
            std::process::Command::new("openssl")
                .args(["verify", "-x509_strict", "-CAfile"])
                .arg(tls.join("root-ca.crt"))
                .args(["-purpose", "sslserver", "-verify_ip", "127.0.0.1"])
                .arg(tls.join("cert.pem"))
                .output()
                .unwrap()
        };
        assert!(verify().status.success());
        if old_profile == 2 {
            // Reproduce 0.7.1: same CA key, but a reconstructed issuer subject
            // based on the new instance ID instead of the persisted CA subject.
            let mut guessed_ca = CertificateParams::default();
            guessed_ca.distinguished_name.push(
                DnType::CommonName,
                format!("Agent Instance {} Local CA", &new_id[..8]),
            );
            let issuer = Issuer::new(
                guessed_ca,
                KeyPair::from_pem(std::str::from_utf8(&root_key).unwrap()).unwrap(),
            );
            let mut leaf = CertificateParams::new(vec!["127.0.0.1".into()]).unwrap();
            leaf.use_authority_key_identifier_extension = true;
            let key = KeyPair::generate().unwrap();
            let cert = leaf.signed_by(&key, &issuer).unwrap();
            std::fs::write(tls.join("cert.pem"), cert.pem()).unwrap();
            std::fs::write(tls.join("key.pem"), key.serialize_pem()).unwrap();
            assert!(!verify().status.success());
        }
        let mut marker: Value =
            serde_json::from_slice(&std::fs::read(tls.join("managed.json")).unwrap()).unwrap();
        marker["leaf_profile"] = json!(old_profile);
        marker["sans"] = json!(["127.0.0.1"]);
        std::fs::write(
            tls.join("managed.json"),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();

        welcome::prepare(dir.path(), &new_id, vec!["127.0.0.1".into()]).unwrap();
        let verified = verify();
        assert!(
            verified.status.success(),
            "{}",
            String::from_utf8_lossy(&verified.stderr)
        );
        assert_eq!(root, std::fs::read(tls.join("root-ca.crt")).unwrap());
        assert_eq!(root_key, std::fs::read(tls.join("root-ca.key")).unwrap());
        let marker: Value =
            serde_json::from_slice(&std::fs::read(tls.join("managed.json")).unwrap()).unwrap();
        assert_eq!(marker["leaf_profile"], 3);
        let renewed = std::fs::read(tls.join("cert.pem")).unwrap();
        welcome::prepare(dir.path(), &new_id, vec!["127.0.0.1".into()]).unwrap();
        assert_eq!(renewed, std::fs::read(tls.join("cert.pem")).unwrap());
    }
}

#[tokio::test]
async fn certificate_help_uses_actual_retained_root_name_after_identity_changes() {
    let (_dir, state) = fixture();
    let original_id = state.instance_config().instance_id;
    let new_id = uuid::Uuid::new_v4().to_string();
    state.config.write().unwrap().config.instance_id = new_id.clone();
    let response = welcome::router(state)
        .oneshot(
            Request::builder()
                .uri("/welcome/info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let data: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
    assert_eq!(data["instance_id"], new_id);
    assert_eq!(
        data["certificate_name"],
        format!("Agent Instance {} Local CA", &original_id[..8])
    );
}

#[tokio::test]
async fn custom_certificate_setup_does_not_offer_an_unrelated_root() {
    let (_dir, state) = fixture();
    state.config.write().unwrap().config.server.certificate_file =
        Some("/operator/cert.pem".into());
    let router = welcome::router(state);
    let info = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/welcome/info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let data: Value =
        serde_json::from_slice(&to_bytes(info.into_body(), 10000).await.unwrap()).unwrap();
    assert_eq!(data["download_available"], false);
    let download = router
        .oneshot(
            Request::builder()
                .uri("/welcome/root.crt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(download.status(), 404);
}
