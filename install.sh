#!/usr/bin/env bash
# Install one independent, unconfigured agent. Run as its OS owner, never as root.
set -euo pipefail
umask 077

SCRIPT_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
REPOSITORY=microsoft/amplifier-constellation-instance
AGENT_INSTALL_HOME=""
AGENT_NAME="New agent"
AGENT_OWNER=$(id -un)
BIND_ADDRESS=0.0.0.0
PORT=0
SOURCE_CHECKOUT=""
SOURCE_REF=""
RELEASE_TAG=""
LOCAL_BINARY=""
INSTALL_SERVICE=1
START_SERVICE=1
OPEN_BROWSER=1
INSTALL_PREREQUISITES=1
INSTALL_PAM=1
ENABLE_LINGER=0
DEFAULT_BACKEND=native
NATIVE_BINARY=""
REUSE_ACCOUNT=1
DRY_RUN=0
TEMP_DIR=""

usage() {
  cat <<'USAGE'
Install one agent-instance (Debian/Ubuntu Linux, as the installing user).

  bash install.sh [options]

Source (choose at most one; default: this checkout):
  --from PATH          Build this checkout with its locked dependencies.
  --ref REF            Fetch a Git ref from the selected repository and build it.
  --release TAG        Download a checksummed binary; build that tag if unavailable.
  --binary PATH        Install an existing local binary, without building.
  --repo OWNER/NAME    Repository for --ref/--release (default microsoft/amplifier-constellation-instance).

Instance:
  --home PATH          New, empty instance folder (default: a unique folder under
                       ~/.local/share/agent-instance/).
  --name NAME          Display name (default: New agent).
  --owner USER         PAM login owner; must be the current installing user.
  --bind IP            Listener address (default: 0.0.0.0).
  --port PORT          Port, or 0 to select and persist a free port (default: 0).
  --backend TYPE       native (default): independent Constellation Rust runtime;
                       codex: alternative engine; none: advanced configuration.
  --runtime-binary PATH  Use this native executable instead of the pinned download.
  --no-reuse-account   Connect an account in setup instead of copying this OS
                       owner's existing Codex account once into this instance.

Installation:
  --no-service         Initialize files only; do not install or start a user service.
  --no-start           Install the user service without starting it.
  --no-browser         Print connection URLs without opening a browser.
  --linger             Enable this user's services after logout and at boot (sudo).
  --skip-prerequisites Require dependencies to be installed already.
  --skip-pam           Keep an existing /etc/pam.d/agent-instance profile.
  --dry-run            Print the plan; no downloads, writes, sudo or services.
  --help               Show this help.

The installer adds missing apt build/runtime dependencies when needed and installs
the dedicated PAM profile through sudo. It does not modify AppArmor, firewalls,
Tailscale, other instances or the user's default Rust version. The default runtime
gets its own account store; no existing provider account is modified.
USAGE
}
die() { printf 'Install error: %s\n' "$*" >&2; exit 1; }
note() { printf '%s\n' "$*"; }
need_value() { [[ $# -ge 2 && -n "$2" ]] || die "$1 requires a value"; }
while [[ $# -gt 0 ]]; do
  case "$1" in
    --home) need_value "$@"; AGENT_INSTALL_HOME=$2; shift 2 ;;
    --name) need_value "$@"; AGENT_NAME=$2; shift 2 ;;
    --owner) need_value "$@"; AGENT_OWNER=$2; shift 2 ;;
    --bind) need_value "$@"; BIND_ADDRESS=$2; shift 2 ;;
    --port) need_value "$@"; PORT=$2; shift 2 ;;
    --backend) need_value "$@"; DEFAULT_BACKEND=$2; shift 2 ;;
    --runtime-binary) need_value "$@"; NATIVE_BINARY=$2; shift 2 ;;
    --no-reuse-account) REUSE_ACCOUNT=0; shift ;;
    --from) need_value "$@"; SOURCE_CHECKOUT=$2; shift 2 ;;
    --ref) need_value "$@"; SOURCE_REF=$2; shift 2 ;;
    --release) need_value "$@"; RELEASE_TAG=$2; shift 2 ;;
    --binary) need_value "$@"; LOCAL_BINARY=$2; shift 2 ;;
    --repo) need_value "$@"; REPOSITORY=$2; shift 2 ;;
    --no-service) INSTALL_SERVICE=0; START_SERVICE=0; shift ;;
    --no-start) START_SERVICE=0; shift ;;
    --no-browser) OPEN_BROWSER=0; shift ;;
    --linger) ENABLE_LINGER=1; shift ;;
    --skip-prerequisites) INSTALL_PREREQUISITES=0; shift ;;
    --skip-pam) INSTALL_PAM=0; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) die "Unknown option: $1" ;;
  esac
