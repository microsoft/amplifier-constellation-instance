//! Installation-owned tools travel over ordinary ACP HTTP MCP descriptors.
//! This module grants no conversation access and provides no execution policy.
use crate::{auth, config::valid_name, state::AppState};
use anyhow::{Result, ensure};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

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

/// Only digests remain in the revocation guard. The resolved descriptors are
/// delivered to the trusted ACP backend and are never recorded by Instance.
pub struct Guard {
    connections: Vec<Connection>,
    secret_digests: BTreeMap<String, String>,
}
impl Guard {
    pub fn check(&self, state: &AppState, headers: Option<&HeaderMap>) -> Result<()> {
        ensure!(
            allowed(state, headers)
                && headers.is_some_and(|headers| auth::check(state, headers, "acp", false).is_ok()),
            "Tool connection authorization ended; reconnect with tools:use access"
        );
        ensure!(
            state.instance_config().tool_connections == self.connections,
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
    let connections = state.instance_config().tool_connections;
    if !allowed(state, headers) || (connections.is_empty() && !required_for_session) {
        return Ok(None);
    }
    validate(&connections)?;
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
            !connections
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
            json!({"type":"http","name":connection.name,"url":connection.url,"headers":values}),
        );
    }
    params["mcpServers"]
        .as_array_mut()
        .unwrap()
        .extend(descriptors);
    Ok(Some(Guard {
        connections,
        secret_digests,
    }))
}
