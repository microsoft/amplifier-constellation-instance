# ACP boundary

This instance is an ACP agent endpoint backed by a separately launched ACP agent implementation. The management UI, configuration API, profile editing and runtime observations are a separate interface. Ordinary ACP clients do not need that management API.

The normative compatibility entry point is the `acp` attach command printed by setup. It speaks newline-delimited JSON-RPC on stdin/stdout. Diagnostics go to stderr. It attaches to this individual, rather than installing or creating another individual.

The attach command connects to the running service at `/api/v1/acp` using authenticated WSS. This network hop is the **agent-instance.wss/1 custom framing profile**: one JSON-RPC object per WebSocket text message. It is not the proposed official ACP Streamable HTTP transport. Each connection has an independent backend stdio process, negotiated capabilities and active-session bindings. All processes belong to this one installed individual; they share only the backend state that its configuration intentionally supplies.

## Backend behavior remains authoritative

The relay forwards initialization, authentication, prompts, content blocks, MCP server configuration, session configuration, cancellation, callbacks, updates and extension methods. It does not manufacture `loadSession`, list, resume or other capabilities. Reconnection continuity therefore requires a backend that advertises and implements persistent sessions. A client must create, load or resume a session on its connection before prompting it.

The frontend `agentInfo` identifies this installed wrapper. The original backend identity and the individual instance ID appear in the namespaced `io.github.bkrabach.agent-instance` entry in initialization `_meta`. Other metadata is retained.

That same namespace optionally publishes `management` discovery with profile `agent-instance.management/1`, an absolute `descriptorUrl`, and `authentication: {required: true, separateFromAcp: true}`. It prefers the configured public URL, then a nonloopback service address, then the local base URL; it is omitted if no service address is available. This private extension lets tools discover the separate management interface without adding ACP capabilities or granting management access. The descriptor still requires a management credential with the `observe` scope; ACP provider authentication does not authorize it.

Permission, filesystem and terminal callbacks travel back to the actual ACP client. The wrapper does not silently approve them or pretend the client's filesystem is the service machine's filesystem. Whether a backend runs its own tools or requests client tools is backend behavior and must be configured deliberately.

Instance name, purpose, instructions, and configured profile documents are prepended as a separate text block to each prompt. This supplies ordinary ACP context, **not a privileged system-message channel**. Clients still submit normal ACP content, and backends may replay that configured text as part of the saved conversation. Do not put credentials in those profiles. Provider credentials belong in named secret references supplied to the backend environment.

## Optional in-turn input

The pre-standard `_session/steering` extension is available only when the backend
advertises `_meta.steering.supported` during initialization. Instance preserves
that capability and the backend's outcomes; it does not add steering to a plain
ACP engine. Requests require an already loaded session, a nonempty `prompt`
array, and `_meta.steering.idleBehavior: "promptRequired"`. This prevents an idle
request from silently starting an untracked second turn.

Steering receives the same configured identity/profile block, fresh authorized
context, tool-access checks, and optional attention-ticket validation and
consumption as a prompt. The existing audience binding remains authoritative.
Input IDs, client metadata and block-local source/snapshot metadata are preserved;
attention credentials are consumed and stripped before reaching the backend.
An admitted attention event is an input admission, not a fresh task allowance.

The active prompt keeps its work ID and update-admission guard. Ordinary steering
neither cancels that prompt nor dismisses a pending permission callback. Backend
input receipts, including empty text updates carrying metadata and the final
prompt's receipt list, pass through unchanged. `promptRequired`, failed or
unacknowledged input does not cause Instance to issue a fallback prompt. The
client owns its durable queue and must reconcile receipt state before retrying;
the backend's acceptance outcome alone is not proof of model incorporation.

## Optional conversation continuity

Version 0.4 can merge an agent-owned HTTP MCP server into the standard session MCP configuration when the backend advertises HTTP support. Native owner clients need no custom methods. Adapters may supply an optional namespaced conversation grant; the wrapper validates and strips it before forwarding, binds the audience to the session, and keeps credentials out of prompts and observations. Original client MCP entries remain intact. [Provider, authorization and migration contract](conversation-continuity.md).

## Deployment boundaries

