# Install one agent

For native macOS and Windows, including update/rollback and mobile browser access, see [Native platforms](native-platforms.md).

The installer creates one independent agent ready for identity setup. It gives that agent its own folder, executable, pinned Constellation Rust ACP runtime, state, TLS certificate, credentials, port and user service. It installs the working engine automatically; choosing executable paths and writing JSON are advanced options.

Native setup does not copy the owner's existing provider account. Configure a provider key or connect this agent's own ChatGPT account through Model & capabilities. Each instance owns its credentials and saved conversations. The optional `--backend codex` path retains its separate account-cache reuse behavior; add `--no-reuse-account` when selecting it to connect a fresh account instead.

This installer targets Debian/Ubuntu Linux with systemd and PAM. The web interface can be used from any reachable browser. Native Windows/macOS service installation, account authentication and update/rollback have their own [platform installers](native-platforms.md).

## Install from public source

The source and pinned runtime downloads are public. The default installation needs Git and network access; it does not require a GitHub account.

On Debian/Ubuntu, run this as the OS user who will own the agent, **without putting sudo around the whole block**:

```bash
if ! command -v git >/dev/null; then
  sudo apt-get update
  sudo apt-get install -y git
fi
AGENT_INSTALL_SOURCE="$(mktemp -d)"
git clone https://github.com/microsoft/amplifier-constellation-instance.git "$AGENT_INSTALL_SOURCE/source"
bash "$AGENT_INSTALL_SOURCE/source/install.sh" --from "$AGENT_INSTALL_SOURCE/source" --name "My agent"
```

The command builds the fetched checkout with `Cargo.lock`. Its exact commit and whether the checkout was modified are recorded in the installation. The temporary checkout is no longer required after a successful installation.

The installer uses sudo only when installing missing host packages, adding the dedicated PAM profile, or when `--linger` is selected. Source builds may need `build-essential`, `pkg-config`, `libpam0g-dev`, `libclang-dev`, `cmake`, `curl`, Python 3 and CA certificates. Python is needed for runtime installation/repair; normal native execution uses the two Rust executables. If a suitable Rust toolchain is absent, the installer uses official rustup to add the package's minimum version without changing your default Rust toolchain or shell startup files.

At the end, the installer prints the browser entry address, HTTPS URLs and certificate fingerprint. It tries to open a local browser only from an interactive graphical shell. On a headless machine, open one of the LAN or Tailscale addresses from another machine and follow the certificate connection instructions. Sign in with the installing user's OS account, choose the agent's name and purpose, and configure its provider. The [native account setup](native-configuration.md) supports an API key or this agent's own ChatGPT authorization, including remote callback guidance. Until setup is completed, the management service is available but the agent does not accept work.

Account readiness indicates configured credential metadata. It does not claim that a model request has been validated or that the account has available quota. Each actual ACP interaction exposes its own outcome in the dashboard.

## Default runtime and account recovery

Agent Instance **0.12.0** pins Constellation **0.8.0**. Its platform archives and binary hashes are recorded in `scripts/native-runtime-lock.json` and embedded in the instance executable. Provisioning verifies both the downloaded archive and selected binary before use. The default runtime is downloaded directly without GitHub sign-in. Authenticated GitHub CLI remains a fallback for explicitly configured private releases. Use `--runtime-binary /path/to/agent-runtime` for a verified local build. Every installation owns its runtime files, provider state and history; updating one agent does not update another.

For an initialized instance that lacks the default backend:

```bash
AGENT_INSTANCE_HOME="$HOME/agents/design-reviewer"
"$AGENT_INSTANCE_HOME/bin/agent-instance" backend install --home "$AGENT_INSTANCE_HOME"
"$AGENT_INSTANCE_HOME/bin/agent-instance" backend status --home "$AGENT_INSTANCE_HOME"
```

The same installation operation is available in the web setup and through `POST /api/v1/backend/install`, with `engine: "native"` as the default. It does not turn the agent on or copy an existing conversation. Repeating native provisioning preserves the existing runtime configuration and credential references. A partial download is never selected as the active runtime. If network access is unavailable, supply a local runtime binary or retry when connectivity is restored.

Native account operations use `/api/v1/native-account` and the `native-account` CLI; see [Native configuration](native-configuration.md). OS/PAM sign-in controls access to the management UI; provider authentication supplies model access.

For the explicitly selected Codex alternative, `scripts/backend-lock.json` pins Codex **0.165.0**, `@agentclientprotocol/codex-acp` **2.5.0**, and Node **22.23.2**. That separate package uses its own provider folder and `/api/v1/backend/account/login` device-code operation. Its account cache, private runtime files and npm/Node download process are not part of the native path.

To use a different ACP implementation, pass `--backend none` during installation, then configure that implementation in **Advanced**. This opt-out also makes backend-free installer tests independent of network access. A custom implementation's credentials and capabilities remain its own responsibility; it does not silently become a Codex backend.

## Select a source or version

From an existing checkout:

```bash
bash ./install.sh --from "$PWD" --name "Architecture reviewer"
```

