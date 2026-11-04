# Bundled single-agent interface

The interface is a dependency-free HTML/CSS/JavaScript client, embedded in the service release. It manages one installation. It neither discovers a fleet nor configures Slack/Teams adapters.

## Entry points and embedding

`index.html`, `app.css`, and `app.js` ship together. Network asset and API URLs are relative to the page directory; the favicon is embedded data. Serve the entry URL with a trailing slash. The same bundle can therefore run at `/` or under an authenticated proxy prefix such as `/instances/alpha/`. A proxy must forward requests under that prefix to the selected service and maintain the service's authenticated-origin checks.

The browser uses the owner's secure HttpOnly session cookie and the CSRF value returned by `api/v1/auth/session` or `auth/login`. Every mutation includes `X-CSRF-Token`. JavaScript, CSS, and fonts require no third-party network requests. The bundle works with `script-src 'self'` and `style-src 'self'`; no inline script/style execution is required.

The optional iframe message contract is `agent-instance.ui/1`. The receiver requires an authenticated page, an exact origin in `config.server.embed_origins`, and `event.source === window.parent`. It never uses a wildcard reply origin. Allowed request types are `get_state`, `actions`, and `command`, each with a caller-supplied string `id`. Replies contain the same protocol/id and either `{ok: true, result}` or `{ok: false, error}`. This handshake grants no additional permissions and accepts only view commands. Domain operations use the management API.

Prefer a same-origin prefix proxy for embedding. Merely allowing an iframe origin does not solve cross-site cookie restrictions or authenticate a parent. The initial bundle does not implement an independent cross-site login/delegation handshake.

## Screens and domain operations

| Screen | Operations |
| --- | --- |
| Sign in | Owner username/password through PAM login; password is transient; public browser certificate help |
| Setup | Identity → Ready; installed runtime/account summary, account connection when needed, save and start; optional Advanced settings |
| Overview | Runtime state, connection/session counts, uptime, saved revision, ACP connection instructions, active address/certificate and desired listener |
| Configuration | Name, purpose, working guidance and fixed installation metadata; runtime/account summary; optional Advanced settings for executable, JSON arguments/environment/secret references, working directory/roots, limits and service settings |
| Context & profiles | Import a portable Persona Workshop package, inspect the installed version, restore a previous version; edit `communication`, `working-style`, and separate `identity` text |
| Connections | ACP entrypoint, attached clients, reported sessions, selected session details |
| Activity | Recent work, outcomes and problems from the server-filtered activity API; category/search/problems filtering; opt-in diagnostics; earlier activity paging and detail inspection |
| Access & credentials | Named write-only secrets, scoped expiring tokens, current view inspection |

Instance ID, schema version, and installation owner are displayed as fixed metadata. `setup_complete` is changed by the setup workflow and `enabled` by lifecycle actions. The UI does not pretend those are free-form persona fields.

Runtime enable/stop keeps the management service available. It does not equate an enabled runtime with a model currently doing work. Listener settings show both desired configuration and the actual advertised listener; `restart_required` is displayed when they differ. The UI does not silently restart the host service.

Configuration edits preserve the expected revision. Start/stop also updates configuration, so the browser reloads the document after its own lifecycle actions. If another client changes configuration while this browser has edits, a notice offers **Load latest settings**. This updates the saved baseline while preserving explicit field drafts. It does not silently replace the user's pending input.

## Persona profile packages

**Context & profiles** shows the installed profile and an **Import profile package** action. A package downloaded from Persona Workshop supplies `communication.md` and `working-style.md` together. The portable schema remains `twin-profile/1` for compatibility; `.persona.json` and earlier `.twin.json` downloads can both be selected. The separate `identity` profile is not part of this operation.

The browser reads `GET api/v1/profile-package` for the active snapshot, history and numeric revision. Applying calls `POST api/v1/profile-package` with `{expected_revision, package}`. **Version history** reveals previous snapshots; **Restore** calls `POST api/v1/profile-package/restore` with `{expected_revision, id}`. These domain endpoints require `profiles` or `admin` scope. The token creation dialog exposes the narrower **Install and restore profiles** permission.

Profile updates apply to subsequent messages. Work already in progress retains its supplied context; earlier conversation history remains unchanged. The UI labels imported source versions separately from local edits and the baseline before an import. It does not claim that a version was followed by a model merely because it was installed.

