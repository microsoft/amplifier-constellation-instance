# Optional native engine editor

The host's Model & capabilities card appears for an explicitly configured `agent-runtime` executable. Arbitrary ACP agents keep their own configuration interface. This management feature is not an ACP method or requirement.

The editor discovers the installed engine's schema and calls its versioned local configuration commands. Provider/model/endpoint settings, image/audio input, provider-hosted tools, and named decision services have dedicated controls. Complete configuration opens an editor for every field supported by that engine version, including context compaction, execution limits, permissions and account settings. Provider defaults come from the installed engine, not a duplicated host catalog. Unsupported combinations fail validation without a silent fallback.

When the engine publishes a model catalog, the model field offers its current
supported classes. **Use provider defaults** stages the engine's current default
model and connection settings; it does not save, change authentication routes or
rewrite existing sessions. Gateways and custom deployments without a published
default keep their owner-selected model name. Loading settings never migrates a
model automatically.

An engine that reports configuration validation separately from inspection can
keep retired settings visible and editable. The editor shows the validation
problem while retaining the original file and revision. The owner can select a
supported model, validate and explicitly save. Invalid configurations remain
ineligible for execution; inspection is not execution approval.

- `GET /api/v1/native-config`: config, content revision, engine capabilities, host backend fingerprint and named-secret mapping.
- `GET /api/v1/native-config/schema`: the installed engine's JSON Schema and UI hints.
- `POST /api/v1/native-config/validate`: validate without changing files.
- `PUT /api/v1/native-config`: atomically save through the engine's compare-and-swap configuration command.

Validation and save accept `{expected_revision,expected_backend_sha256,config}`. These endpoints require `admin`; `observe` does not execute configuration commands. The local CLI uses the same paths via `agent-instance api`. UI operations are `native.load`, `native.preset`, `native.details`, `native.validate`, `native.save`, `native.raw.open`, and `native.raw.save`. Drafts and disclosures use the existing revisioned view state.

The host invokes only the configured engine executable with its explicit `--home`, argument arrays, a clean environment, 15-second deadline and bounded output. It supplies no provider key values or owner account directory to these inspection/validation commands. Candidate files are private and removed after the operation. Credentials remain named references managed through Access & credentials and the host backend's secret environment mapping.

Saving leaves existing ACP connections running with their existing configuration. Clients reconnect to adopt new settings; provider/model changes may require a new session. The UI does not automatically interrupt work or restart clients. Changes are refused while background runs are pending or active. Native `runtime.json` contents are also included in scheduler approval fingerprints, so a configuration change invalidates earlier execution approval and requires an operator to approve the revised setup.

A successful validation establishes configuration correctness, not provider access, model entitlement, tool quality, or authentication success. Engines older than the configuration CLI contract report an unsupported operation and must be updated before using this editor.


## Native ChatGPT account

The separate ChatGPT account card uses the native engine's authorization flow. It
never reads or copies a Codex account cache. Completing sign-in leaves the current
provider configuration unchanged. **Use this account** intersects the account's
models with the installed runtime's support catalog, preserving the selected
model when available or staging the supported runtime default. **Save engine settings** applies
that explicit choice. Existing sessions and background approvals retain the
configuration rules above.

An empty intersection shows a repair message instead of selecting an unsupported
model. A malformed runtime support catalog fails closed. Engines that do not
publish a catalog retain their earlier account-list behavior; the engine's own
configuration validation remains authoritative in either case.

A browser on the agent machine can open the sign-in link directly. For a browser
on another machine, the card first shows a loopback-only SSH forwarding command
for the actual callback port. Run that command on the browser's computer, leave
it running, then open sign-in and give consent. SSH access must already work. The
service does not expose an OAuth callback on its LAN interface or pretend a remote
browser can reach the agent's localhost automatically. A busy port requires a new
attempt or an explicitly selected free callback port.

All account routes require administration:

| Route | Method | Body/result |
| --- | --- | --- |
| `native-account` | GET | Public account status, configured credential-file path, and current sign-in job. No credential values. |
| `native-account/login` | POST | `{expected_backend_sha256,callback_port:0}`. Start one asynchronous job; repeated requests return the pending job. Port 0 chooses an available port. |
| `native-account/login` | GET | `starting`, `pending`, `connected`, `cancelling`, `cancelled`, `expired`, `interrupted` or `failed`. Pending includes the ephemeral authorization link, loopback callback, deadline and optional SSH command. |
| `native-account/login` | DELETE | `{job_id}`. Cancel that exact attempt; stale IDs fail instead of cancelling a newer attempt. |
| `native-account/models` | POST | Fetch account model identifiers/display names intersected with the installed engine's support catalog when available; includes `runtime_catalog_available`. This may refresh account authorization but does not make an inference request. |
| `native-account` | DELETE | `{confirm:true}`. Revoke saved account access through the native engine. Engine configuration/history and account binding remain. |

The job owns a bounded subprocess independently of an HTTP request or browser.
It allows 15 seconds to produce the sign-in address and at most five minutes to
finish. Cancellation, expiry and managed service shutdown terminate and reap the
callback process. Linux also ties it to the parent process lifetime; Windows uses
a kill-on-close job. After a service restart the saved job becomes interrupted
and is never replayed automatically. Check account status before retrying, since
consent may have completed immediately before an interruption. The account's
binding and lock remain owned by the engine.

Only job metadata is durable; the authorization URL/state stays in authenticated
transient responses, never in view records or activity events. Account fields are
explicitly filtered, and provider credentials are not inherited by the sign-in
subprocess. Engine configuration changes are refused during a pending sign-in.

CLI equivalents use the same authenticated service endpoints:

```sh
agent-instance --home /path/to/instance native-account status
agent-instance --home /path/to/instance native-account sign-in --callback-port 0
agent-instance --home /path/to/instance native-account login-status
agent-instance --home /path/to/instance native-account cancel --job-id JOB_ID
agent-instance --home /path/to/instance native-account models
agent-instance --home /path/to/instance native-account sign-out --confirm
```

The UI exposes `native.account.load`, `.connect`, `.cancel`, `.models`, `.use`,
`.open`, `.tunnel.copy`, `.tunnel.ready`, `.disconnect.open` and `.disconnect`
through `agentInstanceControl.perform`. `account.location` (`local` or `remote`)
and optional `account.callback_port` are regular view drafts;
`native_tunnel_ready` is an explicit view filter. No view command grants account
access or supplies the user's consent. Real OpenAI consent and account model
availability must still be verified with the account owner.