For a fixed commit or tag, set `AGENT_SOURCE_REF` to the desired existing ref, then run:

```bash
bash ./install.sh --ref "$AGENT_SOURCE_REF" --name "Architecture reviewer"
```

This fetches the requested ref through authenticated GitHub access and checks it out detached in a temporary folder. Use a full commit digest for the strongest source pin. `--ref` and `--from` are alternatives; the installer never checks out a different ref in your working tree.

When a release has been built and published, set `AGENT_RELEASE_TAG` to that existing tag:

```bash
bash ./install.sh --release "$AGENT_RELEASE_TAG" --name "Architecture reviewer"
```

The installer downloads a native x86_64 or ARM64 Linux archive through `gh release download`, checks its accompanying SHA-256, and extracts only the expected executable, build metadata, license and dependency notices. Older two-file archives remain readable. The installation retains the license and notices alongside its release. Missing assets fall back to a source build of the same tag. A checksum mismatch stops installation. Checksums detect damaged or inconsistent assets; they are not an independent signature. Prebuilt GNU/Linux assets are built on Ubuntu 24.04; use a source build when the host's libc/PAM libraries are incompatible. The release workflow initially creates a draft for review; this document does not imply a release already exists.

For a locally built executable:

```bash
bash ./install.sh --binary "$PWD/target/release/agent-instance" --name "Local experiment"
```

`install.sh --help` lists all options. `--dry-run` prints the intended installation without downloads, filesystem changes, sudo or service operations.

## Folder, port, and service

Choose a new or empty folder and a fixed port if desired:

```bash
bash ./install.sh \
  --from "$PWD" \
  --home "$HOME/agents/design-reviewer" \
  --name "Design reviewer" \
  --bind 0.0.0.0 \
  --port 18443 \
  --linger
```

The default port is `0`: the first service start selects a free port and preserves the chosen port in configuration. An occupied saved port is an error to inspect rather than a reason to move the service silently. The default bind address is all IPv4 interfaces. Select a specific LAN/Tailscale address or `127.0.0.1` to restrict the listener. IPv6 addresses can be specified explicitly; network availability still depends on host routing/firewall policy.

A second optional welcome page helps browsers connect before they trust a newly generated certificate. `server.welcome_port` defaults to `0`, chooses and persists another free port, and can be a fixed port or `null` to disable it. This HTTP page provides connection instructions and the public trust certificate at `/welcome/root.crt`; it never accepts a password, account credentials or management actions. Sign-in and management use HTTPS only. The installer opens the welcome address by default and prints both address sets. A separately verified preferred HTTPS address may be used directly.

New installations create an instance-specific trust root (`tls/root-ca.crt`) and server certificate. Compare the printed **trust certificate** fingerprint when trusting this instance on a device; the server certificate's fingerprint is shown separately. Private keys remain in the private instance folder. For an externally supplied certificate, follow its normal trust chain and the connection page's guidance.

`--linger` explicitly enables the installing user's systemd services after logout and at boot. Without it, service lifetime follows the host's existing user-session configuration. The installer does not change firewall, router, Tailscale or AppArmor policy.

Each installation contains:

```text
bin/agent-instance                  # this instance's own executable copy
releases/<version>-<digest>/        # its retained executable and build metadata
config.json                        # own identity and desired configuration
install.json                       # source/version/digest/install provenance
agent-instance.service             # generated user-service definition
tls/                               # own certificate and private key
secrets/                           # own credentials; never printed as URLs
engine/releases/<version>-<digest>/ # own Constellation executable
engine/runtime.json                # own native provider and tool configuration
engine/history.sqlite3             # own native saved conversations
engine/accounts/                   # own connected native provider accounts
workspace/                         # default directory for this agent's work
endpoints.json                     # addresses and certificate fingerprint after start
...                                # state and other folders created by the agent
```

The user unit is named `agent-instance-<instance-id>.service`; its exact name is printed during installation. It starts the executable from that instance's folder, with that same folder as its home. Stopping it terminates its own process group, not other agents. There is no shared agent runtime or fleet coordinator.

The installer refuses an existing nonempty instance folder. Editing a running agent uses its configuration API/CLI, not a second install into the same home. Mac/Windows update and rollback use the [native platform helper](native-platforms.md); that helper does not adopt an older Linux installer layout. [Checkpoints](checkpoints.md) provide explicit offline restore/clone operations into new folders without registering services.

Use an ordinary Linux path without backslashes, control characters or trailing whitespace. Spaces, dollar signs and percent signs are supported and are passed literally to the service.

## Headless and managed-host options

`--no-browser` suppresses browser launch. `--no-start` installs the user unit without starting it. `--no-service` creates the unconfigured installation without registering a user unit; run it manually or supply another supervisor:

```bash
AGENT_INSTANCE_HOME="$HOME/agents/design-reviewer"
"$AGENT_INSTANCE_HOME/bin/agent-instance" serve --home "$AGENT_INSTANCE_HOME"
```

In another terminal, inspect addresses or state:

