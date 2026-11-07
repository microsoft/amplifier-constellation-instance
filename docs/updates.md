# Installation updates

Open **Updates** in the instance dashboard to check, update, or change preferences.
The overview includes an Updates card. The same controls are available through
`agent-instance --home <home> updates` and the versioned management API.

Checks run hourly by default; choose manual, every 15 minutes, hourly, every six
hours, or daily. Checking does not install anything. Installation defaults to
**When requested**; automatic updates wait for idle work, optionally within a UTC
maintenance window. Stable releases are the default channel. Development selects
the latest successful main-branch CI build, and needs the host owner's GitHub CLI
access to Actions artifacts. Stable public downloads need no GitHub login.

**Allow agent-initiated updates** is one installation-wide switch. When enabled,
the agent may request an update regardless of who asks or where the conversation
happens. There is no owner-DM requirement. An ordinary ACP HTTP MCP connection
carries the `installation_updates` tool alongside the instance's continuity tools.
The switch does not grant arbitrary host commands or access to other installations.

## Installation-owned helper

Each installation has its own owner-run helper. It survives service restarts and,
for a container installation, runs outside the container. The container receives
no Docker socket, updater credentials, or writable helper directory. The agent
queues a fixed operation; the helper resolves enrolled components, sources and
service identities from its private registration. Observatory is an optional
client of these controls, never a required provisioning service.

New installers enroll the helper automatically. For an existing installation,
run on the host:

```sh
agent-instance --home /path/to/agent install-updates
```

To include an independently installed Slack connector, add
`--slack-home /path/to/connector` (repeatable). This registers its installed
executable and service without moving its identity, credentials, or configuration.
Each installation still works independently. The default helper directory is
`~/.local/share/constellation-updates/<instance-id>`; override with `--state`.
Linux uses a user service, macOS a LaunchAgent, and Windows a scheduled task owned
by the signed-in user. The installer requires Python 3.9 or newer.

The private `registration.json` can enroll additional update providers. The built-in
GitHub provider supports the native instance, runtime, Slack and Teams component
repositories when a matching platform artifact is available. A `command` provider
has fixed administrator-selected `status`, `plan`, `apply` and `rollback` argv
arrays. Its plan returns JSON with `available` and `version`. It owns its own
transaction and health verification. Chat cannot add providers or supply argv.
This is an extension point for other software, not an automatic updater for every
program on the computer.

## Recovery and boundaries

The helper validates download hashes from GitHub, selects an exact ordinary
executable from the archive, verifies its identity, and rejects version downgrades.
It records artifact, commit/build or release provenance. This authenticates the
published artifact; it is not a reproducible-build or independent-signature claim.
Stable releases with the same version as an installed development binary are not
silently substituted.

Slack-only updates close admission and drain work only for the changed connectors.
They leave the instance, runtime, other connectors, and their active ACP connections
running. The frozen plan records the service scope; failure and rollback use that
same scope and restore only changed executables. The helper verifies the existing
instance identity without requiring a new process, plus fresh reconnection of the
updated Slack connectors. An unrelated connector's pending work does not block it.

Runtime and instance changes close admission to new prompts, wait for active
prompts and pending Slack replies, and restart the enrolled services. A
chat-requested update waits for the requesting response to finish. These updates
verify installed hashes, the instance identity, and fresh Slack reconnection.
Other experience owners still need to coordinate their own ongoing work before
an instance/runtime restart; an idle ACP connection is not a paused workstream.
Unknown custom providers retain the conservative installation-wide restart scope.

New Slack enrollment records `service_name` on its component. Existing schema-1
registrations infer it from the connector identity or native installation receipt.
An ambiguous mapping falls back to the installation-wide scope. Plans made before
scoped updates recover using their original installation-wide behavior. Changing
an enrollment after a scoped plan is prepared requires preparing a new plan.

Failure restores previous binaries. An interrupted
cutover recovers on helper restart. Configuration, credentials, conversation stores
and new messages are not overwritten by old backups. Unknown delivery outcomes
prevent cutover and require reconciliation. The update request remains queued or
in progress until the receipt says succeeded, rolled back, or failed.

## API and CLI

- `GET /api/v1/updates`: policy, helper health, versions, plan and durable request.
- `PUT /api/v1/updates/policy`: revisioned settings; administrator authentication.
- `POST /api/v1/updates`: `action`, UUID `request_id`, optional UUID `plan_id`.
  Actions: `check`, `plan`, `update`, `apply`, `rollback`. `update` checks and applies;
  `apply` and `rollback` select a prepared plan. No caller-supplied source or path.
