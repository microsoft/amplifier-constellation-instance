# Durable background work

The optional `agent-instance.scheduler/1` management capability owns execution inside **one agent instance**. It is disabled by default. The service runs its ordinary configured ACP backend through an internal client; closing the browser, ending an HTTP request, or disconnecting a conversation does not stop an authorized run. There is no fleet dependency, shared peer database, or automatic external delivery.

## Authorizing work

1. Configure a backend whose built-in tools are restricted to the local workspace work you intend to authorize. The host offers no client filesystem/terminal tools, no conversation-provider grants, and no interactive permission approvals. It cancels permission requests and unknown client callbacks. **ACP itself is not a tool sandbox**: the backend must enforce its own local-tool and network policy. Do not enable background work for an unrestricted engine or one with automatically authorized external connectors.
2. Read `GET /api/v1/scheduler`. Save `PUT /api/v1/scheduler` with `expected_revision`, `enabled`, `paused`, `concurrency` (1–4), optional `session_mode` (null keeps the engine default), and `approved_backend_sha256` equal to the returned current configuration fingerprint. The hash covers backend configuration and, for recognized native Agent Runtime installations, the bounded `runtime.json` contents. It does not cover binary contents or arbitrary external files. Use versioned installation paths when changing engines.
3. Record and accept a responsibility through the work API or UI. Acceptance by a conversational agent is only a ledger transition; it cannot authorize execution.
4. Explicitly queue a run. The authorizing management credential is recorded separately from the agent's persona identity. Authorization persists until cancelled or invalidated; closing/signing out of the authorizing browser does not revoke it.

The execution endpoints require `work:execute` for mutations. `work` and `work:read` do not imply execute permission. Reads accept `work:read`, `work`, or `work:execute`; full reports are not exposed through the general `observe` scope. The read-only `work_execution` agent tool applies the current conversation's existing audience boundary. It cannot queue, reconcile, or retry a run.

```json
{
  "type": "queue",
  "expected_work_revision": 2,
  "request_id": "local-check-2026-11-01-1",
  "budget": {
    "max_seconds": 300,
    "max_turns": 4,
    "max_output_bytes": 131072,
    "max_tool_calls": 30
  }
}
```

Send that body to `POST /api/v1/work/{id}/execution`. The same request ID, body, work ID and authority return the existing run; changing content under the same ID is rejected. Only accepted/in-progress work owned by this instance is eligible. Only one pending/active run per responsibility is allowed. The queue holds at most 64 pending/active runs.

An explicit `session_mode`, such as `workspace` or `read_only`, is captured when each run is authorized. The scheduler checks that the new session advertises that exact mode ID and successfully calls standard `session/set_mode` **before** sending the work prompt. An absent/unsupported mode or rejected mode change fails before the prompt; it does not fall back to a different mode. Null does not send a mode change. Changes to this setting affect future authorizations only; queued and active runs retain their captured mode. An unexpected current-mode notification after selection interrupts the run. Interactive sessions and the engine’s own defaults remain unchanged. Permission callbacks are still cancelled in every mode.

A new run creates an ACP session and authorizes **1–12 turns** using `budget.max_turns` (omitted means one for compatibility). All turns reuse that same session. An explicit resume can load a saved session as described below. Time is limited to 1–3600 seconds including handshake, returned text to 1 KiB–1 MiB, and observed unique tool calls to 0–100. These limits cancel further execution; a backend may already have performed a tool action before its observation arrives. They are not hard model-token, cost, filesystem, or network quotas. Cancellation sends the ACP cancellation notification, then closes the connection and terminates its process group/job with a short shutdown grace period. It does not undo side effects.

## Pause, cancel, and recovery

`paused: true` prevents new starts while active runs finish. `enabled: false` also interrupts active runs. Stopping the agent runtime interrupts them too. Changing the work's goal/role/audience/source or changing the approved backend invalidates the captured authorization. Pending jobs fail before their prompt; active jobs stop and require inspection if execution may have occurred.

A durable `prompt_may_have_run` marker is stored before prompt delivery. After restart, any interrupted marked run becomes `needs_reconciliation`; it is never automatically replayed. A job interrupted before this marker becomes `failed_before_prompt`, also requiring an explicit retry. Queued jobs remain queued and can start under their still-valid authorization. The service performs recovery only after acquiring its exclusive installation lock; CLI reads never recover or execute jobs.

Commands at the same execution endpoint:

```json
{"type":"cancel","run_id":"RUN_UUID","expected_revision":4,"reason":"Stop this attempt"}
```

```json
{"type":"reconcile","run_id":"RUN_UUID","expected_revision":6,"note":"Inspected the workspace and partial report. The artifact is incomplete; another attempt can continue safely.","allow_retry":true}
```