```bash
"$AGENT_INSTANCE_HOME/bin/agent-instance" endpoints --home "$AGENT_INSTANCE_HOME"
"$AGENT_INSTANCE_HOME/bin/agent-instance" status --home "$AGENT_INSTANCE_HOME" --json
"$AGENT_INSTANCE_HOME/bin/agent-instance" config get --home "$AGENT_INSTANCE_HOME"
```

Configuration can also be prepared entirely through files and CLI. Save `config get` to a private JSON file, edit the supported fields, then validate and apply it against the revision you read:

```bash
"$AGENT_INSTANCE_HOME/bin/agent-instance" config validate --home "$AGENT_INSTANCE_HOME" --file "$AGENT_CONFIG_FILE"
"$AGENT_INSTANCE_HOME/bin/agent-instance" config set --home "$AGENT_INSTANCE_HOME" --file "$AGENT_CONFIG_FILE" --expected-revision "$AGENT_CONFIG_REVISION"
```

Set those two variables to your private file and its original numeric revision. The file may contain the complete configuration document or its bare `config` object. A stale revision is rejected rather than overwriting another edit. Use secret references for backend credentials; do not put actual credentials in ordinary configuration environment values.

If an SSH shell cannot reach the user service manager, the installer leaves the new installation and unit intact and prints the `systemctl --user enable --now ...` command to run in the correct user session. It does not fall back to a root service.

`--skip-prerequisites` requires dependencies to be installed already. `--skip-pam` requires an existing `/etc/pam.d/agent-instance`. Administrators can use these when package installation and PAM policy are managed centrally. An existing, different PAM profile is preserved, never overwritten. The supplied Debian/Ubuntu profile includes the host's `common-auth` and `common-account`, keeping its account policy in effect.

PAM initially supports only the service owner's account. Password authentication requires HTTPS, including on a LAN. Login does not require granting browser clients access to the instance's local control-token file. For automation, use scoped, expiring grants rather than giving another agent an OS password. Keep token output out of shared logs and command histories.

## ACP connection information

Use the instance's ACP entrypoint from a compatible local client:

```text
command: /absolute/path/to/instance/bin/agent-instance
args: ["acp", "--home", "/absolute/path/to/instance"]
```

The command connects to the already running individual. The dashboard and `endpoints` command expose the service's supported connections and current certificate information. Any advertised custom network transport is identified as such; it is not automatically a standardized ACP transport. Slack/Teams credentials and routing belong in separate adapters, not this installation.

## Troubleshooting and validation

- **Private repository cannot be read:** check the active `gh` account and repository access. No token needs to be pasted into the installer.
- **PAM login denied:** use the installing user's account and inspect the host's account policy. Unsupported multi-step/MFA conversations require an authentication enhancement; disabling PAM policy is not a workaround.
- **Certificate warning:** inspect the printed SHA-256 fingerprint and trust the intended certificate, or configure an appropriate certificate. A reachable port is not enough to establish the server's identity.
- **Port unavailable:** inspect `config.json` through the configuration interface and choose an available port; update clients deliberately.
- **Runtime missing:** select Install runtime in setup, or run `backend install`. Python 3 and HTTPS access to nodejs.org and registry.npmjs.org are needed. Other installed agents are unaffected.
- **ChatGPT connection missing or expired:** connect again from setup/dashboard. Device-code login needs the account/workspace setting enabled. Account readiness is separate from successful model execution and available quota.
- **Backend tools fail under AppArmor:** the agent service may still be healthy while its selected backend lacks required host capabilities. Inspect that backend's diagnostics. Any host policy change is a separate reviewed administrator action; this installer does not relax global user-namespace restrictions or install AppArmor exceptions.
- **Service stops after logout:** use the printed explicit linger command when persistent background operation is intended.

Run `python3 scripts/test-installation.py` and `python3 scripts/test-provisioning.py` for deterministic installer and runtime-boundary checks. They do not invoke sudo, download runtimes, start a service, or use real provider/PAM credentials. A clean-host install, browser login, pinned ACP attachment and restart remain the meaningful end-to-end checks for a release.

`python3 tests/pam_live.py --binary target/release/agent-instance` is an explicit host integration check. It requires passwordless sudo, installs the matching PAM profile if absent, creates a disposable OS user, and runs a loopback-only unconfigured process under that user. It checks real password/account policy, cookies, CSRF and logout, then removes the account, process and temporary files. Passwords travel only through stdin and the local HTTPS request body; reports contain no credentials. It preserves any existing different PAM profile and never changes an existing user's password or account policy.

Sources: [GitHub CLI clone](https://cli.github.com/manual/gh_repo_clone), [authenticated release downloads](https://cli.github.com/manual/gh_release_download), [official rustup](https://rustup.rs/), [GitHub runner platforms](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).

Account connection follows the official [Codex authentication guidance](https://learn.chatgpt.com/docs/auth) and [app-server account API](https://learn.chatgpt.com/docs/app-server). Runtime sources are the published [Codex ACP package](https://www.npmjs.com/package/@agentclientprotocol/codex-acp), [Codex package](https://www.npmjs.com/package/@openai/codex), and [Node 22.23.2 release checksums](https://nodejs.org/dist/v22.23.2/SHASUMS256.txt).
