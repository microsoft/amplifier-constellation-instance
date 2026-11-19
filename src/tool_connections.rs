//! Installation-owned tools travel over ordinary ACP HTTP MCP descriptors.
//! This module grants no conversation access and provides no execution policy.
use crate::{auth, config::valid_name, state::AppState};
use anyhow::{Result, ensure};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub const SCOPE: &str = "tools:use";
const MAX_SERVERS: usize = 8;
const MAX_HEADER_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub header_secrets: BTreeMap<String, String>,
    /// Allow this integration to be unavailable without blocking unrelated work.
    #[serde(default)]
    pub optional: bool,
}

pub fn validate(connections: &[Connection]) -> Result<()> {
    ensure!(
        connections.len() <= MAX_SERVERS,
        "At most 8 tool connections are supported"
    );
    let mut names = BTreeSet::new();
    let mut endpoints = BTreeSet::new();
    for connection in connections {
        ensure!(
            valid_name(&connection.name) && connection.name != "agent-continuity",
            "Use a distinct non-reserved tool connection name"
        );
        ensure!(
            names.insert(&connection.name),
            "Duplicate tool connection name"
        );
        ensure!(connection.url.len() <= 2048, "Tool endpoint is too long");
        let url = reqwest::Url::parse(&connection.url)
            .map_err(|_| anyhow::anyhow!("Invalid tool endpoint"))?;
        let loopback = url
            .host_str()
            .and_then(|h| h.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
        ensure!(
            (url.scheme() == "https" || (url.scheme() == "http" && loopback))
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "Tool endpoints require HTTPS or literal loopback HTTP, without credentials, query or fragment"
        );
        ensure!(endpoints.insert(url.to_string()), "Duplicate tool endpoint");
        ensure!(
            connection.header_secrets.len() <= 8,
            "At most 8 secret headers per connection"
        );
        let mut headers = BTreeSet::new();
        for (header, reference) in &connection.header_secrets {
            let name = HeaderName::from_bytes(header.as_bytes())
                .map_err(|_| anyhow::anyhow!("Invalid tool header name"))?;
            ensure!(
                ![
                    "host",
                    "connection",
                    "content-length",
                    "transfer-encoding",
                    "content-type",
                    "accept",
                    "upgrade",
                    "mcp-session-id",
                    "mcp-protocol-version"
                ]
                .contains(&name.as_str())
                    && header.len() <= 128,
                "Transport headers are managed by the MCP client"
            );
            ensure!(
                headers.insert(name.as_str().to_owned()),
                "Duplicate tool header name"
            );
            ensure!(
                valid_name(reference) && reference != "control-token",
                "Tool headers must reference a non-reserved stored secret"
            );
        }
    }
    Ok(())
}

pub fn allowed(state: &AppState, headers: Option<&HeaderMap>) -> bool {
    headers.is_some_and(|headers| auth::explicit_scope(state, headers, SCOPE))
}

/// Token issuance resolves an owner-selected name to the entire credential-free descriptor.
pub(crate) fn bound_scope(c: &Connection) -> String {
    let descriptor = serde_json::to_string(c).expect("serializable tool connection");
    format!("tools:bound:{}", auth::digest(&descriptor))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Authority {
    None,
    Full,
    Single(String),
}

impl Authority {
    pub fn current(state: &AppState, headers: Option<&HeaderMap>) -> Self {
        let scopes = headers
            .map(|h| auth::explicit_scopes(state, h))
            .unwrap_or_default();
        if scopes.iter().any(|s| s == SCOPE) {
            Self::Full
        } else if let Some(scope) = scopes.into_iter().find(|s| s.starts_with("tools:bound:")) {
            Self::Single(scope)
        } else {
            Self::None
        }
    }

    fn record(&self) -> Option<Value> {
        match self {
            Self::None => None,
            Self::Full => Some(json!({"tools_required":true})),
            Self::Single(scope) => Some(json!({"bound_scope":scope})),
        }
    }

    pub fn permits(&self, stored: &Value) -> bool {
        self.record().as_ref() == Some(stored)
    }
}

/// Hold compatible authority from before a backend load (including its replay) until close.
/// Pending, failed, cancelled and disconnected relays drop their own reservations.
pub struct Attachment {
    state: Arc<AppState>,
    session: String,
    authority: Authority,
}

impl Attachment {
    pub fn reserve(state: &Arc<AppState>, session: &str, authority: &Authority) -> Result<Self> {
        ensure!(
            !session.is_empty() && session.len() <= 512,
            "Invalid session identity"
        );
        let mut live = state.tool_attachments.lock().unwrap();
        if let Some(record) = state.get_record("session_tool_access", session)? {
            ensure!(
                authority.permits(&record),
                "Different tool authority is required for this session"
            );
        }
        let (held, n) = live
            .entry(session.to_owned())
            .or_insert_with(|| (authority.clone(), 0));
        ensure!(
            held == authority,
            "An attached client has different tool authority"
        );
        *n += 1;
        Ok(Self {
            state: Arc::clone(state),
            session: session.to_owned(),
            authority: authority.clone(),
        })
    }
    pub fn commit(&self) -> Result<()> {
        if let Some(record) = self.authority.record() {
            self.state
                .set_record("session_tool_access", &self.session, record)?;
        }
        Ok(())
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        let mut live = self.state.tool_attachments.lock().unwrap();
        if let Some((_, n)) = live.get_mut(&self.session) {
            *n -= 1;
            if *n == 0 {
                live.remove(&self.session);
            }
        }
    }
}

fn selected(registry: &[Connection], authority: &Authority) -> Result<Vec<Connection>> {
    match authority {
        Authority::None => Ok(Vec::new()),
        Authority::Full => Ok(registry.to_vec()),
        Authority::Single(scope) => {
            let c = registry
                .iter()
                .find(|c| c.header_secrets.is_empty() && bound_scope(c) == *scope)
                .ok_or_else(|| anyhow::anyhow!("Bound tool descriptor is no longer registered"))?;
            Ok(vec![c.clone()])
        }
    }
}

/// Only digests remain in the revocation guard. The resolved descriptors are
/// delivered to the trusted ACP backend and are never recorded by Instance.
pub struct Guard {
    pub authority: Authority,
    connections: Vec<Connection>,
    secret_digests: BTreeMap<String, String>,
}
impl Guard {
    pub fn check(&self, state: &AppState, headers: Option<&HeaderMap>) -> Result<()> {
        ensure!(
            Authority::current(state, headers) == self.authority
                && headers.is_some_and(|headers| auth::check(state, headers, "acp", false).is_ok()),
            "Tool connection authorization ended; reconnect with the required tool grant"
        );
        ensure!(
            selected(&state.instance_config().tool_connections, &self.authority)?
                == self.connections,
            "Tool connections changed; reconnect to apply them"
        );
        for (reference, expected) in &self.secret_digests {
            ensure!(
                state
                    .read_secret(reference)
                    .is_ok_and(|secret| auth::digest(&secret) == *expected),
                "Tool credentials changed; reconnect to apply them"
            );
        }
        Ok(())
    }
}

/// Prepare after continuity so the total budget includes every descriptor.
pub fn prepare(
    state: &AppState,
    headers: Option<&HeaderMap>,
    params: &mut Value,
    http_supported: bool,
    required_for_session: bool,
) -> Result<Option<Guard>> {
    let registry = state.instance_config().tool_connections;
    let authority = Authority::current(state, headers);
    let connections = selected(&registry, &authority)?;
    if authority == Authority::None || (connections.is_empty() && !required_for_session) {
        return Ok(None);
    }
    validate(&registry)?;
    ensure!(
        connections.is_empty() || http_supported,
        "This backend does not support configured HTTP MCP tools"
    );
    let supplied = params
        .get("mcpServers")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("mcpServers must be an array"))?;
    ensure!(
        supplied.len() + connections.len() <= MAX_SERVERS,
        "Client, continuity and installation MCP connections exceed the combined limit of 8"
    );
    for item in supplied {
        ensure!(
            !registry
                .iter()
                .any(|connection| item["name"] == connection.name
                    || item["url"]
                        .as_str()
                        .is_some_and(|raw| reqwest::Url::parse(raw).ok()
                            == reqwest::Url::parse(&connection.url).ok())),
            "Client MCP connections cannot override installation tool names or endpoints"
        );
    }
    let mut descriptors = Vec::new();
    let mut secret_digests = BTreeMap::new();
    let mut total_bytes = 0;
    for connection in &connections {
        let mut values = Vec::new();
        for (header, reference) in &connection.header_secrets {
            let secret = state
                .read_secret(reference)
                .map_err(|_| anyhow::anyhow!("A tool connection credential is unavailable"))?;
            total_bytes += header.len() + secret.len();
            ensure!(
                !secret.is_empty()
                    && secret.len() <= 8192
                    && total_bytes <= MAX_HEADER_BYTES
                    && HeaderValue::from_str(&secret).is_ok()
                    && !secret.contains(['\r', '\n', '\0']),
                "A tool credential is invalid or exceeds the header budget"
            );
            secret_digests.insert(reference.clone(), auth::digest(&secret));
            values.push(json!({"name":header,"value":secret}));
        }
        descriptors.push(
            json!({"type":"http","name":connection.name,"url":connection.url,"headers":values,"_meta":{"io.github.bkrabach.agent-runtime":{"optional":connection.optional}}}),
        );
    }
    params["mcpServers"]
        .as_array_mut()
        .unwrap()
        .extend(descriptors);
    Ok(Some(Guard {
        authority,
        connections,
        secret_digests,
    }))
}