The browser preserves pending profile edits when refreshing. An import or restore is blocked until pending communication/working-style edits are saved or discarded; identity drafts can remain open. A stale package revision returns a conflict and reloads the current version for review. It never retries a profile mutation automatically. Individual style saves create local versions through the existing profile API.

Uploaded packages and their evidence metadata stay transient in the current browser until submitted directly to the profile-package endpoint. They are never stored in view drafts, acknowledgements, event payloads, URLs or session storage. Closing the import dialog clears its transient selection. A restored dialog on another browser or after reload asks for the file again. Agents can submit packages directly to the management API without a file picker.

Backend arguments and roots are JSON arrays of strings; environment and secret references are JSON objects with string values. The default working directory is `${AGENT_HOME}/workspace`. Empty optional URL/certificate/key fields become `null`. Empty backend command means no configured backend. The normal installer provisions the included runtime. When that installation is missing, Ready displays **Install runtime** rather than requiring a raw executable path. A custom ACP runtime remains available under Advanced settings. Completing setup requires a configured runtime; the included Codex runtime also requires its local account credentials to be ready. This is not presented as a completed model-access probe.

## Runtime readiness and account connection

`GET api/v1/backend` supplies `kind`, `configured`, `installed`, `name`, versions, `auth_ready`, `auth_validated`, `account_source`, `status`, `problem`, `help`, and advertised `actions`. The summary distinguishes an installed engine, a configured custom runtime, a missing installation, and an account that still needs connecting. A copied owner account is described as an existing account in this agent's private store. Installation owner and instance ID are fixed display metadata, separate from the agent's editable name and purpose.

The **Install runtime** action calls `POST backend/install`, then reloads the saved config and readiness. **Connect ChatGPT** calls `POST backend/account/login`; a pending response displays the official device sign-in page and one-time code. The browser polls `GET backend/account/login`, supports `DELETE` to cancel, and refreshes readiness on completion. The device URL is restricted to `https://auth.openai.com/codex/device`. Device codes and account progress live only in authenticated, transient browser memory; they are absent from view state and observations. Other agents can drive the same domain endpoints.

The login and access screens link to relative `welcome`. This public guide can be opened before sign-in and provides certificate download/trust steps. When advertised, `addresses.welcome_url` is shown as the setup guide for another device; authentication remains on HTTPS. The optional HTTP help listener can be configured under Advanced settings through `server.welcome_port` (`0` chooses a port, empty becomes `null` to disable). The built-in certificate-help link is relative and preserves the proxy prefix; the optional advertised setup URL is a direct host address.

## Activity projection

The normal activity view calls:

```text
GET api/v1/activity?limit=100&diagnostics=false&category=all&search=
```

Responses are newest first: `{items:[{seq,at,kind,data,summary,category,severity}],next_before,has_more}`. The browser displays the server's human-readable summary, time and category. It requests an older page using `before=next_before`. Search and categories are applied by the service before limiting results. Categories are `all`, `work`, `outcomes`, `problems`, `connections`, `configuration`, `access`, `runtime`, and opt-in `diagnostics`. **Only problems** sends `category=problems`; it is a view choice, not an extra API parameter.

The overview independently requests five recent entries with diagnostics disabled. Its useful-work window does not depend on the browser's raw event buffer or the Activity screen's filters. This prevents frequent `view.changed`/`view.ack` events from hiding work. The existing raw `/events` and SSE APIs retain their observation/control roles. Detail inspection can show safe structured metadata for a selected activity item; normal rows do not display protocol event names unless diagnostics is enabled.

## View state and the three-foot experience

Each browser tab registers a separate view using `POST api/v1/views`, retaining its ID in tab-local session storage. The server is the authority for the view's `revision` and `state`.

The supported state is:

```json
{
  "route": "setup|dashboard|configuration|profiles|connections|activity|access",
  "step": 0,
  "selected_session": null,
  "filters": {"advanced": false, "search": "", "category": "all", "problems_only": false, "diagnostics": false, "profile_package_history": false},
  "draft": {},
  "focus": null,
  "scroll": {"x": 0, "y": 0},
  "modal": null
}
```

Setup steps are 0 (Identity) and 1 (Ready). Old persisted step 2/3 positions migrate to Ready when the browser reconnects. During unfinished setup, an installed default runtime also clears an old runtime draft only when every field is empty or a known neutral default. Name/purpose drafts and meaningful custom runtime edits are preserved; this migration does not run while the user is editing. The Advanced disclosure uses `filters.advanced` on both setup and configuration. Activity filters use `search`, `category`, `problems_only`, and `diagnostics`; remote changes trigger the same server queries as local controls. Profile history uses the boolean `profile_package_history` filter. Supported modal IDs are `profile-package`, `secret`, `token`, `token-created`, `view`, and `event:<sequence>`. A token-created dialog only shows its one-time value in the browser that created it; that value is absent from view state.

