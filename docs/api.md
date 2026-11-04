# Management API v1

The management API configures one installation. It is independent of ACP session methods. All paths below are relative to `/api/v1`; the service also publishes `GET openapi.json`, `GET schema` (the configuration JSON Schema), and `GET descriptor`.

Optional versioned surfaces include [attributed memory](memory.md), [bounded background work](background-work.md), and [native engine configuration](native-configuration.md). Discover their capabilities before presenting their controls for an arbitrary ACP installation. Memory uses separate `memory:read`/`memory` grants; background execution remains separate from work-record editing; native engine configuration requires administration.

## Authentication

`POST auth/login` accepts `{ "username": "machine-owner", "password": "..." }`. PAM checks both authentication and account policy. Only the configured service owner can sign in. The first version supports a single ordinary `Password:` challenge; multi-step/MFA or password-change conversations fail closed. Up to two PAM workers run concurrently, callers time out after 20 seconds, and timed-out workers retain their concurrency slots until completion. Repeated failures impose a cooldown.

Success sets an instance-specific `Secure; HttpOnly; SameSite=Strict` cookie for eight hours and returns `{owner, csrf}`. `GET auth/session` retrieves the current browser session. Cookie-authenticated mutations require `X-CSRF-Token`; a supplied `Origin` must match the request's HTTPS origin. `POST auth/logout` invalidates the session.

Automation uses `Authorization: Bearer <token>`, never a URL query parameter. Tokens are stored hashed, have a bounded lifetime, and can be revoked. Scopes are:

| Scope | Access |
| --- | --- |
| `observe` | Status, discovery/schema, connections, session metadata, useful activity, runtime readiness, event history/stream |
| `ui` | Display state and commands; view events only when `observe` is absent |
| `acp` | Authenticated ACP WebSocket connection |
| `context:owner` | Owner-level conversation context through a verified ACP context binding; no management access |
| `tools:use` | Explicitly receive configured installation-owned MCP tools through ACP; requires `acp` separately. Neither `context:owner` nor `admin` implies this grant. |
| `profiles` | Inspect, apply, and restore versioned communication/working-style packages; no identity, credentials, or runtime configuration access |
| `work:read` | Read local responsibilities, assignments, templates, sharing policy and background execution reports; no mutations |
| `work` | Read/change responsibilities and prepare/receive explicit delegation envelopes; no policy or runtime configuration changes |
| `work:execute` | Configure the local background queue; authorize, cancel, reconcile or retry bounded runs; inspect execution reports. Does not imply ordinary work-record editing |
| `memory:read` | Read attributed local memories and their history; no changes |
| `memory` | Read, remember, resolve and archive attributed local memories; no background execution or configuration authority |
| `admin` | Configuration, profiles, credentials, tokens, lifecycle, and other management scopes. `tools:use` must still be granted explicitly. |

UI access includes editor drafts and can steer a visible display. Give it only to trusted controllers. It cannot apply configuration or run agent work through the view API. Use the corresponding administrative operation for a business action. Password/secret/token input values are excluded from view-state storage; ordinary draft text is private editable context, not a credential store.

Installation creates a private local control token for the CLI. A filesystem owner can recover access with `token create --local --name ... --scopes admin`; store the returned token privately and replace `secrets/control-token` if recovering local CLI access. The local bootstrap/recovery operation is not exposed over HTTP. Grants and sessions are rechecked on active streams within 15 seconds.

## Resources