```json
{"type":"retry","run_id":"RUN_UUID","expected_revision":7,"expected_work_revision":3,"request_id":"local-check-2","reason":"Continue after inspecting the interrupted attempt","budget":{"max_seconds":300,"max_output_bytes":131072,"max_tool_calls":30}}
```

Reconciliation records the operator's inspection, not independent proof or completion. `allow_retry` permits a **separate** explicit retry command; it never starts one. A retry captures the current responsibility and uses a fresh session, run ID and budget. It includes the earlier report as attributed source data, without carrying forward execution authority. Its link to the earlier attempt is retained. Reopen/start the responsibility first if it is currently completed/cancelled/reviewed.

## Continuation and saved sessions

Each model turn is asked to finish with a final `AGENT_WORK_STATUS` line followed by `{"status":"continue"|"blocked"|"review","summary":"..."}`. This small text convention works with ordinary ACP agents; it grants no additional tools or authority. `continue` spends another turn from the same already-authorized run, `blocked` stops for input, and `review` stops for independent artifact review. Missing/malformed status in a multi-turn run stops as blocked. A legacy single-turn report remains reviewable without this line.

Time, output bytes and observed tool calls apply cumulatively across the entire run. Tool IDs reused in separate turns count again. Each completed turn records its number, report byte range, timestamp, disposition and progress summary. Reaching the turn limit after `continue` produces `paused_limit` and leaves the responsibility in progress; it does not imply completion or readiness for review. Reported blockers produce `blocked` work with the agent's stated reason.

Cleanly finished `reported`, `blocked`, or `paused_limit` runs may be explicitly resumed:

```json
{"type":"resume","run_id":"RUN_UUID","expected_revision":8,"expected_work_revision":4,"request_id":"next-local-steps","reason":"Continue the next two steps after supplying the missing input","budget":{"max_seconds":300,"max_turns":2,"max_output_bytes":131072,"max_tool_calls":30}}
```

The responsibility must first be accepted/in-progress. A resume is a new authorization and run record, linked to the previous run, with the same execution-chain principal and saved ACP session ID. The backend must advertise `loadSession`; standard `session/load` is used and the explicitly authorized mode is reapplied. Replayed history is not appended to the new execution report or charged as new tool/output usage. Backend/configuration changes disallow saved-session resume. A failed or interrupted attempt never resumes automatically: reconcile it, then explicitly authorize a fresh retry. Restart recovery remains conservative even if some completed turns were persisted.

## Outcomes and evidence

`GET /api/v1/work/{id}/execution` returns the most recent 100 runs, reports, source/audience, authorization, budget, captured `session_mode`, `mode_applied`, observed usage, report SHA-256, session ID and reconciliation. The management dashboard summarizes the most recent 100 runs and lifetime state counts. Reports can contain private work content; the general activity stream contains metadata only.

A normal `end_turn` with returned report text and a `review` disposition moves unchanged in-progress work to `review` and adds an execution-report reference. Its trust is `agent_report_unverified`. It does not independently inspect artifacts, verify test assertions, or mark the work completed. Actual artifact evaluation and explicit evidence review remain separate work commands. An empty response, error, denied approval, exhausted budget, cancellation, interruption or stale goal never produces a completion claim.

No result is posted to Slack, Teams or other experiences. The existing owner/conversation context tool can inspect the report within its audience; delivery requires a separately authorized connector action.

## CLI and view control

```sh
agent-instance --home ./my-agent background status
agent-instance --home ./my-agent background configure --file scheduler-settings.json
agent-instance --home ./my-agent background runs WORK_UUID
agent-instance --home ./my-agent background command WORK_UUID --file execution-command.json
```

The equivalent generic `api GET/PUT/POST ... --file` commands remain supported. The descriptor advertises exact paths/scopes and OpenAPI provides settings and command schemas.

Responsibilities → Background work exposes settings, authorization budgets, cancellation, inspection, retry and report reading. The same actions are available through `window.agentInstanceControl.perform`: `scheduler.open/save/advanced`, `execution.open/save/report/limits`. `execution.open` accepts `{id: work_id, command: "queue"|"cancel"|"reconcile"|"retry"|"resume", run: run_id?}`. Non-secret fields are durable view drafts under `scheduler.*` and `execution.*`; selected work/run, modal, report disclosure, advanced-mode disclosure, and budget disclosure are view filters/state. Remote view control changes the view; execution still calls the authenticated management API.

## Validation boundary

The automated suite uses a standard ACP subprocess fixture that creates actual temporary workspace files. It verifies background ownership, persistence, scoped inspection, authorization, idempotency, cancellation, stale goals/backend approval, output/tool/time limits, restart uncertainty, explicit mode negotiation before prompts, and immutable per-run mode authorization. These are protocol/process tests, not evidence of model judgment or task quality. A native runtime with a real model can attach through the same configured-backend contract without changing this scheduler.