- `POST /api/v1/updates/report`: authenticated host-helper heartbeat and progress.
- `agent-instance updates status|check|plan|update|apply|rollback` uses the same API;
  add `--plan-id` for apply/rollback. `updates configure --file policy.json` changes
  settings with a current `revision`.
- `installation_updates` MCP tool accepts the same actions plus `status`; writes
  require the global agent-initiated switch, and never change that policy.

The UI's route, drafts, controls and actions use the existing view API, including
`updates.check`, `updates.update`, `updates.rollback` and `updates.save`. A voice
agent can display the page, edit settings, and press the same controls through the
existing authenticated browser-control interface.

## Exact plans for additional software

Owner-enrolled command providers may set `"protocol": 2` and supply a fixed
`verify` command in addition to `status`, `plan`, `apply`, and `rollback`. The
helper saves the entire plan response in a private file and verifies its hash
before stopping services. It supplies the file through
`CONSTELLATION_UPDATE_PLAN`; the file contains `id`, `component`, and `plan`.
Commands must consume this saved plan, rather than re-resolving "latest" during
installation. A successful `verify` exit is required after apply and rollback.
Rollback must be idempotent and handle an interrupted apply. Only providers whose
apply was attempted are rolled back. Providers keep their own private receipts
outside the agent's writable folders. Existing protocol-1 registrations continue
to work, with their previous provider-owned transaction semantics.

`ops/updates/container_provider.py` implements this protocol for existing Linux
Constellation containers. Its owner-only configuration names an absolute
`manifest`, `state`, and `installer` path plus an `image` reference and optional
`pull: true`. Each command is an absolute Python path, the absolute provider path,
`--config`, the private config path, `--action`, and the action name. Planning
resolves the image to its immutable local Docker digest. Apply preserves mounts,
credentials and network restrictions, retains the previous container, and checks
the running image. The surrounding helper also verifies instance/Slack health.
An owner must publish or select an image source; this does not publish images or
make arbitrary software on the computer eligible for updates.

Source-installed services, game adapters and their package dependencies can use
the same protocol with their own build, digest, health and rollback logic. They
are not silently treated as single-binary updates. The dashboard reports enrolled
components, not a promise that every installed program has an update provider.

## Updating the whole installation

The helper can enroll vanilla Codex ACP, Claude Agent ACP, and Amplifier Agent ACP
engines as well as the native runtime. `install-updates` discovers these supported
versioned engine layouts. Other custom commands need an owner-enrolled provider.
The external engine provider stages an upstream release in a new private version
directory, freezes its files and dependency graph, and switches only the existing
backend's executable paths at idle. Accounts, models, environment settings and
session stores remain owned by that engine. Failed verification restores the prior
binding. No inference is required to check or install a release.

Use `install-updates --container-manifest <existing manifest> --container-image
<owner-selected reference>` to enroll an existing container. An immutable digest
tracks that exact image; a registry tag is pulled when checking for a newer image.
Existing owner-enrolled local image providers can instead track a locally built
tag with `pull: false`. Enrollment does not build or publish container images.

Connected experiences can enroll protocol-1 `participants` in registration.json:

```json
{"id":"experience","protocol":1,"components":["instance","runtime","engine","container-image"],"commands":{"prepare":["/owner/helper","prepare"],"verify":["/owner/helper","verify"],"restore":["/owner/helper","restore"]}}
```

Commands receive `CONSTELLATION_UPDATE_PLAN`, an immutable private JSON file:
`{id, participant, plan:{schema:1,transaction_id,operation,components,restart_scope}}`.
`operation` is `apply` or `rollback`. `CONSTELLATION_UPDATE_OUTCOME` is `unchanged`,
`applied`, or `rolled_back`. Fixed commands must complete within 30 seconds and
return respectively `{"ready":true}`, `{"verified":true}`, or
`{"restored":true}`. Preparation can return `{"ready":false,"reason":"busy"}`;
the helper polls for up to 30 minutes without interrupting work. The participant
must durably close its own admission before acknowledging readiness, preserve its
original state before mutation, and be idempotent across lost replies. Unknown
listed component IDs are ignored; selection uses changed enrolled components.

Protocol-2 component providers also receive `CONSTELLATION_UPDATE_TRANSACTION`.
An explicitly enrolled component can declare `restart_scope` with `services`,
`instance` and `slack_homes`. For a connector whose participant owns all service
operations, use `{"services":[],"instance":false,"slack_homes":[]}`. Shared engine
and host changes retain the full installation scope by default.

Select **Install automatically when idle** and the desired check interval in the
dashboard. It uses this same transaction for every enrolled component. No separate
chat permissions or scheduler deployment script is needed. A successful staged
plan is not yet an installed update; status records verification and recovery.