| Resource | Methods | Contract |
| --- | --- | --- |
| `config` | GET, PUT | GET returns `{revision,config}`. PUT requires `{expected_revision,config}`. Stale saves return 409; instance ID and OS owner cannot change. |
| `config/validate` | POST | Validate a bare configuration without applying it. |
| `status` | GET | Instance identity/version, uptime, desired revision, enabled state, active connections, observed sessions, actual listener metadata, restart requirement. |
| `backend` | GET | Runtime type, installed version, account availability and setup actions. `auth_validated:false` means account availability is not a completed model-access test. |
| `backend/install` | POST | `{reuse_owner_account:true}` (default); installs pinned private runtime files, then applies configuration with an optimistic revision check. Five-minute download timeout, one installer per home. A concurrent config edit can leave installed files ready for retry while returning 409. |
| `backend/account` | GET | Admin-only provider account availability, excluding identifiers and credentials. |
| `backend/account/login` | GET, POST, DELETE | Admin-only inspect/start/cancel of device-code sign-in. The transient code stays out of views and event history. Only the official verified sign-in URL is accepted. |
| `native-account` | GET, DELETE | Native engine account status or explicit `{confirm:true}` revocation. Admin-only, separate from the default backend account. |
| `native-account/login` | GET, POST, DELETE | Native OAuth job with start/pending/cancel/expiry/recovery; loopback callback, no copied account credentials. [Contract](native-configuration.md). |
| `native-account/models` | POST | Admin-only native account model catalog. |
| `activity` | GET | Newest-first safe summaries: `{items,next_before,has_more}`. `limit` 1–200, `before` sequence, `category`, `search`, `diagnostics=false`. Diagnostics have an independent 1,000-row retention budget. |
| `lifecycle` | POST | `{action:"start"}` or `{action:"stop"}`. Persist enabled state; stopping closes backend connections while keeping management available. |
| `scheduler` | GET, PUT | Opt-in background execution settings, queue summary and backend approval. Reads require work-read/work/execute authority; PUT requires `work:execute`. |
| `work/{id}/execution` | GET, POST | Inspect runs/reports or explicitly queue, cancel, reconcile or retry. Mutations require `work:execute`; goal/run revisions and dedupe protect against accidental replay. [Contract](background-work.md). |
| `context/providers` | GET, PUT | Admin-only optional conversation registry. PUT `{providers:[{id,label,url,secret_ref}]}`; GET adds credential-presence flags. See [continuity contract](conversation-continuity.md). |
| `profile-package` | GET, POST | GET returns `{revision,active,previous,applied_at}`. POST `{expected_revision,package}` atomically installs a supported portable profile and preserves rollback history. |
| `profile-package/restore` | POST | `{expected_revision,id}` restores a previous local snapshot of both style files. |
| `profiles/{name}` | GET, PUT | Names: `communication`, `working-style`, `identity`. PUT `{text}`; GET returns text and SHA-256. New prompts read current profiles. |
| `secrets` | GET | Configured names only. |
| `secrets/{name}` | PUT, DELETE | PUT `{value}`. Values are write-only. Delete requires removing current backend and context-provider references first. `control-token` is reserved. |
| `tokens` | GET, POST | Metadata list; POST `{name,scopes,expires_in_seconds}` returns a token once. Lifetime: 60 seconds–366 days. |
| `tokens/{id}` | DELETE | Revoke a grant. |
| `connections` | GET | Current backend/client connection metadata. |
| `sessions` / `sessions/{id}` | GET | Observed session metadata; no copied transcript or invented backend capability. |
| `events?after=N` | GET | Up to 500 ordered events and `next_after`. Continue until caught up. Meaningful events use configured retention; diagnostics have their own 1,000-row budget. Sequence gaps after pruning are normal. |
| `events/stream?after=N` | GET | SSE events with sequence IDs; `Last-Event-ID` resumes within retained history. Re-read status after a long disconnection. |
| `views` | GET, POST | List displays; POST `{label}` creates one. |
| `views/{id}` | GET, PATCH | Inspect a display; PATCH `{expected_revision,state}` replaces its state. |
| `views/{id}/commands` | POST | `{expected_revision,command}` applies one semantic view command. |
| `views/{id}/ack` | POST | Browser reports `{revision,status:"applied"|"error",detail}`. A request is not proof that a visible display rendered it. |

Configuration replacement is atomic on disk. Backend configuration snapshots apply to new ACP connections; existing connections keep their captured implementation settings. Profile files are read at prompt boundaries. Server settings require a process restart; the dashboard distinguishes saved settings from `addresses.effective_server`. A port of zero chooses and persists a free port on the next start. A conflicting fixed port fails visibly.

Set `server.public_url` to the preferred reachable HTTPS base URL when a host has several network interfaces or sits behind a proxy. Optional ACP management discovery prefers that explicit address. The URL can contain a proxy prefix, but cannot contain credentials, a query, or a fragment.

Events contain metadata and actor IDs, not raw prompts, profile bodies, secret values, callback bodies, or backend stderr. `view.changed` stores only ID/revision/actor; fetch the view using a UI grant to read drafts. The backend may retain its own private conversation state.

## Example: direct an open display

First list displays with `agent-instance api GET views --home /path/to/instance`, then read the chosen view. Put this JSON in a file:

```json
{
  "expected_revision": 7,
  "command": {"type": "navigate", "route": "connections"}
}
```

Submit it with `agent-instance api POST views/DISPLAY_ID/commands --file command.json --home /path/to/instance`. Read the view again and check that `ack.revision` matches the returned revision and `ack.status` is `applied`. A background browser can render too; the acknowledgement detail reports browser visibility. Do not treat another controller's newer revision as acknowledgement of your exact request.

Other commands are `set_draft {path,value}`, `focus {target}`, `scroll {x,y}`, `select_session {session_id}`, `set_filter {key,value}`, `open_modal {modal}`, and `close_modal`. Draft paths include `config.name`, `config.backend.command`, and `profiles.communication`. Commands change view state; applying configuration/profile edits still uses the corresponding API operation.

All UI business actions use the same API. The bundled browser additionally exposes `window.agentInstanceControl` for semantic inspection/control and browser-local actions such as copying connection information. See [the UI contract](ui-contract.md) for the embedding bridge and control IDs. Same-origin reverse-proxy embedding is the first supported integration shape; cross-site sign-in needs a future auth handoff, not looser cookie rules.

## Failure and concurrency behavior

