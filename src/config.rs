use anyhow::{Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigDocument {
    pub revision: u64,
    pub config: InstanceConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstanceConfig {
    pub schema_version: u32,
    pub instance_id: String,
    pub name: String,
    pub purpose: String,
    pub instructions: String,
    pub owner: String,
    pub setup_complete: bool,
    pub enabled: bool,
    pub server: ServerConfig,
    pub backend: Option<BackendConfig>,
    pub limits: Limits,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub bind: String,
    pub port: u16,
    /// Public certificate help only. None disables HTTP; zero chooses a stable free port.
    #[serde(default = "default_welcome_port")]
    pub welcome_port: Option<u16>,
    pub public_url: Option<String>,
    pub pam_service: String,
    pub embed_origins: Vec<String>,
    pub certificate_file: Option<String>,
    pub private_key_file: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BackendConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub secret_env: BTreeMap<String, String>,
    pub working_directory: String,
    pub allowed_working_roots: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub max_connections: usize,
    pub max_frame_bytes: usize,
    pub backend_start_timeout_seconds: u64,
    pub idle_timeout_seconds: u64,
    pub event_retention: usize,
}

impl InstanceConfig {
    pub fn blank(name: String, owner: String) -> Self {
        Self {
            schema_version: 1,
            instance_id: uuid::Uuid::new_v4().to_string(),
            name,
            purpose: String::new(),
            instructions: String::new(),
            owner,
            setup_complete: false,
            enabled: false,
            server: ServerConfig {
                bind: "0.0.0.0".into(),
                port: 0,
                welcome_port: default_welcome_port(),
                public_url: None,
                pam_service: if cfg!(target_os = "macos") {
                    "login"
                } else {
                    "agent-instance"
                }
                .into(),
                embed_origins: vec![],
                certificate_file: None,
                private_key_file: None,
            },
            backend: None,
            limits: Limits {
                max_connections: 8,
                max_frame_bytes: 4 * 1024 * 1024,
                backend_start_timeout_seconds: 30,
                idle_timeout_seconds: 1800,
                event_retention: 10000,
            },
        }
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema_version == 1, "Unsupported configuration schema");
        uuid::Uuid::parse_str(&self.instance_id)?;
        ensure!(
            !self.name.trim().is_empty() && self.name.len() <= 200,
            "Name must contain 1–200 bytes"
        );
        ensure!(
            !self.owner.is_empty() && self.owner.len() < 100 && !self.owner.contains(['\n', '\0']),
            "Invalid owner"
        );
        ensure!(
            self.instructions.len() <= 128 * 1024 && self.purpose.len() <= 16000,
            "Instructions or purpose too large"
        );
        self.server.bind.parse::<std::net::IpAddr>()?;
        ensure!(
            self.server.port == 0 || self.server.welcome_port != Some(self.server.port),
            "The welcome page and secure service need different ports"
        );
        ensure!(
            self.server.certificate_file.is_some() == self.server.private_key_file.is_some(),
            "Certificate and key must be supplied together"
        );
        ensure!(
            valid_name(&self.server.pam_service),
            "Invalid PAM service name"
        );
        for origin in &self.server.embed_origins {
            let url = reqwest::Url::parse(origin)?;
            ensure!(
                url.scheme() == "https" && url.origin().ascii_serialization() == *origin,
                "Embed origins must be exact HTTPS origins"
            );
        }
        if let Some(url) = &self.server.public_url {
            let url = reqwest::Url::parse(url)?;
            ensure!(
                url.scheme() == "https"
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "Public URL must use HTTPS without credentials, a query, or a fragment"
            );
        }
        ensure!(
            (1..=64).contains(&self.limits.max_connections),
            "Connection limit must be 1–64"
        );
        ensure!(
            (1024..=16 * 1024 * 1024).contains(&self.limits.max_frame_bytes),
            "Invalid message size limit"
        );
        ensure!(
            (1..=300).contains(&self.limits.backend_start_timeout_seconds),
            "Invalid startup timeout"
        );
        ensure!(
            (30..=86400).contains(&self.limits.idle_timeout_seconds),
            "Invalid idle timeout"
        );
        ensure!(
            (100..=100000).contains(&self.limits.event_retention),
            "Invalid event retention"
        );
        if let Some(b) = &self.backend {
            ensure!(
                !b.command.trim().is_empty() && !b.command.contains('\0'),
                "Backend command is required"
            );
            ensure!(
                !b.working_directory.is_empty(),
                "Working directory is required"
            );
            for (key, value) in &b.env {
                ensure!(
                    valid_env(key) && !value.contains('\0'),
                    "Invalid backend environment entry"
                );
            }
            for (key, value) in &b.secret_env {
                ensure!(
                    valid_env(key) && valid_name(value) && value != "control-token",
                    "Invalid secret reference"
                );
            }
            ensure!(
                !b.args.iter().any(|v| v.contains('\0')),
                "Invalid backend arguments"
            );
        }
        ensure!(
            !self.setup_complete || self.backend.is_some(),
            "Choose an ACP backend before completing setup"
        );
        ensure!(
            !self.enabled || self.setup_complete,
            "Complete setup before enabling the agent"
        );
        Ok(())
    }
}
fn default_welcome_port() -> Option<u16> {
    Some(0)
}
pub fn valid_name(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 80
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
pub fn valid_env(v: &str) -> bool {
    !v.is_empty()
        && !v.as_bytes()[0].is_ascii_digit()
        && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
pub fn expand(home: &Path, value: &str) -> String {
    value.replace("${AGENT_HOME}", &home.to_string_lossy())
}
pub fn private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    crate::platform::windows::private_dir(path)?;
    Ok(())
}
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&temp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(temp, path)?;
    Ok(())
}
