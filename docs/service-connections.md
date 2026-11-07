# Saved service connections v1

The Instance Connections page manages saved service connections independently of
live ACP clients. The installation's existing host helper serializes connection
operations with enrolled updates. It is not a central provisioning service.
Domain connectors own setup, credentials, sessions, tasks, service lifecycle and
recovery. No browser or agent supplies host commands, executable paths or shell.

## Enrollment

Owner-managed `registration.json` has an optional `service_connections` object:

```json
{
  "schema": 1,
  "services": [{
    "id": "minecraft", "label": "Minecraft",
    "setup_url": "https://installation.example/minecraft/setup"
  }],
  "connections": [{
    "id": "minecraft-workshop", "service": "minecraft", "label": "Workshop",
    "revision": "owner-config-digest",
    "context_provider_ids": ["minecraft:jarvis:workshop"],
    "commands": {
      "status": ["/fixed/python", "/fixed/connection.py", "status"],
      "connect": ["/fixed/python", "/fixed/connection.py", "connect"],
      "disconnect": ["/fixed/python", "/fixed/connection.py", "disconnect"]
    }
  }]
}
```

A service exposes either a usable `setup_url`, a typed Add provider, or both.
HTTPS is required except numeric loopback HTTP for local setup. Credentials must
not appear in URLs. Each context provider belongs to at most one connection.
There are at most 32 services, 64 connections and 16 provider IDs per connection.
Registration changes retain other component IDs, participants and policies.
Fixed owner command files live outside agent-writable/container-mounted paths.

## Connect and Disconnect

A fixed command receives JSON on stdin, never interpolated into argv:

```json
{"schema":1,"connection_id":"minecraft-workshop","revision":"owner-config-digest","action":"disconnect","request_id":"UUID"}
```

Each invocation is bounded to 30 seconds and a single JSON response of at most
64 KiB. Commands return:

```json
{"schema":1,"connection_id":"minecraft-workshop","state":"disconnected","request_id":"UUID","settled":true,"context_ready":false,"failed":false,"message":"Disconnected; saved session retained."}
```

States are `connected`, `disconnected`, `busy`, `unknown`. Optional
`context_ready` and `failed` default to false. Periodic status has null request_id.
A matching request ID plus settled=true and the requested terminal state proves
success. Acceptance, a healthy process or transport closure does not.

Before effects the domain durably records exact request ID, action and revision.
Changed reuse rejects. Status may advance phases of that SAME recorded operation,
never allocate a new operation or repeat settled effects. The host journals before
dispatch. Following crash, timeout or lost reply it calls status only. Unknown
operations block overlapping updates; they are never silently retried or cleared.

A known settled failure may return failed=true, settled=true and a known actual
state (connected/disconnected) after confirming no unsettled effects. This closes
the request as failed, leaves context gated and permits a new explicit request.
Busy/unknown cannot be represented as settled failure.

Disconnect preserves configuration, credentials, identity, sessions, tasks and
unknown receipts. Connect grants no new work authority and does not revive expired
work. Domain admission/lifecycle locking must serialize with the connector's
update participant. A disconnected connector must defer an update that requires
unavailable saved-session verification; it must not temporarily join merely to
update, claim an unperformed verification or globally disable scheduling.

## Context and operation ordering

The browser queues an action; it does not execute it. A queued action must not
invalidate an updater already verifying a saved session. After active update
transactions finish, the host sends a matching busy/context_ready=false report,
gets its acknowledgement, then journals and dispatches the fixed command. This
closes that connection's provider gates before domain effects.

Connect may return context_ready=true with the matching request ID while busy,
after its context/MCP endpoint is reachable under closed controller/effect
admission. Instance then opens only its mapped context gates for original-session
verification. This does not settle Connect or grant effect authority. Final
success still requires matching connected+settled. Unknown/no-ready observations
close the gate again. The host persists reports in an outbox and acknowledges them
before another domain status phase, including after restart/lost HTTP reply.

