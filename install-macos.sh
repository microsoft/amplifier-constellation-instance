#!/bin/bash
# Native per-owner launch agent. Every installation owns its executable and data.
set -euo pipefail
[[ $(uname -s) == Darwin ]] || { echo "Use install.sh on Linux or install-windows.ps1 on Windows." >&2; exit 1; }
SCRIPT_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
ACTION=install
AGENT_DIR="$HOME/Library/Application Support/AgentInstance/first-agent"
LOCAL_BINARY=""
EXTRA=()
SKIP_PREREQUISITES=0
while (($#)); do
  case "$1" in
    install|update|rollback|inspect) ACTION=$1; shift ;;
    --home) AGENT_DIR=$2; shift 2 ;;
    --binary) LOCAL_BINARY=$2; shift 2 ;;
    --skip-prerequisites) SKIP_PREREQUISITES=1; shift ;;
    --name|--bind|--port|--release|--engine|--runtime-binary) EXTRA+=("$1" "$2"); shift 2 ;;
    --no-service|--no-browser|--skip-runtime) EXTRA+=("$1"); shift ;;
    --help|-h)
      echo 'Usage: ./install-macos.sh [install|update|rollback|inspect] [--home FOLDER] [--binary FILE] [--engine native|codex] [--runtime-binary FILE] [--name NAME] [--bind ADDRESS] [--port PORT] [--release ID] [--no-service] [--no-browser] [--skip-runtime] [--skip-prerequisites]'
      exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
  esac
done
[[ $(id -u) != 0 ]] || { echo "Install as the ordinary account that will own the agent." >&2; exit 1; }
if ! xcode-select -p >/dev/null 2>&1; then
  ((SKIP_PREREQUISITES)) || xcode-select --install
  echo "Complete Apple's Command Line Tools installation, then run this command again." >&2
  exit 1
fi
command -v python3 >/dev/null || { echo "Install Python 3 (for example: brew install python) and retry." >&2; exit 1; }
if [[ $ACTION == install || $ACTION == update ]] && [[ -z $LOCAL_BINARY ]]; then
  if ! command -v cargo >/dev/null; then
    if [[ -x "$HOME/.cargo/bin/cargo" ]]; then
      export PATH="$HOME/.cargo/bin:$PATH"
    else
      ((SKIP_PREREQUISITES == 0)) || { echo 'Install Rust or pass --binary.' >&2; exit 1; }
      RUSTUP_INSTALLER=$(mktemp -t agent-instance-rustup)
      trap 'rm -f "$RUSTUP_INSTALLER"' EXIT
      curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o "$RUSTUP_INSTALLER"
      sh "$RUSTUP_INSTALLER" -y --profile minimal --no-modify-path >&2
      export PATH="$HOME/.cargo/bin:$PATH"
    fi
  fi
  (cd "$SCRIPT_ROOT" && cargo build --release --locked >&2)
  LOCAL_BINARY="$SCRIPT_ROOT/target/release/agent-instance"
fi
if [[ -n $LOCAL_BINARY ]]; then EXTRA+=(--binary "$LOCAL_BINARY"); fi
# macOS ships Bash 3.2, where an empty array trips nounset without this guard.
exec python3 "$SCRIPT_ROOT/scripts/native-install.py" "$ACTION" --home "$AGENT_DIR" ${EXTRA[@]+"${EXTRA[@]}"}
