# Native hosts and other devices

Agent Instance runs as an independent process on Linux, macOS, or Windows. Its
dashboard, setup, management API, and ACP attach command ship in the same binary.
Multiple instances can run under one account, each with its own folder, ports,
identity, credentials, retained releases, and startup job. Adapters remain separate.

## Consumer packages

A release consumer bundle contains the Instance executable, fixed setup support
and its own CPython distribution. Setup copies that interpreter into this
installation's `support/` directory. Backend setup and the independent updater
use that fixed path, including after the original download is removed. The
interpreter is installation support, not the selected reasoning engine.

The package matrix is Windows x64 and ARM64, macOS Intel and Apple Silicon,
and Linux x64 and ARM64. Build and native fixture results qualify the actual
published artifacts; a platform name alone is not a launch or provider proof.
Windows ARM64 uses a native Windows 11 ARM runner. Windows x64 CI uses Server
2025, so consumer Windows 11 desktop account and startup behavior remains a
separate qualification. Packages do not require WSL or developer tools. Windows
Python carries its private VC runtime DLLs; no redistributable installer is run.
Python's exact distributor hashes and license provenance live in
[the source-pinned shared Python lock](../scripts/vendor/python/lock.json), and its notices
ship with every copy. The Linux Python baseline is glibc 2.17; Instance's own
Ubuntu build baseline and PAM dependency still apply. Mac executables are unsigned.

Run `agent-instance setup --home PATH --name NAME` from the extracted bundle,
or let an authorized local fleet recipe invoke it. The default engine is the
checksummed release in [`native-runtime-lock.json`](../scripts/native-runtime-lock.json).
Only that lock defines available engine platforms. A missing target is an
explicit setup error; `--skip-runtime` can create a host whose engine remains
unconfigured. It does not make that host a working agent. Account sign-in and
provider credentials remain separate from installation.

Linux uses an existing user systemd manager for automatic startup. The current
`agent-instance` PAM policy must be installed by an explicit administrator step
as documented in [Linux installation](install.md); setup does not modify `/etc`.
Without startup registration, `--no-service` installs the same independent home
and prints its actual non-running state.

## Developer source installation on a Mac

Install Git, then clone the public source:

```sh
git clone https://github.com/microsoft/amplifier-constellation-instance.git
cd amplifier-constellation-instance
./install-macos.sh --name "My agent"
```

The script detects Apple Command Line Tools and Python, installs Rust if needed,
builds the native executable, installs this version's checksummed Constellation runtime,
and registers a per-user LaunchAgent. If Command Line Tools are missing, Apple's
installer opens; finish it and repeat the command. Python supplied with the Command
Line Tools is sufficient; a current Homebrew Python also works.

Pass `--runtime-binary /path/to/agent-runtime` to use a verified local native
build instead of the pinned download. `--engine codex` explicitly chooses the
separately packaged Codex backend.

The default folder is `~/Library/Application Support/AgentInstance/first-agent`.
Use `--home "/another/new/folder"` for another independent agent. Sign in with
the owning Mac account's short username and password. macOS uses its existing
`login` OpenPAM policy; the installer does not modify `/etc/pam.d`.

## Developer source installation on Windows

Use native PowerShell under the account that will own the agent:

```powershell
git clone https://github.com/microsoft/amplifier-constellation-instance.git
cd amplifier-constellation-instance
.\install-windows.ps1 -Name 'My agent'
```

The script detects Python, Rust, and the Visual C++ build tools. When missing, it
uses Windows Package Manager to install those prerequisites. An administrator
prompt or restart may be required by their installers. `-SkipPrerequisites` makes
missing tools an explicit error. A verified native binary supplied with `-Binary`
avoids the Rust/C++ build prerequisites. Python is still used for installation and
default runtime provisioning.

Constellation is the default engine. To use an already verified native runtime
instead of downloading the pinned release, pass `-RuntimeBinary C:\path\agent-runtime.exe`.
`-Engine codex` explicitly selects the separately packaged Codex backend.

Default folder: `%LOCALAPPDATA%\AgentInstance\first-agent`. Use `-InstanceHome`
to install additional individuals. A task named `AgentInstance-<instance-id>`
starts the service at this owner's logon with limited privileges. It does not
store a password or run as SYSTEM. The instance directory's Windows ACL permits
the owner and SYSTEM; child files inherit that protection.