View commands are posted to `api/v1/views/{id}/commands` with `{expected_revision, command}`:

| Command | Fields |
| --- | --- |
| `navigate` | `route`, optional `step` |
| `set_draft` | `path`, `value` |
| `focus` | `target` |
| `scroll` | `x`, `y` |
| `select_session` | `session_id`, or `null` to clear |
| `set_filter` | `key`, `value` |
| `open_modal` | `modal` |
| `close_modal` | No additional fields |

Draft paths follow `config.<field.path>`, `profiles.<profile-name>`, and non-secret access fields `access.secret_name`, `access.token_name`, `access.token_hours`, `access.token_scopes.<scope>`. JSON editor fields can contain JSON text or structured JSON values. The browser rejects unknown draft paths from its control helper and routes credential values through their dedicated APIs.

Controls have stable `data-control` names such as `nav.configuration`, `config.backend.command`, `profiles.communication`, `settings.advanced`, `activity.search`, `activity.category`, `activity.problems`, `activity.diagnostics`, and `session.detail`. Runtime/account actions have semantic names `backend.install`, `backend.account.connect`, and `backend.account.cancel`. The read-only certificate and account sign-in links expose stable control names and ordinary URLs. Buttons and form controls expose semantic `data-action` attributes. Clicking, typing, focus changes, scrolling, modal selection, Advanced disclosure and filtering are mirrored into view commands. For example, `set_filter` with `{key:"advanced",value:true}` exposes the technical controls, while `{key:"problems_only",value:true}` shows the problems view. Domain operations do not have to pass through the UI. Typing/scroll are debounced; revisions serialize local mutations and stale updates refresh/retry once.

`view.changed` observation events contain only identifying metadata. The browser fetches its matching full view from the protected view endpoint, applies it, then posts `views/{id}/ack` after rendering. An acknowledgement includes the revision, `applied`/`error`, and whether the document was visible. The client reports an error if a requested focus target is unavailable. An absent browser cannot provide this acknowledgement.

The browser also exposes:

```javascript
window.agentInstanceControl.getState()
window.agentInstanceControl.getDisplay()
window.agentInstanceControl.actions()
window.agentInstanceControl.command({ type: "navigate", route: "connections" })
window.agentInstanceControl.setDraft("config.purpose", "Help the team explore ideas.")
window.agentInstanceControl.perform("config.save")
```

`actions()` reports currently rendered semantic actions and disabled state. `perform()` invokes the same handlers used by buttons, including configuration save/reload, profile save, lifecycle changes, secret/token operations, and clipboard helpers. External agents normally use the documented HTTP operations rather than injecting JavaScript. Clipboard access is still subject to browser permission/user-gesture policy; connection data and token creation results are available programmatically from their own APIs.

Profile package controls have stable names `profile.package.status`, `profile.package.open`, `profile.package.file`, `profile.package.preview`, `profile.package.apply`, `profile.package.history`, and `profile.package.versions`. Semantic actions include `profile.package.open`, `profile.package.prepare`, `profile.package.import`, `profile.package.restore`, `profile.package.history`, and `profile.discard`. `prepare` accepts `{package, expected_revision?}` and shows the import preview; `import` accepts the same payload and also works directly without opening the dialog. `restore` accepts `{id, expected_revision?}`. The package itself is an argument to the domain action, not a view draft. For example:

```javascript
await window.agentInstanceControl.perform("profile.package.prepare", {
  package: exportedPackage,
  expected_revision: currentProfileRevision
});
await window.agentInstanceControl.perform("profile.package.import");
```

Remote controllers can expose version history using `set_filter` with `{key:"profile_package_history",value:true}`, or open an empty import dialog using `open_modal` with `{modal:"profile-package"}`. Installing through the HTTP API and then focusing `profile.package.status` offers the same user-visible result without passing package content through the view channel.

Domain action parity does not require an agent to click the UI. For example, save through `PUT config`, issue a token through `POST tokens`, or save profile text through `PUT profiles/communication`, then use view commands to show the result on the user's display. Configuration drafts do not automatically execute domain actions.

## Secrets and content boundaries

