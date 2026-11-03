//! Public connection help. This router deliberately has no management or sign-in endpoints.
use crate::{
    config::{expand, write_private},
    error::{ApiError, ApiResult},
    state::AppState,
};
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{io::BufReader, path::Path, sync::Arc};
use time::{Duration, OffsetDateTime};

fn ca_params(instance_id: &str, created: OffsetDateTime) -> CertificateParams {
    let mut p = CertificateParams::default();
    p.distinguished_name.push(
        DnType::CommonName,
        format!("Agent Instance {} Local CA", &instance_id[..8]),
    );
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    p.not_before = created - Duration::days(1);
    p.not_after = created + Duration::days(3650);
    p
}

/// Managed installations use their own CA so browsers can import a real trust anchor.
/// The CA stays stable while server certificates renew or interface addresses change.
/// Explicit operator-supplied certificates are never replaced.
pub fn prepare(home: &Path, instance_id: &str, mut sans: Vec<String>) -> Result<()> {
    sans.sort();
    sans.dedup();
    let marker = home.join("tls/managed.json");
    let now = OffsetDateTime::now_utc();
    let previous: Option<Value> = if marker.exists() {
        Some(serde_json::from_slice(&std::fs::read(&marker)?)?)
    } else {
        None
    };
    let created = previous
        .as_ref()
        .and_then(|v| v["created_at"].as_i64())
        .map(OffsetDateTime::from_unix_timestamp)
        .transpose()?
        .unwrap_or(now);
    let ca_key = if previous.is_some() {
        ensure!(
            home.join("tls/root-ca.crt").exists(),
            "Managed certificate authority is missing; restore its backup"
        );
        KeyPair::from_pem(&std::fs::read_to_string(home.join("tls/root-ca.key"))?)?
    } else {
        // Preserve the pre-0.3 generated leaf/key for rollback; never expose these backups.
        for name in ["cert.pem", "key.pem"] {
            let old = home.join("tls").join(name);
            if old.exists() {
                let backup = home.join("tls").join(format!("pre-ca-{name}"));
                ensure!(
                    !backup.exists(),
                    "Certificate migration backup already exists; inspect before retrying"
                );
                write_private(&backup, &std::fs::read(old)?)?;
            }
        }
        let key = KeyPair::generate()?;
        let ca = ca_params(instance_id, created).self_signed(&key)?;
        write_private(&home.join("tls/root-ca.crt"), ca.pem().as_bytes())?;
        write_private(
            &home.join("tls/root-ca.key"),
            key.serialize_pem().as_bytes(),
        )?;
        key
    };
    if let Some(old) = &previous
        && old["leaf_profile"] == 3
        && old["sans"] == json!(sans)
        && old["leaf_expires_at"].as_i64().unwrap_or(0)
            > (now + Duration::days(30)).unix_timestamp()
        && home.join("tls/cert.pem").exists()
        && home.join("tls/key.pem").exists()
    {
        return Ok(());
    }
    ensure!(
        now + Duration::days(365) < created + Duration::days(3650),
        "Local CA is nearing expiry; replace it and reinstall trust before renewing"
    );
    // The persisted CA may predate the current instance identity (for example,
    // an imported installation). Preserve its actual subject and key identifier.
    let issuer = Issuer::from_ca_cert_pem(
        &std::fs::read_to_string(home.join("tls/root-ca.crt"))?,
        ca_key,
    )
    .context("Read the existing managed certificate authority")?;
    let key = KeyPair::generate()?;
    let mut leaf = CertificateParams::new(sans.clone())?;
    leaf.distinguished_name.push(
        DnType::CommonName,
        format!("Agent Instance {}", &instance_id[..8]),
    );
    leaf.not_before = now - Duration::days(1);
    leaf.not_after = now + Duration::days(365);
    leaf.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    leaf.use_authority_key_identifier_extension = true;
    let cert = leaf.signed_by(&key, &issuer)?;
    // Only the leaf is served. Clients trust the separate downloadable root.
    write_private(&home.join("tls/cert.pem"), cert.pem().as_bytes())?;
    write_private(&home.join("tls/key.pem"), key.serialize_pem().as_bytes())?;
    write_private(
        &marker,
        &serde_json::to_vec_pretty(&json!({
            "created_at":created.unix_timestamp(),"leaf_expires_at":leaf.not_after.unix_timestamp(),"sans":sans,"leaf_profile":3
        }))?,
    )?;
    Ok(())
}

pub fn fingerprint(path: &Path) -> Result<String> {
    let mut pem = BufReader::new(std::fs::File::open(path)?);
    let der = rustls_pemfile::certs(&mut pem)
        .next()
        .context("Certificate file is empty")??;
    Ok(format!("{:x}", Sha256::digest(der.as_ref())))
}