Browser sign-in uses Windows `LogonUserW`, then compares the authenticated account's
SID with the running owner. Use your account’s short username, or the `DOMAIN\username` /
`MACHINE\username` recorded in `config.json`; both authenticate only this owner. Windows Hello PINs, biometrics, and
interactive MFA are not password substitutes for this endpoint. Account policies
can reject network password logon even when interactive desktop sign-in works.
The browser and automation token APIs otherwise match Linux and macOS.

If PowerShell's execution policy blocks a downloaded script, inspect the checkout
and use your organization's normal script approval process. No installer changes
that machine policy.

## Start, update, inspect, and roll back

Both platform installers print JSON including the instance ID, release, startup
job, verified running state, and connection URLs. They open the local certificate
help page unless `--no-browser` / `-NoBrowser` is supplied. Use the returned LAN or
Tailscale URL from another device. Opening a browser on the installing host is an
optional convenience, not part of initialization.

Example Mac commands, all targeting one existing instance:

```sh
./install-macos.sh update --home "$HOME/Library/Application Support/AgentInstance/first-agent"
./install-macos.sh rollback --home "$HOME/Library/Application Support/AgentInstance/first-agent"
./install-macos.sh inspect --home "$HOME/Library/Application Support/AgentInstance/first-agent"
```

Windows equivalents:

```powershell
.\install-windows.ps1 -Action update -InstanceHome "$env:LOCALAPPDATA\AgentInstance\first-agent"
.\install-windows.ps1 -Action rollback -InstanceHome "$env:LOCALAPPDATA\AgentInstance\first-agent"
.\install-windows.ps1 -Action inspect -InstanceHome "$env:LOCALAPPDATA\AgentInstance\first-agent"
```

Update builds the checked-out source unless a binary is supplied. It retains the
old executable, validates the candidate's configuration compatibility, stops only
this instance's startup job, waits for its service lock, and backs up configuration
and SQLite state. It replaces the executable, starts the same instance, and checks
its authenticated identity/version endpoint. A failed health check restores the
prior executable and starts it again. Profiles, identity, sessions, credentials,
and default runtime files are left in place.

Rollback selects the previous retained executable; `--release` / `-Release`
selects a particular retained release ID. It **does not rewind history**. Database
backups are private recovery artifacts, not automatically restored. A future
incompatible database migration needs an explicit migration/recovery plan; an old
binary that cannot open current data will fail the health check. Do not downgrade
across such a change just because an executable is retained.

`--no-service` / `-NoService` installs without a startup job. Start it directly:

```sh
"/path/to/instance/bin/agent-instance" serve --home "/path/to/instance"
```

On Windows use `& 'C:\path\to\instance\bin\agent-instance.exe' serve --home ...`.
Stop a manually started service before update/rollback. `--skip-runtime` /
`-SkipRuntime` leaves runtime setup to the web UI or a custom ACP backend.
Native installers intentionally connect provider accounts explicitly; they do not
copy an existing user's provider credentials. CLI/API `backend install` also
defaults to Constellation. Account reuse is an option of the explicitly selected
Codex compatibility path, not native provisioning.

LaunchAgents and these Windows tasks run while their owner is logged in. They do
not provide a pre-login daemon or wake a sleeping laptop. Linux's user systemd
installation and optional linger remain documented in [install.md](install.md).
The native update helper does not adopt an older Linux installer layout.

## Host recipe contract

An optional management client can use a verified complete consumer bundle without
Python or a build tool on PATH. Its fixed entry is:

```text
agent-instance[.exe] setup install --home NEW_HOME --name NAME --bind ADDRESS --port PORT --no-browser
agent-instance[.exe] setup update --home EXISTING_HOME --no-browser
agent-instance[.exe] setup rollback --home EXISTING_HOME --no-browser
agent-instance[.exe] setup inspect --home EXISTING_HOME
```