Application errors return `{error,detail}` and meaningful HTTP status codes. Invalid framework-level JSON may be rejected before the application handler. Configuration and display conflicts return 409 with the current revision. Reload and deliberately reconcile rather than replaying a stale edit. Single-profile edits and secrets retain last-writer-wins behavior. Style-package apply and restore require the current profile revision, independent of the configuration revision. Single-profile style edits create a new local snapshot and advance that profile revision, so a pending package apply cannot silently overwrite them.

The optional scheduler runs bounded, explicitly authorized local responsibilities; it is not a recurring job planner or fleet dispatcher. The optional installation update API manages locally enrolled components; it confers no remote-machine provisioning or fleet authority. Runtime installation provisions only this existing instance. The CLI `api METHOD RESOURCE --file body.json` exposes every JSON resource without requiring a dedicated subcommand. ACP, embedding, and administrator access do not independently grant access to other machines.

## Public connection help

`/welcome`, `/welcome/info`, `/welcome/root.crt` and `/welcome/check` are outside the authenticated management API. The optional HTTP welcome listener serves only these help assets and public certificate information; it has no login, management, or ACP routes. All sign-in and agent operations use HTTPS. Custom operator-supplied TLS certificates are not replaced and do not offer the unrelated managed CA for download.

Managed TLS uses a per-instance root CA with a separately signed one-year server certificate. Startup renews the leaf when fewer than 30 days remain or interface addresses change, keeping the CA stable. The pre-0.3 generated leaf/key are privately backed up during migration. CA replacement near its ten-year expiry requires reinstalling trust. No private key is downloadable. The UI shows the CA fingerprint; compare it with trusted installer output before importing a certificate downloaded over HTTP.

`window.agentInstanceWelcome` exposes `getState`, `selectPlatform`, `check`, `openSecure`, `downloadCertificate`, and `showDetails` for browser automation of the public guide. The probe has no credentials and allows cross-origin reads only from this instance's advertised HTTP welcome origins. Success means the browser accepted the connection, including any previously accepted exception; failure cannot distinguish certificate, network, and browser restrictions.


## Portable persona profiles

Persona Workshop produces `twin-profile/1` packages (the format identifier remains compatible with existing exports). Give its management connector an expiring token with `observe` and `profiles` scopes. The connector is optional: the same installation is available through the instance's **Context & profiles → Import profile package**, HTTP API, or CLI:

```sh
agent-instance api GET profile-package --home /path/to/instance
agent-instance api POST profile-package --file apply-profile.json --home /path/to/instance
agent-instance api POST profile-package/restore --file restore-profile.json --home /path/to/instance
```

The apply body contains `expected_revision` from GET and `package` containing the complete export. The restore body contains `expected_revision` and a snapshot `id` from `previous`. The installation counter is distinct from the package's source revision UUID. A stale counter returns 409; read status and deliberately review before retrying. Each history entry includes local snapshot ID, source subject/version (if imported), creation time, kind, and the two content digests. GET returns metadata only, never the private profile text.

Imports validate the format, required context, supported instruction format, UUIDs, and all file digests. Only `communication.md` and `working-style.md` become runtime context. Identity, raw evidence, credentials, backend configuration, and Slack connections are not imported or restored. Package metadata remains out of prompts and event bodies. Changing a style can influence agent choices within its existing permissions; this scope deliberately does not grant permission to change those permissions.

Both files are stored under `profiles/versions/<snapshot-id>/`. An atomic `profiles/current.json` pointer activates the complete pair. The first managed change preserves existing loose style files as a baseline for rollback; those original files then cease to be authoritative. Use the management API or CLI for subsequent edits. Versions are retained locally; the status API lists up to 50 previous snapshots. Restoring changes the active pair and increments the installation counter without changing the immutable snapshot.

Each new prompt reads one complete version. In-flight work retains its supplied context, and previous messages still exist in backend history. Session observations record `last_profile_package` and `last_profile_sha256`; these prove context was supplied, not that a model followed it. No dependency on Persona Workshop is needed once a profile is installed. Profile updates are explicit; editing or deleting a source draft does not reach already installed copies.

## Responsibilities and profile delivery

Optional capability IDs `agent-instance.organization/1`, `agent-instance.work/1`, `agent-instance.delegation/1` and `agent-instance.profile-use/1` are advertised by `descriptor`. Their versioned paths and read/mutation scopes are listed in `operations` and OpenAPI. See [the complete contract](responsibilities.md) for JSON shapes, transitions, audience enforcement, delegation provenance/deduplication and profile-use evidence. ACP-only integrations do not require these resources.

## Installation-owned tools

Optional [HTTP MCP tool connections](tool-connections.md) are configured through the same revision-checked `config` API/CLI and schema as other agent settings. Their credentials use the existing write-only secret API. Clients receive these connections only with an explicit `tools:use` token scope; ordinary ACP clients continue to work without them.

## Installation updates

See [Updates](updates.md) for the optional `constellation.installation-updates/1` management capability, policies, commands, host helper, and agent tool.