pub fn router(state: Arc<AppState>) -> Router {
    routes().route("/", get(page)).with_state(state)
}
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/welcome", get(page))
        .route("/welcome/", get(|| async { Redirect::to("../welcome") }))
        .route("/welcome/style.css", get(style))
        .route("/welcome/app.js", get(script))
        .route("/welcome/info", get(info))
        .route("/welcome/check", get(check))
        .route("/welcome/root.crt", get(certificate))
}
fn public_response(content_type: &'static str, body: impl IntoResponse) -> Response {
    let mut r = body.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
    h.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
    h.insert("referrer-policy", "no-referrer".parse().unwrap());
    h.insert("content-security-policy", "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self' https:; object-src 'none'; base-uri 'self'; form-action 'none'; frame-ancestors 'self'".parse().unwrap());
    r
}
async fn page(State(state): State<Arc<AppState>>) -> Response {
    public_response(
        "text/html; charset=utf-8",
        crate::server::named_page(
            include_str!("../ui/welcome.html"),
            &state.config.read().unwrap().config.name,
            "Agent Instance setup",
        ),
    )
}
async fn style() -> Response {
    public_response("text/css; charset=utf-8", include_str!("../ui/welcome.css"))
}
async fn script() -> Response {
    public_response(
        "application/javascript; charset=utf-8",
        include_str!("../ui/welcome.js"),
    )
}

fn certificate_common_name(path: &Path) -> Option<String> {
    let pem = std::fs::read(path).ok()?;
    let certificate = rustls_pemfile::certs(&mut pem.as_slice()).next()?.ok()?;
    let (_, parsed) = x509_parser::parse_x509_certificate(certificate.as_ref()).ok()?;
    parsed
        .subject()
        .iter_common_name()
        .next()?
        .as_str()
        .ok()
        .map(str::to_owned)
}

pub async fn info(State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.instance_config();
    let addresses = state.addresses.read().unwrap().clone();
    let managed =
        config.server.certificate_file.is_none() && state.home.join("tls/root-ca.crt").exists();
    let certificate_name = if managed {
        certificate_common_name(&state.home.join("tls/root-ca.crt"))
            .unwrap_or_else(|| "Agent Instance local certificate authority".into())
    } else {
        "Configured server certificate".into()
    };
    Json(
        json!({"instance_id":config.instance_id,"https_urls":addresses["urls"],
        "preferred_url":config.server.public_url,"https_port":addresses["effective_server"]["port"],
        "download_available":managed,"certificate_name":certificate_name,
        "certificate_sha256":if managed {fingerprint(&state.home.join("tls/root-ca.crt")).ok()} else {None},
        "download":"welcome/root.crt","certificate_help":"welcome"}),
    )
}
async fn certificate(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    if state.instance_config().server.certificate_file.is_some() {
        return Err(ApiError::missing());
    }
    let path = state.home.join("tls/root-ca.crt");
    if !path.is_file() {
        return Err(ApiError::missing());
    }
    // Parse and re-encode only public certificates, even if a file was accidentally bundled with a key.
    let pem = std::fs::read_to_string(path)?;
    let start = pem
        .find("-----BEGIN CERTIFICATE-----")
        .ok_or(ApiError::missing())?;
    let end = pem[start..]
        .find("-----END CERTIFICATE-----")
        .ok_or(ApiError::missing())?
        + start
        + "-----END CERTIFICATE-----".len();
    let mut response = public_response(
        "application/x-x509-ca-cert",
        format!("{}\n", &pem[start..end]),
    );
    let name = format!(
        "attachment; filename=\"agent-instance-{}.crt\"",
        &state.instance_config().instance_id[..8]
    );
    response
        .headers_mut()
        .insert(header::CONTENT_DISPOSITION, name.parse().unwrap());
    Ok(response)
}
async fn check(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let mut response = public_response(
        "application/json",
        Json(json!({"instance_id":state.instance_config().instance_id,"reachable":true})),
    );
    // Only public reachability data crosses origins; never cookies or management data.
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        let allowed = state.addresses.read().unwrap()["welcome_urls"]
            .as_array()
            .is_some_and(|urls| urls.iter().any(|u| u.as_str() == Some(origin)));
        if allowed {
            response
                .headers_mut()
                .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.parse().unwrap());
            response
                .headers_mut()
                .insert(header::VARY, "Origin".parse().unwrap());
        }
    }
    response
}

/// Private clients trust our CA, while custom certificates retain their explicit trust anchor.
pub fn client_trust_file(home: &Path, certificate_file: &Path) -> std::path::PathBuf {
    let default = home.join("tls/cert.pem");
    if certificate_file
        .canonicalize()
        .ok()
        .is_some_and(|p| Some(p) == default.canonicalize().ok())
        && home.join("tls/root-ca.crt").is_file()
    {
        home.join("tls/root-ca.crt")
    } else {
        certificate_file.to_owned()
    }
}

pub fn configured_cert(home: &Path, config: &crate::config::InstanceConfig) -> std::path::PathBuf {
    expand(
        home,
        config
            .server
            .certificate_file
            .as_deref()
            .unwrap_or("${AGENT_HOME}/tls/cert.pem"),
    )
    .into()
}
