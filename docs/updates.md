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

Updates close admission to new prompts, wait for active prompts and pending Slack
replies, and hold the connector's admission lock across cutover. A chat-requested
update waits for the requesting response to finish. Runtime and instance changes
restart the enrolled services, then verify installed hashes, the instance identity,
and fresh Slack reconnection. Failure restores previous binaries. An interrupted
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
