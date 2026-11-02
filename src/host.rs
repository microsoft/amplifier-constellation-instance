use crate::{
    auth,
    config::{ConfigDocument, InstanceConfig, expand, private_dir, write_private},
    server,
    state::{AppState, now_ms},
    welcome,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    net::{IpAddr, TcpListener},
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(unix)]
pub fn current_user() -> Result<String> {
    let output = std::process::Command::new("id").arg("-un").output()?;
    ensure!(output.status.success(), "Cannot identify current OS user");
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
#[cfg(windows)]
pub fn current_user() -> Result<String> {
    crate::platform::windows::current_user()
}
pub fn local_addresses() -> Vec<IpAddr> {
    let mut addresses: Vec<_> = get_if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|i| i.ip())
        .filter(|ip| {
            !ip.is_loopback()
                && !ip.is_unspecified()
                && !matches!(ip,IpAddr::V6(v6) if (v6.segments()[0]&0xffc0)==0xfe80)
        })
        .collect();
    addresses.sort();
    addresses.dedup();
    addresses
}
pub fn certificate_names() -> Vec<String> {
    let mut sans = vec![
        "localhost".to_owned(),
        "127.0.0.1".to_owned(),
        "::1".to_owned(),
    ];
    sans.extend(local_addresses().iter().map(ToString::to_string));
    if let Ok(hostname) = std::fs::read_to_string("/etc/hostname")
        && !hostname.trim().is_empty()
    {
        sans.push(hostname.trim().to_owned());
    }
    if let Ok(output) = std::process::Command::new("hostname").output()
        && output.status.success()
        && let Ok(hostname) = String::from_utf8(output.stdout)
        && !hostname.trim().is_empty()
    {
        sans.push(hostname.trim().to_owned());
    }
    sans.sort();
    sans.dedup();
    sans
}
pub fn init(
    home: &Path,
    name: String,
    owner: Option<String>,
    bind: String,
    port: u16,
) -> Result<Value> {
    ensure!(
        !home.join("config.json").exists(),
        "This folder already contains an instance; use config set or the dashboard to edit it"
    );
    let owner = owner.unwrap_or(current_user()?);
    ensure!(
        owner == current_user()?,
        "Install as the OS user who will own this instance"
    );
    let mut config = InstanceConfig::blank(name, owner);
    config.server.bind = bind;
    config.server.port = port;
    config.validate()?;
    private_dir(home)?;
    for folder in ["tls", "secrets", "profiles", "workspace", "backend"] {
        private_dir(&home.join(folder))?;
    }
    welcome::prepare(home, &config.instance_id, certificate_names())?;
    for file in ["communication.md", "working-style.md", "identity.md"] {
        write_private(&home.join("profiles").join(file), b"")?;
    }
    let document = ConfigDocument {
        revision: 1,
        config,
    };
    write_private(
        &home.join("config.json"),
        &serde_json::to_vec_pretty(&document)?,
    )?;
    let state = AppState::open(home.to_path_buf())?;
    let token = auth::issue_token(
        &state,
        "Local installation control",
        &["admin".into()],
        366 * 86400,
    )
    .map_err(|e| anyhow::anyhow!(e.1))?;
    write_private(
        &home.join("secrets/control-token"),
        token["token"].as_str().unwrap().as_bytes(),
    )?;
    state.event(
        "instance.created",
        json!({"instance_id":document.config.instance_id,"version":env!("CARGO_PKG_VERSION")}),
    )?;
    Ok(
        json!({"home":home,"instance_id":document.config.instance_id,"name":document.config.name,"setup_complete":false,"next":"Start the service, then open one of its HTTPS URLs"}),
    )
}
fn url(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(ip) => format!("https://{ip}:{port}"),
        IpAddr::V6(ip) => format!("https://[{ip}]:{port}"),
    }
}
#[cfg(unix)]
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}
#[cfg(windows)]
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}
pub async fn serve(home: PathBuf) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    ensure!(
        current_user()? != "root",
        "Run this service as its ordinary OS owner, not root"
    );
    let state = Arc::new(
        AppState::open(dunce::canonicalize(&home)?).context("Cannot open this instance state")?,
    );
    ensure!(
        state.instance_config().owner == current_user()?,
        "This service must run as its configured OS owner"
    );
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state.home.join("service.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock_file)
        .context("This instance already has a running service")?;
    let config = state.instance_config();
    let bind: IpAddr = config.server.bind.parse()?;
    let listener = TcpListener::bind((bind, config.server.port))
        .context("Cannot bind the configured port (it may already be occupied)")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let welcome_listener = if let Some(welcome_port) = config.server.welcome_port {
        let listener = TcpListener::bind((bind, welcome_port))
            .context("Cannot bind the certificate-help port (it may already be occupied)")?;
        listener.set_nonblocking(true)?;
        Some(listener)
    } else {
        None
    };
    let welcome_port = welcome_listener
        .as_ref()
        .map(|l| l.local_addr().map(|a| a.port()))
        .transpose()?;
    if config.server.port != port || config.server.welcome_port != welcome_port {
        let mut document = state.config.write().unwrap();
        document.config.server.port = port;
        document.config.server.welcome_port = welcome_port;
        document.revision += 1;
        write_private(
            &state.home.join("config.json"),
            &serde_json::to_vec_pretty(&*document)?,
        )?;
    }
    let config = state.instance_config();
    if config.server.certificate_file.is_none() {
        welcome::prepare(&state.home, &config.instance_id, certificate_names())?;
    }
    let cert = welcome::configured_cert(&state.home, &config);
    let key = PathBuf::from(expand(
        &state.home,
        config
            .server
            .private_key_file
            .as_deref()
            .unwrap_or("${AGENT_HOME}/tls/key.pem"),
    ));
    let fingerprint = welcome::fingerprint(&cert)?;
    let trust_certificate_file = welcome::client_trust_file(&state.home, &cert);
    let trust_fingerprint = welcome::fingerprint(&trust_certificate_file)?;
    let loopback: IpAddr = if bind.is_unspecified() {
        if bind.is_ipv4() {
            "127.0.0.1".parse()?
        } else {
            "::1".parse()?
        }
    } else {
        bind
    };
    let base_url = url(loopback, port);
    let mut urls = vec![base_url.clone()];
    if bind.is_unspecified() {
        urls.extend(
            local_addresses()
                .into_iter()
                .filter(|a| bind.is_ipv6() || a.is_ipv4())
                .map(|ip| url(ip, port)),
        );
    }
    if let Some(public) = &config.server.public_url {
        urls.push(public.trim_end_matches('/').to_owned());
    }
    urls.sort();
    urls.dedup();
    let exe = std::env::current_exe()?;
    let command = format!(
        "{}{} acp --home {}",
        if cfg!(windows) { "& " } else { "" },
        shell_quote(&exe.to_string_lossy()),
        shell_quote(&state.home.to_string_lossy())
    );
    let welcome_url = welcome_port.map(|p| url(loopback, p).replace("https://", "http://"));
    let welcome_urls: Vec<String> = welcome_port
        .map(|p| {
            urls.iter()
                .filter_map(|u| {
                    let mut parsed = reqwest::Url::parse(u).ok()?;
                    // Only direct listener addresses; reverse proxies have their own entrypoints.
                    if parsed.port_or_known_default() != Some(port) {
                        return None;
                    }
                    parsed.set_scheme("http").ok()?;
                    parsed.set_port(Some(p)).ok()?;
                    Some(parsed.origin().ascii_serialization())
                })
                .collect()
        })
        .unwrap_or_default();
    let addresses = json!({"base_url":base_url,"urls":urls,"certificate_sha256":fingerprint,"certificate_file":cert,"trust_certificate_file":trust_certificate_file,"trust_certificate_sha256":trust_fingerprint,"welcome_url":welcome_url,"welcome_urls":welcome_urls,"certificate_help_url":"welcome","acp_attach":command,"acp":{"command":exe,"args":["acp","--home",state.home],"transport":"stdio"},"remote_bridge":{"url":format!("{}/api/v1/acp",base_url.replace("https://","wss://")),"profile":"agent-instance.wss/1","authentication":"Bearer token; acp scope"},"effective_server":config.server,"started_at":now_ms(),"pid":std::process::id()});
    write_private(
        &state.home.join("endpoints.json"),
        &serde_json::to_vec_pretty(&addresses)?,
    )?;
    *state.addresses.write().unwrap() = addresses.clone();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
    state.event(
        "service.started",
        json!({"version":env!("CARGO_PKG_VERSION"),"port":port}),
    )?;
    eprintln!(
        "Agent instance is ready. Open {} and sign in as {}.\nCertificate SHA-256: {}",
        urls.iter()
            .find(|u| !u.contains("127.0.0.1") && !u.contains("[::1]"))
            .unwrap_or(&base_url),
        config.owner,
        fingerprint
    );
    let handle = axum_server::Handle::new();
    let http_handle = axum_server::Handle::new();
    let welcome_task = welcome_listener.map(|listener| {
        let state = state.clone();
        let handle = http_handle.clone();
        tokio::spawn(async move {
            axum_server::from_tcp(listener)?
                .handle(handle)
                .serve(welcome::router(state).into_make_service())
                .await
        })
    });
    let (scheduler_stop, scheduler_task) =
        crate::scheduler::start(state.clone()).map_err(|e| anyhow::anyhow!(e.1))?;
    let shutdown_handle = handle.clone();
    let shutdown_http_handle = http_handle.clone();
    let shutdown_state = state.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            if let Ok(mut term) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                tokio::select! {_=tokio::signal::ctrl_c()=>(),_=term.recv()=>()};
            } else {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        shutdown_state
            .runtime_enabled
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let _ = shutdown_state.runtime_stop.send(());
        shutdown_http_handle.graceful_shutdown(Some(std::time::Duration::from_secs(2)));
        shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(8)));
    });
    let served = axum_server::from_tcp_rustls(listener, tls)?
        .handle(handle)
        .serve(server::router(state.clone()).into_make_service())
        .await;
    let _ = scheduler_stop.send(());
    let _ = scheduler_task.await;
    http_handle.shutdown();
    if let Some(task) = welcome_task {
        let _ = task.await;
    }
    let _ = state.event("service.stopped", json!({}));
    drop(lock_file);
    served?;
    Ok(())
}
pub async fn api(home: &Path, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    let endpoints: Value = serde_json::from_slice(
        &std::fs::read(home.join("endpoints.json"))
            .context("Start the service first; endpoints.json is not available")?,
    )?;
    let base = endpoints["base_url"]
        .as_str()
        .context("No service URL is available")?;
    let cert_path = endpoints["certificate_file"]
        .as_str()
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("tls/cert.pem"));
    let certificate = reqwest::Certificate::from_pem(&std::fs::read(welcome::client_trust_file(
        home, &cert_path,
    ))?)?;
    let token = std::fs::read_to_string(home.join("secrets/control-token"))?;
    let client = reqwest::Client::builder()
        .add_root_certificate(certificate)
        .timeout(std::time::Duration::from_secs(
            if path.contains("backend/") { 360 } else { 30 },
        ))
        .build()?;
    ensure!(
        !path.contains("..") && !path.contains("://"),
        "Path must be a management API resource"
    );
    let path = path
        .trim_start_matches('/')
        .strip_prefix("api/v1/")
        .unwrap_or(path.trim_start_matches('/'));
    let mut request = client
        .request(method.parse()?, format!("{base}/api/v1/{path}"))
        .bearer_auth(token.trim());
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let text = response.text().await?;
    let value =
        serde_json::from_str::<Value>(&text).context("Service returned a non-JSON response")?;
    ensure!(status.is_success(), "{}: {}", status, value);
    Ok(value)
}