`install` accepts a new/empty home only and obtains the current OS owner. Engine
selection is explicit (`--engine native|codex`); an update does not change it.
`--runtime-binary` is an explicit verified local-engine input, never a URL or shell
fragment. The complete package is checked before its private interpreter runs.
This local OS-owner entry adds no remote command API or new ACP method.

Developer wrappers can still call the same `native-install/1` implementation with
an already available Python:

```sh
python scripts/native-install.py install --home INSTANCE --binary BINARY --name NAME --no-browser
python scripts/native-install.py update --home INSTANCE --binary BINARY
python scripts/native-install.py rollback --home INSTANCE
python scripts/native-install.py inspect --home INSTANCE
```

The helper writes one JSON result to stdout and diagnostics to stderr. Zero exit
means the requested operation completed. A registered service must pass its
authenticated health check before `running_verified` is true. `inspect` reports
installation metadata and makes no live-health claim. `install.json` contains
`manager: "native-install/1"`, `instance_id`, `current`, `previous`, and `service`;
there are no credentials in it. `releases/<version>-<sha256-prefix>/` contains each
immutable executable and manifest. Normal management still uses the versioned API
and scoped token contract. Installation is an OS-owner operation, separate from
the ACP protocol and the service's remote permissions.

## Browser packaging for Windows, Mac, Linux, iOS, and Android

The service embeds its HTML/CSS/JavaScript. It serves those same assets from its
HTTPS origin on every host. No frontend development server, Node install, cloud
hosting, or user-specific static bundle is required. This is the web packaging
boundary for desktop and mobile clients. iOS and Android use a browser connected
to a running host; this release does not install an always-running local agent
process on either mobile OS.

- For LAN use, bind a reachable interface and allow only the desired service and
  certificate-help ports through the host firewall. Installers do not add blanket
  firewall rules or expose ports through an internet router.
- The optional HTTP welcome port serves certificate help and a public CA download
  only. Authentication, profile editing, and ACP access remain HTTPS.
- Trust the instance certificate on the client, or use a correctly configured
  HTTPS reverse proxy. Tailscale HTTPS needs the tailnet DNS name and an issued
  certificate; a Tailscale IP does not itself make a certificate trusted.
- iOS/Android certificate installation and managed-device policies differ. A
  trusted DNS certificate is the simplest mobile experience. Mobile is a remote
  web client, with network reachability and host sleep constraints.
- A separate fleet UI can embed an instance using its explicit embedding origins
  and view-control contract. It can also render its own UI from the API. Neither
  choice changes the agent's identity, conversation boundaries, or ACP transport.

No native mobile shell, app-store packaging, background mobile runtime, or offline
web mode is claimed by this implementation.

## Native validation

The repository's CI checks native executable and process boundaries on Linux,
macOS, and Windows. [Validation](validation.md) gives reproducible commands and
links to results for the published source. Real account sign-in, startup jobs,
and provider access also need an installation check on the target machine.

The current runtime lock controls which native engine architectures can be
provisioned. New Instance packages do not invent or infer unpublished runtime
hashes. CI fixture success does not establish live provider access or every
machine's account policy. `tests/consumer_bundle_live.py` exercises a real package
with developer tools removed from PATH, actual TLS/SQLite support, same-home
restart and preserved credentials/data. It deliberately excludes OS startup
registration, provider calls and production installations.

Platform references: [Apple LaunchAgents](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html),
[Windows account logon](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-logonuserw),
[Codex native Windows sandbox](https://learn.chatgpt.com/docs/windows/windows-sandbox).

## Native engine configuration

Default provisioning installs the pinned runtime into this individual's `engine/releases/` and creates its own `engine/runtime.json`, workspace and history store. It records a digest and installation receipt. The service identifies an executable named `agent-runtime` (or `agent-runtime.exe`) with an explicit `--home` as **Constellation** and offers its native configuration editor. A manually supplied engine follows the same rule. `provider.api_key_env` maps through `backend.secret_env` to a named secret in this installation; native ChatGPT authentication uses its own account files instead.

Readiness inspects configuration and credential-file metadata, not credential contents or a model response. Its `auth_validated` field remains false; a separately observed real task is needed to establish model access. The configuration editor validates and saves through the selected engine, without passing provider credentials to inspection commands. Native engine installation/account setup and the optional Codex compatibility workflow remain separate.