Passwords, secret input values, device sign-in codes and one-time tokens are never placed in view drafts, session storage, URLs, or view acknowledgements. They exist transiently in browser memory for their immediate operation. Secret values are sent only to their write-only endpoint. Editing ordinary environment values is intentionally labeled **Non-secret environment**; sensitive values belong in stored secrets referenced by name.

Profiles and normal editor drafts are user content. They remain in protected view/configuration/profile resources; the UI does not copy them into observation events. UI view grants and observation grants are separate. The optional embedding bridge returns only the permitted view contract and accepts no generic JavaScript execution.

## Browser validation

The UI was exercised in Chromium using two distinct environments:

- A prefixed HTTP protocol fixture verifies the two-step setup without manual runtime fields, explicit account readiness/device sign-in, old-step migration, local/remote Advanced controls, human-readable activity despite 1,200 newer UI events, category/search/problems/diagnostics filters and pagination. It also covers relative assets/API routing, remote SSE navigation/drafts/session selection, profile save, write-only secret input, one-time token display, credential/device-code-free view telemetry, acknowledgements, untrusted embedding rejection, and 390px mobile layout. This checks the UI contract; provider login and PAM responses are synthetic.
- A fresh temporary installation of the real Rust service verifies authenticated setup and lifecycle/configuration revision handling, remote view commands over the actual API/SSE, acknowledgements, profile event redaction, explicit configuration reload with pending drafts, desktop/mobile rendering and the service's strict CSP. It also verifies optional remote Advanced controls and Activity filters against the real service. Its browser authentication is a synthetic session in the temporary test database and its preconfigured runtime is a local placeholder; real PAM login, default-runtime provisioning, provider account connection and model execution are validated independently.

No workshop services, agent folders or Slack integrations are used by these UI tests.

## Responsibilities, roles and profile delivery

The `work` route provides creation, selection, lifecycle changes, progress, blockers, evidence and recorded checks. All changes use the versioned work API. The `organization` route offers discipline, assignment and sharing-relationship forms; policy edits use the revision captured when the editor opened. An unrelated refresh does not turn a stale editor into an authorized overwrite. Delegation preparation downloads an explicit envelope; receiving takes that envelope with a selected local relationship and private assignment. Neither action sends a message.

The same semantic actions are callable through `window.agentInstanceControl.perform`: `work.new`, `work.select {id}`, `work.action {id,command,revision}`, `work.save`, `work.import.open`, `organization.edit {kind,id?}`, and `organization.save {kind}`. Forms use revisioned `set_draft` fields under `work.*` and `organization.*`. Work selection and pending command are view filters; dialogs, focus and scroll use the existing view API. Credentials remain excluded. Alternate UIs can call the business APIs directly; browser actions are not required.

Context & profiles shows **Profile in use**, using `/api/v1/profile-use`. It shows **No persona profile configured** when the effective guidance is empty, preserves historical turn details, distinguishes configured guidance from the latest started turn, explicitly shows missing/partial/stale evidence, and never presents behavior quality as a consequence of a matching hash. The backend API also supports selecting a session and freshness window.

Background execution uses the Responsibilities route. `scheduler.open/save/advanced` and `execution.open/save/report/limits` are exposed through the same control API as UI clicks. Settings and budget drafts (`scheduler.*`, `execution.*`), selected work/run, command, modal, expanded report, advanced session-mode settings and other-limits disclosure all live in durable view state. Execution actions call the distinct `work:execute` management endpoints; UI control alone does not confer that authority. See [background-work.md](background-work.md) for command payloads and evidence semantics.

The Memory route provides attributed statements, alternative accounts, source inspection and revision-checked resolution/archive. Its selection, archive disclosure and `memory.*` drafts use the same view-state interface. The optional native engine editor uses `native.*` drafts, modal and disclosure state, while its mutations require the separate administrative API. Both screens expose their actions through `window.agentInstanceControl.perform`; see [memory.md](memory.md) and [native-configuration.md](native-configuration.md) for their action names and API paths.

`navigate` accepts an optional `work_id` when `route` is `work`, allowing an embedding experience to select the responsibility atomically with navigation. This changes view state only. Acceptance forms are also exposed through the semantic controls documented in [completion contracts](completion-contract.md).

## Attention review

Activity → Attention review uses `attention.open`; labels use
`attention.review {id,verdict}`. Modal/view state is remotely controllable.
Configuration → Advanced settings edits the optional `attention` config using
the existing revisioned config API. Secret values remain in the secret store.
See [attention](attention.md) for API payloads and confidence/admission semantics.