- Each backend receives a cleared environment, a small OS-variable allowlist and explicitly configured environment/secret entries. HOME and XDG state directories default to this installation's private `backend/` folder; explicit backend environment settings can override them. Slack credentials and the parent process's arbitrary environment are not inherited.
- `cwd` and additional workspace roots must be existing absolute directories beneath configured roots, resolving symlinks before comparison. Provided `cwd` is never silently replaced. These checks scope session creation; they are not an OS filesystem sandbox for the backend's tools.
- Frames, startup time, idle time, connection count and output backpressure are bounded. The Linux process group is terminated on disconnect, service stop, error or task cancellation, including descendants that remain in the group. Daemonized processes that deliberately leave it require a stronger OS containment mechanism.
- PAM/HTTP access control is independent of ACP provider authentication. WSS validates certificates. Local attach trusts the installation's `tls/root-ca.crt` for managed certificates, or the actual custom certificate path recorded in `endpoints.json`. Legacy installations without a local CA continue to use their `tls/cert.pem`; public roots apply when no local trust file exists.
- Observation stores method names, session IDs, timestamps, connection/implementation information and status metadata. It does not store raw prompts, profile bodies, MCP credentials, callback bodies, model output, or backend stderr. The backend may independently persist its own authorized history.
- Each connection reports the atomically captured `config_revision` that supplied its fixed backend settings. Session records report `last_profile_sha256` for the exact combined profile text most recently forwarded, `last_profile_supplied_at`, and `last_prompt_config_revision` for that prompt's name/purpose/instructions. A connection can therefore retain older backend settings while receiving newer profile and instruction context. These fields describe what was supplied, not a claim that the backend accepted or used it.
- Failures before the ACP relay loop emit `acp.launch_failed` with the failed setup stage and a suggested operator action. These events omit the raw error, command arguments, environment values and backend stderr.
- Prompt boundaries emit `acp.prompt.started` and a terminal `finished`, `cancelled`, or `failed` event with a generated work ID, connection/session IDs, duration, and normalized stop reason. Unfinished prompts receive failure observations when the connection ends. A finished turn means the backend returned a response; it is not a claim that the user's larger task is complete. The Activity API presents these outcomes without content or token-stream noise. Configured event retention applies to meaningful activity; diagnostics have their own 1,000-event budget.

The deployed Linux and macOS installations use this same ACP boundary. Service packaging and authentication remain platform-specific; see [native platform status](native-platforms.md).

## Verification

`tests/fixtures/acp_backend.py` is an independently implemented Python peer. Rust integration tests exercise standard resource links, per-session configuration, MCP descriptors, auth, permission and filesystem callbacks (including overlapping request IDs in opposite directions), session persistence/replay, cancellation, concurrent connections, directory rejection, secret-free observation and process-group cleanup.

`tests/acp-client/official-client.mjs` uses the pinned official TypeScript SDK 2.2.0 against the standard attach command. Run it against a test installation configured with the Python fixture:

```sh
npm ci --prefix tests/acp-client
node tests/acp-client/official-client.mjs /absolute/test/work /absolute/agent-instance acp --home /absolute/test/instance
```

For a complete temporary TLS installation, real WSS transport, official SDK run, unauthenticated-access check, and shutdown-with-open-stdin regression check, run `python3 tests/acp_wss_smoke.py` after building. It removes its temporary installation afterward and does not change any existing agent.
Add `--custom-certificate` to exercise a different self-signed certificate outside the instance's default TLS directory while retaining the original certificate there.

The fixture tests transport and lifecycle mechanics; they do not prove an arbitrary model backend implements every advertised ACP feature. The official ACP TCK and a real external-client/backend matrix remain useful additional release gates. A passing experimental TCK result is not official protocol certification.

## Primary references checked October 31, 2026

- [Stable ACP v1 transport](https://agentclientprotocol.com/protocol/v1/transports)
- [Initialization and capability negotiation](https://agentclientprotocol.com/protocol/v1/initialization)
- [Session setup and replay](https://agentclientprotocol.com/protocol/v1/session-setup)
- [Authentication](https://agentclientprotocol.com/protocol/v1/authentication)
- [Extensibility](https://agentclientprotocol.com/protocol/v1/extensibility)
- [Active remote transport proposal](https://agentclientprotocol.com/rfds/streamable-http-websocket-transport)
- [Official compatibility test kit](https://github.com/agentclientprotocol/acp-tck)

## Optional host attention

[Attention](attention.md) is consumed by Instance before a prompt or steering input
reaches the backend. Advertised backend capabilities, authentication, permissions, streaming
and session state remain backend-owned. Plain clients need not adopt the
extension; plain engines do not implement it. Engine-family changes require a
new installation; this boundary does not promise portable session files.