Context Provider has optional enabled:boolean, default true for compatibility.
The effective permission is provider.enabled AND its connection gate. Disabled
sources are omitted from owner context listing and rejected for grant preparation,
orientation and queries, including existing grants. Deactivation preserves
credentials, entries and historical bindings. Already delivered context cannot be
retracted; an in-flight query may finish, subsequent queries are denied. Context
activation does not prove a connector is online.

## Typed Add for CLI-only services

Instead of setup_url a service can enroll a bounded form and fixed Add commands:

```json
{
  "id":"minecraft","label":"Minecraft",
  "add":{"revision":"template-digest","fields":[
    {"name":"label","label":"Connection name","kind":"text","required":true,"max_length":120},
    {"name":"server","label":"Server","kind":"choice","required":true,
     "choices":[{"value":"workshop","label":"Constellation Workshop"}]}
  ]},
  "add_commands":{"add":["/fixed/python","/fixed/setup.py","add"],
                  "status":["/fixed/python","/fixed/setup.py","status"]}
}
```

At most 16 fields: text with max_length 1..1000, integer with explicit min/max,
boolean, or choice with at most 32 value/label options. Names are alphanumeric or
underscore, at most 64 characters. Credential field names/types are not supported;
platform authorization stays in domain setup. Unknown fields reject. The domain
independently validates all inputs against its approved template, including
allowed network endpoints, routes and identity/no-overwrite rules.

Add command stdin (status uses the same values/revision/ID and action=status):

```json
{"schema":1,"service_id":"minecraft","revision":"template-digest","action":"add","request_id":"UUID","values":{"label":"Workshop","server":"workshop"}}
```

Successful result has schema=1, service_id, matching request_id, state="saved",
settled=true, message, and `connection`: the complete host enrollment descriptor
shown above, including fixed commands. The helper validates and atomically appends
ONLY this connection under its worker lock. An existing identical ID/descriptor is
idempotent; a conflicting ID rejects. The domain does not separately rewrite host
registration. Other enrolled components/policies remain unchanged. Updater
participant/component enrollment is a separate owner operation, never implied by
Add. Add alone grants no work and does not connect.

Pending results use busy/unknown, settled=false, omit connection. Known settled
setup failure uses state=failed, settled=true with matching ID and no connection.
An uncertain result is reconciled with status, not reissued as a new Add.

## Instance management API

These endpoints require authenticated admin access (browser CSRF applies):

- GET `/api/v1/service-connections`: public catalog, observed connection states,
  helper freshness, pending request and last completed result; no commands/secrets.
- POST same path: `{connection_id, revision, action:connect|disconnect, request_id}`
  or `{service_id, revision, action:add, request_id, values}`. Request IDs are UUIDs.
  Changed reuse, stale revision, offline helper and another pending operation reject.
- POST `/api/v1/service-connections/report`: host-only-by-deployment authenticated
  report `{schema:1,services,connections,add_result?}`. Services exclude commands;
  connection observations include public descriptor fields plus state, request_id,
  settled, context_ready, failed, message. Add result has service_id, request_id,
  state, settled, connection_id (on success), message. Add settles only when its
  matching saved connection is actually in the report.

Authentication is an installation trust boundary: report uses the existing admin
credential held by the installed helper, not a new remote command capability.
A removed enrollment remains visibly unknown and gated; it does not erase saved
history. Re-enrollment and updater replacement preserve service_connections.

## Verification

Rust tests cover authority, request identity, typed fields, saved registry restart,
context-ready versus terminal settlement, and existing-grant deactivation. Host
fixtures execute fixed subprocess commands, lost-delivery/status-only recovery,
readiness report acknowledgement ordering, and atomic Add enrollment. Browser
fixtures exercise Add/Connect/Disconnect with two saved connections and mobile
layout. None of these fixtures call models, Slack or a live game.