done

[[ $(uname -s) == Linux ]] || die "This installer currently supports Linux. See docs/install.md for platform limits."
[[ $(id -u) -ne 0 ]] || die "Run as the OS user who will own the agent, without sudo. The installer requests sudo only for host prerequisites."
[[ "$AGENT_OWNER" == "$(id -un)" ]] || die "The Linux PAM setup currently supports the installing user's own account only."
[[ "$PORT" =~ ^[0-9]{1,5}$ ]] && (( 10#$PORT <= 65535 )) || die "--port must be between 0 and 65535."
[[ "$REPOSITORY" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || die "--repo must be OWNER/NAME."
[[ "$DEFAULT_BACKEND" == native || "$DEFAULT_BACKEND" == codex || "$DEFAULT_BACKEND" == none ]] || die "--backend must be native, codex or none."
for value in "$AGENT_INSTALL_HOME" "$AGENT_NAME" "$AGENT_OWNER" "$SOURCE_REF" "$RELEASE_TAG"; do
  [[ "$value" != *$'\n'* && "$value" != *$'\r'* ]] || die "Arguments cannot contain newlines."
done
[[ "$SOURCE_REF" != -* && "$RELEASE_TAG" != -* ]] || die "Refs and release tags cannot begin with a dash."
source_count=0
for value in "$SOURCE_CHECKOUT" "$SOURCE_REF" "$RELEASE_TAG" "$LOCAL_BINARY"; do
  [[ -z "$value" ]] || source_count=$((source_count + 1))
done
(( source_count <= 1 )) || die "Choose one of --from, --ref, --release or --binary."
if (( source_count == 0 )); then SOURCE_CHECKOUT=$SCRIPT_ROOT; fi
(( INSTALL_SERVICE || !ENABLE_LINGER )) || die "--linger requires service installation; omit --no-service or --linger."
if [[ -z "$AGENT_INSTALL_HOME" ]]; then
  AGENT_INSTALL_HOME="$HOME/.local/share/agent-instance/instance-$(date -u +%Y%m%dT%H%M%S)-${RANDOM}"
fi
[[ "$AGENT_INSTALL_HOME" == /* ]] || AGENT_INSTALL_HOME="$PWD/$AGENT_INSTALL_HOME"
[[ -f "$SCRIPT_ROOT/scripts/install-support.py" ]] || die "Run the installer from a complete checkout or release installer bundle."

note "Install one unconfigured agent: $AGENT_NAME"
note "Folder: $AGENT_INSTALL_HOME"
note "Owner: $AGENT_OWNER; bind: $BIND_ADDRESS; port: $PORT (0 selects a persistent free port)"
note "Source: ${SOURCE_CHECKOUT:-${SOURCE_REF:-${RELEASE_TAG:-$LOCAL_BINARY}}}"
note "Service: $INSTALL_SERVICE; start: $START_SERVICE; install PAM: $INSTALL_PAM; linger: $ENABLE_LINGER"
note "Runtime: $DEFAULT_BACKEND; reuse this OS owner's existing account if available: $REUSE_ACCOUNT"
if (( DRY_RUN )); then
  note "Dry run: no changes. Missing prerequisites would be installed only if needed; each instance gets its own executable and credentials."
  exit 0
fi

TEMP_DIR=$(mktemp -d -t agent-instance-install.XXXXXXXX)
trap 'rm -rf -- "$TEMP_DIR"' EXIT
trap 'printf "Installation stopped. Existing agents were not modified. Inspect the new folder and printed diagnostics before retrying.\n" >&2' ERR

as_root() {
  command -v sudo >/dev/null || die "sudo is needed for host prerequisites. Install them with an administrator, then use --skip-prerequisites --skip-pam."
  sudo -- "$@"
}
install_packages() {
  local missing=() package
  for package in "$@"; do
    if ! command -v dpkg-query >/dev/null || [[ $(dpkg-query -W -f='${Status}' "$package" 2>/dev/null || true) != 'install ok installed' ]]; then
      missing+=("$package")
    fi
  done
  (( ${#missing[@]} )) || return 0
  (( INSTALL_PREREQUISITES )) || die "Missing packages: ${missing[*]}. Install them or omit --skip-prerequisites."
  command -v apt-get >/dev/null || die "Automatic prerequisites currently use apt. Install ${missing[*]} yourself and rerun with --skip-prerequisites."
  note "Installing missing prerequisites: ${missing[*]}"
  as_root apt-get update
  as_root apt-get install -y --no-install-recommends "${missing[@]}"
}

install_packages python3 libpam0g ca-certificates
python3 "$SCRIPT_ROOT/scripts/install-support.py" preflight "$AGENT_INSTALL_HOME" "$BIND_ADDRESS"
AGENT_INSTALL_HOME=$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).resolve())' "$AGENT_INSTALL_HOME")

if (( INSTALL_PAM )); then
  [[ -f /etc/pam.d/common-auth && -f /etc/pam.d/common-account ]] || die "This host needs a reviewed PAM profile at /etc/pam.d/agent-instance; common-auth/account were not found. Then rerun with --skip-pam."
  if [[ -f /etc/pam.d/agent-instance ]]; then
    cmp -s "$SCRIPT_ROOT/scripts/pam/agent-instance" /etc/pam.d/agent-instance || die "An existing /etc/pam.d/agent-instance differs. It was preserved. Review it, then use --skip-pam if it is the policy you intend."
  else
    note "Installing /etc/pam.d/agent-instance using the host's common-auth and common-account policies."
    as_root install -m 0644 "$SCRIPT_ROOT/scripts/pam/agent-instance" /etc/pam.d/agent-instance
  fi
else
  [[ -f /etc/pam.d/agent-instance ]] || die "--skip-pam requires an existing /etc/pam.d/agent-instance."
fi

SOURCE_COMMIT=""
SOURCE_DIRTY=unknown
BINARY_SOURCE=local-binary
if [[ -n "$SOURCE_REF" || -n "$RELEASE_TAG" ]]; then
  install_packages gh git
  gh auth status --hostname github.com >/dev/null 2>&1 || die "Authenticate GitHub CLI for access to $REPOSITORY first: gh auth login"
fi

if [[ -n "$RELEASE_TAG" ]]; then
  case $(uname -m) in
    x86_64) release_target=x86_64-unknown-linux-gnu ;;
    aarch64|arm64) release_target=aarch64-unknown-linux-gnu ;;
    *) release_target="" ;;
  esac
  if [[ -n "$release_target" ]]; then
    archive="agent-instance-${release_target}.tar.gz"
    if gh release download "$RELEASE_TAG" --repo "$REPOSITORY" --pattern "$archive" --pattern "$archive.sha256" --dir "$TEMP_DIR"; then
      # A corrupt/malformed downloaded asset fails closed; only absence uses a source fallback.
      python3 "$SCRIPT_ROOT/scripts/extract-release.py" "$TEMP_DIR/$archive" "$TEMP_DIR/$archive.sha256" "$TEMP_DIR/release"
      LOCAL_BINARY="$TEMP_DIR/release/agent-instance"
      SOURCE_COMMIT=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("commit", ""))' "$TEMP_DIR/release/build-info.json")
      SOURCE_DIRTY=false
      BINARY_SOURCE=github-release
    else
      note "No usable prebuilt download for this tag/platform; building the exact requested tag instead."
      SOURCE_REF=$RELEASE_TAG
    fi
  else
    note "No prebuilt release target for this architecture; building the exact requested tag."
    SOURCE_REF=$RELEASE_TAG
  fi
fi

if [[ -n "$SOURCE_REF" ]]; then
  SOURCE_CHECKOUT="$TEMP_DIR/source"
  gh repo clone "$REPOSITORY" "$SOURCE_CHECKOUT" -- --depth 1
  git -C "$SOURCE_CHECKOUT" fetch --depth 1 origin "$SOURCE_REF"
  git -C "$SOURCE_CHECKOUT" checkout --detach FETCH_HEAD
fi

if [[ -n "$SOURCE_CHECKOUT" ]]; then
  [[ -f "$SOURCE_CHECKOUT/Cargo.toml" && -f "$SOURCE_CHECKOUT/Cargo.lock" ]] || die "Source checkout must contain Cargo.toml and Cargo.lock."
  SOURCE_CHECKOUT=$(cd -- "$SOURCE_CHECKOUT" && pwd -P)
  install_packages build-essential pkg-config libpam0g-dev libclang-dev cmake curl
  minimum_rust=$(python3 "$SCRIPT_ROOT/scripts/install-support.py" package-field "$SOURCE_CHECKOUT/Cargo.toml" rust-version)
  [[ "$minimum_rust" =~ ^[0-9]+\.[0-9]+(\.[0-9]+)?$ ]] || die "Cargo.toml must specify a numeric rust-version."
  if [[ "$minimum_rust" =~ ^[0-9]+\.[0-9]+$ ]]; then minimum_rust="$minimum_rust.0"; fi
  cargo_command=(cargo)
  if ! command -v cargo >/dev/null || ! command -v rustc >/dev/null || ! python3 "$SCRIPT_ROOT/scripts/install-support.py" rust-at-least "$minimum_rust"; then
    (( INSTALL_PREREQUISITES )) || die "Rust $minimum_rust or newer is required."
    rustup_binary=$(command -v rustup || true)
    if [[ -z "$rustup_binary" && -x "${CARGO_HOME:-$HOME/.cargo}/bin/rustup" ]]; then rustup_binary="${CARGO_HOME:-$HOME/.cargo}/bin/rustup"; fi
    if [[ -z "$rustup_binary" ]]; then
      note "Installing rustup for this user; preserving the default toolchain and shell startup files."
      curl --proto '=https' --tlsv1.2 --fail --silent --show-error https://sh.rustup.rs -o "$TEMP_DIR/rustup-init.sh"
      sh "$TEMP_DIR/rustup-init.sh" -y --no-modify-path --default-toolchain none --profile minimal
      rustup_binary="${CARGO_HOME:-$HOME/.cargo}/bin/rustup"
    fi
    "$rustup_binary" toolchain install "$minimum_rust" --profile minimal
    cargo_command=("$rustup_binary" run "$minimum_rust" cargo)
  fi
  note "Building the standalone agent with Cargo.lock."
  "${cargo_command[@]}" build --release --locked --manifest-path "$SOURCE_CHECKOUT/Cargo.toml" --target-dir "$TEMP_DIR/target"
  LOCAL_BINARY="$TEMP_DIR/target/release/agent-instance"
  BINARY_SOURCE=source-build
  if command -v git >/dev/null && git -C "$SOURCE_CHECKOUT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    SOURCE_COMMIT=$(git -C "$SOURCE_CHECKOUT" rev-parse HEAD)
    SOURCE_DIRTY=false
    [[ -z $(git -C "$SOURCE_CHECKOUT" status --porcelain) ]] || SOURCE_DIRTY=true
  fi
fi

[[ -x "$LOCAL_BINARY" ]] || die "No executable agent-instance binary found."
binary_version=$("$LOCAL_BINARY" --version)
[[ "$binary_version" =~ ^agent-instance[[:space:]]([0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?)$ ]] || die "The selected binary does not identify as agent-instance: $binary_version"
version=${BASH_REMATCH[1]}
digest=$(sha256sum "$LOCAL_BINARY")
digest=${digest%% *}
release_dir="$AGENT_INSTALL_HOME/releases/${version}-${digest:0:12}"
install -d -m 0700 "$AGENT_INSTALL_HOME" "$AGENT_INSTALL_HOME/bin" "$release_dir/bin"
install -m 0700 "$LOCAL_BINARY" "$release_dir/bin/agent-instance"
install -m 0700 "$LOCAL_BINARY" "$AGENT_INSTALL_HOME/bin/agent-instance"
for notice in LICENSE THIRD-PARTY-NOTICES.txt; do
  notice_source="$(dirname "$LOCAL_BINARY")/$notice"
  [[ -f "$notice_source" ]] || notice_source="${SOURCE_CHECKOUT:-$SCRIPT_ROOT}/$notice"
  install -m 0600 "$notice_source" "$release_dir/$notice"
done
"$AGENT_INSTALL_HOME/bin/agent-instance" init --home "$AGENT_INSTALL_HOME" --name "$AGENT_NAME" --owner "$AGENT_OWNER" --bind "$BIND_ADDRESS" --port "$PORT"
if [[ "$DEFAULT_BACKEND" != none ]]; then
  note "Installing this agent's own $DEFAULT_BACKEND ACP runtime."
  provision_command=("$AGENT_INSTALL_HOME/bin/agent-instance" backend install --home "$AGENT_INSTALL_HOME" --engine "$DEFAULT_BACKEND")
  if [[ -n "$NATIVE_BINARY" ]]; then provision_command+=(--runtime-binary "$NATIVE_BINARY"); fi
  if (( !REUSE_ACCOUNT )); then provision_command+=(--no-reuse-account); fi
  "${provision_command[@]}"
fi

python3 "$SCRIPT_ROOT/scripts/install-support.py" metadata "$AGENT_INSTALL_HOME" "$version" "$digest" "$BINARY_SOURCE" "$REPOSITORY" "${SOURCE_REF:-$RELEASE_TAG}" "$SOURCE_COMMIT" "$SOURCE_DIRTY"
unit_name=$(python3 "$SCRIPT_ROOT/scripts/install-support.py" unit "$AGENT_INSTALL_HOME")
note "Installed $binary_version in $AGENT_INSTALL_HOME"

if (( INSTALL_SERVICE )); then
  command -v systemctl >/dev/null || die "systemd was not found. Files are installed; run the printed serve command manually or install on a systemd host."
  # SSH/automation shells can retain their bus address but omit the runtime directory.
  # Apply its conventional per-user location only to service-manager commands.
  service_command=(env "XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}" systemctl --user)
  unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
  install -d -m 0700 "$unit_dir"
  [[ ! -e "$unit_dir/$unit_name" ]] || die "A service with the new instance ID already exists; it was not overwritten."
  install -m 0600 "$AGENT_INSTALL_HOME/agent-instance.service" "$unit_dir/$unit_name"
  if (( ENABLE_LINGER )); then
    note "Enabling user services after logout and at boot for $AGENT_OWNER."
    as_root loginctl enable-linger "$AGENT_OWNER"
  fi
  if ! "${service_command[@]}" daemon-reload; then
    note "Files and service are installed, but this shell cannot reach your user service manager."
    note "Log in as $AGENT_OWNER, then run: systemctl --user enable --now $unit_name"
    exit 1
  fi
  "${service_command[@]}" enable "$unit_name"
  if (( START_SERVICE )); then
    "${service_command[@]}" start "$unit_name"
    ready=0
    ready_deadline=$((SECONDS + 30))
    while (( SECONDS < ready_deadline )); do
      "${service_command[@]}" is-active --quiet "$unit_name" || break
      if [[ -f "$AGENT_INSTALL_HOME/endpoints.json" ]] && timeout 3s "$AGENT_INSTALL_HOME/bin/agent-instance" status --home "$AGENT_INSTALL_HOME" --json >"$TEMP_DIR/readiness.json" 2>"$TEMP_DIR/readiness-error.txt"; then
        ready=1
        break
      fi
      sleep 1
    done
    if (( !ready )); then
      note "The new service needs attention. Inspect: journalctl --user -u $unit_name -n 60"
      exit 1
    fi
  fi
fi

note "Instance commands:"
printf '  %q status --home %q\n' "$AGENT_INSTALL_HOME/bin/agent-instance" "$AGENT_INSTALL_HOME"
printf '  %q acp --home %q\n' "$AGENT_INSTALL_HOME/bin/agent-instance" "$AGENT_INSTALL_HOME"
if (( INSTALL_SERVICE )); then
  note "  systemctl --user status $unit_name"
  (( ENABLE_LINGER )) || note "For startup after logout/reboot, explicitly enable lingering: sudo loginctl enable-linger $AGENT_OWNER"
fi

if [[ -f "$AGENT_INSTALL_HOME/endpoints.json" ]]; then
  python3 "$SCRIPT_ROOT/scripts/install-support.py" endpoints "$AGENT_INSTALL_HOME"
  if (( OPEN_BROWSER )) && [[ -t 1 && ( -n "${DISPLAY:-}" || -n "${WAYLAND_DISPLAY:-}" ) ]] && command -v xdg-open >/dev/null; then
    browser_url=$(python3 "$SCRIPT_ROOT/scripts/install-support.py" browser-url "$AGENT_INSTALL_HOME")
    if [[ -n "$browser_url" ]]; then
      nohup xdg-open "$browser_url" </dev/null >/dev/null 2>&1 &
      note "Opening setup in the local browser. The printed addresses also work from another reachable machine."
    fi
  fi
else
  note "The service has not started. Start it to allocate/persist the port and print the HTTPS connection URLs:"
  printf '  %q serve --home %q\n' "$AGENT_INSTALL_HOME/bin/agent-instance" "$AGENT_INSTALL_HOME"
  printf '  %q endpoints --home %q\n' "$AGENT_INSTALL_HOME/bin/agent-instance" "$AGENT_INSTALL_HOME"
fi
if [[ "$DEFAULT_BACKEND" != none ]]; then
  note "The agent's runtime is installed. Sign in as $AGENT_OWNER to choose its identity and finish setup."
  note "Setup shows whether its private account store is ready or offers Connect ChatGPT. No previous conversations, configuration or other agent state was copied."
else
  note "The agent has no backend (--backend none). Configure a custom ACP backend in Advanced settings before enabling work."
fi
