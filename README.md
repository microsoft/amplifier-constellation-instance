# Agent Instance

Part of [Amplifier Constellation](https://github.com/microsoft/amplifier-constellation).

Native Windows and macOS installation, account sign-in, and per-instance update/rollback are documented in [Native platforms](docs/native-platforms.md). Linux installation remains in [Installation](docs/install.md).

One independently installed ACP agent, with its own setup, configuration, observation dashboard, and programmable screen controls.

Each installation has a stable identity, private folder, copied executable, credentials, provider state, HTTPS port, and user service. Different instances can use different releases and ACP backends on the same machine or on separate machines. Slack, Teams, and other adapters live outside this service.

## Install

On Debian/Ubuntu, install Git and run:

```bash
AGENT_INSTALL_SOURCE="$(mktemp -d)"
git clone https://github.com/microsoft/amplifier-constellation-instance.git "$AGENT_INSTALL_SOURCE/source"
bash "$AGENT_INSTALL_SOURCE/source/install.sh" --name "My agent"
```

Agent Instance **0.17.0** installs **Constellation 0.11.0**, the independent Rust ACP runtime, by default. The installer adds missing build prerequisites, creates the instance, starts its user service, and opens connection help when possible. It prints LAN welcome/HTTPS URLs and certificate fingerprints for headless installation. Sign in with the installing user's machine account, name the agent, and configure its provider in Model & capabilities. Native setup does not install Codex or Node or copy an existing provider account. Use a provider key or connect this agent's own ChatGPT account. Free ports are selected once and saved; `--port` overrides HTTPS.

Custom ACP engines remain supported. The Linux installer's `--backend codex` explicitly selects the optional Codex package; `--backend none` leaves the engine unconfigured. A verified local Constellation binary can be supplied with `--runtime-binary`. [Native platforms](docs/native-platforms.md) describes the equivalent Mac and Windows options and the available release architectures.

See [installation](docs/install.md) for the complete bootstrap command, version selection, service persistence, certificate trust, and removal. The running service does not depend on the source checkout or share its executable with another instance.

The [0.17.0 release notes](docs/releases/0.17.0.md) describe the empty-receipt reply fix. The [0.16.1 release notes](docs/release-0.16.1.md) describe active-only peer awareness and collaboration guidance. The [0.16.0 release](docs/release-0.16.md) introduced negotiated mid-turn input. Publication and installation rollout are separate steps.

## What this version does

- Saved service connections with Add, Connect and Disconnect through fixed installation-owned actions. Domain setup, credentials, sessions and tasks remain local. [Connection contract](docs/service-connections.md), [0.21.1 release notes](docs/releases/0.21.1.md).

- Two-step setup: Identity → Ready. Installed runtime and account readiness are visible; runtime paths, environment, limits, and service controls stay under Advanced settings. Installation identity and owner are read-only.
- Public connection help, certificate download, and Windows/Mac/Ubuntu browser-trust instructions. The optional HTTP listener serves only this help; sign-in and management require HTTPS.
- Useful Activity with work outcomes, problems and connection/service changes, server-side search/pagination, and opt-in diagnostics with separate retention.
- Guided GitHub setup verifies repository tools, preserves other connections, and explicitly enables local clients. Build readiness remains a separate check. [Development setup](docs/development-setup.md).
- Optional conversation sources let one agent retrieve context across Slack, web conversations and ordinary ACP clients, with scoped audience checks and standard MCP tools. [Continuity contract](docs/conversation-continuity.md).
- Durable responsibilities with explicit ownership, scoped agent tools, review/evidence history, and prepared delegation envelopes. Local discipline templates, assignments and named-peer sharing rules remain independent of ACP and any fleet. [Work contract](docs/responsibilities.md).
- Optional frozen acceptance briefs, exact artifact-version hashes, named peer review and requester-only closure through the management API/CLI. Recorded reports stay distinct from checks actually executed. [Completion contract](docs/completion-contract.md).
- Offline whole-agent checkpoints with explicit credential policy, checked archives, identity-preserving restore and new-identity clone. [Checkpoint commands](docs/checkpoints.md).
- Optional native ChatGPT sign-in with guided local/remote callback setup, account-model selection, and the same controls in the CLI/API. [Native account setup](docs/native-configuration.md).
- Attributed durable memory with separate audiences, retained conflicting accounts, explicit resolution and source inspection through the UI, API and scoped agent tools. [Memory contract](docs/memory.md).
- Optional durable background execution for explicitly authorized local responsibilities, with bounded multi-turn progress, saved ACP session continuation, pause/cancel controls, restart reconciliation and reports awaiting review. [Background work](docs/background-work.md).
- Optional native engine editor discovers the installed engine's model, provider, native tool and decision-service settings without extending the ACP boundary. [Native configuration](docs/native-configuration.md).
- Profile-in-use observations distinguish installed guidance from the latest started turn and leave behavioral quality unclaimed.
- Separate `communication.md`, `working-style.md`, and `identity.md` context files.
- Write-only named secrets and scoped, expiring access tokens.
- Dashboard with active ACP connections, observed sessions, lifecycle events, actual addresses, certificate fingerprint, and pending service-setting changes.
- CLI and versioned management API for all service operations; JSON Schema and OpenAPI discovery for other tools.
- Per-display navigation, wizard step, editor drafts, selection, filters, focus, scroll, and modal state. Remote changes have revision checks and browser render acknowledgements.
- Bundled responsive UI with relative asset/API paths and an origin-checked embedding bridge.

The service has Linux, macOS and Windows installation paths. The browser UI works from Windows, Mac, Linux, iOS, Android, or other browser platforms that can reach it. Platform-specific validation and remaining limitations are recorded in [Native platforms](docs/native-platforms.md) and [Validation](docs/validation.md).

## Two interfaces

**ACP conducts work.** A standard stdio attach command connects a client to this installed individual:

```text
command: /path/to/instance/bin/agent-instance
args: ["acp", "--home", "/path/to/instance"]
```

Each connection runs a separate configured ACP backend process. The relay preserves negotiated capabilities, content, MCP descriptors, session configuration, callbacks, streaming, and cancellation. Backend capabilities determine persistence and supported session features. Instance instructions/profiles are supplied as ordinary prompt context on each turn; they are not a privileged system-message channel.

The service's authenticated WebSocket hop is explicitly named `agent-instance.wss/1`; it is a custom remote framing profile. The standard compatibility boundary remains ACP over stdio. [Protocol details and verification](docs/acp-boundary.md).

**Management operates the installation.** The CLI and bundled UI use `/api/v1`. Other interfaces and agents can use it directly, without driving browser pixels. Management is optional for future fleet integrations: an ACP-only third-party agent should still be useful. [API contract](docs/api.md), [UI control contract](docs/ui-contract.md).

## Local commands

```bash
agent-instance init --home ./my-agent --name "My agent"
agent-instance backend install --home ./my-agent
agent-instance serve --home ./my-agent
agent-instance status --home ./my-agent --json
agent-instance config get --home ./my-agent
agent-instance endpoints --home ./my-agent
agent-instance background status --home ./my-agent
agent-instance api GET views --home ./my-agent
```

`init` is for an unconfigured folder and refuses to overwrite an existing instance. The installer also sets up the host PAM policy and service. `config set --file ... --expected-revision ...` updates a running service; an offline `config.json` can be edited and checked with `config validate` before service start. Use private secret files/references rather than placing credentials in command arguments or ordinary configuration values.

## Verification

```bash
cargo test --locked
cargo clippy --all-targets -- -D warnings
python3 scripts/test-installation.py
npm ci --prefix tests/acp-client
cargo build --locked
python3 tests/acp_wss_smoke.py
```

The ACP integration tests run an independent Python backend. The WSS smoke test uses the pinned official TypeScript ACP SDK through the complete service and standard attach command. The optional `tests/pam_live.py` check creates and cleans up a disposable OS account to test real password and account policy; it requires sudo. Browser tests and release evidence are described in [validation](docs/validation.md).

The ACP relay records connection/session metadata rather than copying raw turns or backend stderr into its activity log. Constellation owns its saved conversations and exposes authorized retrieval through the optional context layer; another ACP engine's private history still requires its own support or connector. Directory allowlists validate session requests; tool isolation remains the backend's responsibility. Treat configured backend executables and instance-management administrators as trusted code/operators.

`backend install` now selects Constellation by default. Only the explicit `backend install --engine codex` compatibility path can reuse the installing owner's Codex account cache; add `--no-reuse-account` for a fresh Codex account. Native provider authentication is independent of that path.

## Separate experiences

**Agent Observatory** supplies an ACP-first fleet experience with optional management/provisioning providers. **Persona Workshop** extracts separate communication and working-style guidance, keeps identity metadata separate, and exports portable profiles. They are separate repositories and services; this installation works without either. [The roadmap](docs/roadmap.md) describes the boundaries, and [0.11 release notes](docs/release-0.11.md) summarize the current changes and limits.

## Contributing

> [!NOTE]
> This project is not currently accepting external contributions, but we're actively working toward opening this up. We value community input and look forward to collaborating in the future. For now, feel free to fork and experiment!

Most contributions require you to agree to a
Contributor License Agreement (CLA) declaring that you have the right to, and actually do, grant us
the rights to use your contribution. For details, visit [Contributor License Agreements](https://cla.opensource.microsoft.com).

When you submit a pull request, a CLA bot will automatically determine whether you need to provide
a CLA and decorate the PR appropriately (e.g., status check, comment). Simply follow the instructions
provided by the bot. You will only need to do this once across all repos using our CLA.

This project has adopted the [Microsoft Open Source Code of Conduct](https://opensource.microsoft.com/codeofconduct/).
For more information see the [Code of Conduct FAQ](https://opensource.microsoft.com/codeofconduct/faq/) or
contact [opencode@microsoft.com](mailto:opencode@microsoft.com) with any additional questions or comments.

## Trademarks

This project may contain trademarks or logos for projects, products, or services. Authorized use of Microsoft
trademarks or logos is subject to and must follow
[Microsoft's Trademark & Brand Guidelines](https://www.microsoft.com/legal/intellectualproperty/trademarks/usage/general).
Use of Microsoft trademarks or logos in modified versions of this project must not cause confusion or imply Microsoft sponsorship.
Any use of third-party trademarks or logos are subject to those third-party's policies.

Agent image uploads and the optional ACP identity interface: [portable images](docs/agent-identity.md).

### Optional Linux containment

For an existing Linux installation, see the [per-installation container recipe](ops/container/README.md). It preserves ACP connections and managed certificate renewal while keeping host login and service administration outside the container. This is an experimental administrator path with host-managed updates.

### Updates

Use **Updates** to check, install, schedule checks, or let the agent request updates from chat. Each installation owns its update helper. See [installation updates](docs/updates.md).

Saved service connections: see [installation-owned Add, Connect and Disconnect](docs/service-connections.md). The host helper executes only enrolled fixed domain commands; credentials and sessions stay with each connector.

[Instance 0.21.2](docs/releases/0.21.2.md) adds explicit pre-forward attention rejection evidence and reduces history-load diagnostic overhead.
