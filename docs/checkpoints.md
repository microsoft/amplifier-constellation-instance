# Whole-agent checkpoints

`scripts/checkpoint.py` captures one stopped installation and restores it into a
new folder, or creates a new individual from the same knowledge and work. It is
an offline administration command, separate from ACP and the running management
API. Python 3.10 or later is its only dependency.

The archive contains the instance settings, profiles and profile versions,
workspace files, saved conversations, native runtime state, durable work,
memories, and event history inside that installation. External workspaces,
connectors, services, and files referenced outside the installation are separate
dependencies. The command never registers a service, starts an agent, calls a
model, or modifies an existing destination.

## Capture and inspect

Stop the instance service and any clients running its native runtime directly.
Stop other programs writing to its workspace. Choose explicitly whether the
archive should carry this installation's managed credentials:

```sh
python3 scripts/checkpoint.py export \
  --home /path/to/agent \
  --output /path/to/checkpoints/agent-2026-11-02.zip \
  --secrets exclude
```

Use `--secrets include` when transferring the same individual's credentials to a
machine you control. The archive is private to the current operating-system
account but is **not encrypted**. `--secrets exclude` removes the managed secret
and TLS stores, provider account files, instance token and browser-login
records, and inline backend environment values/references. It also clears a
native ChatGPT credential-file reference, including an external one, and leaves
that provider awaiting new authentication.

This credential policy is not redaction of arbitrary workspace or conversation
content. Those files are preserved, including any sensitive material they
already contain. An unfamiliar ACP implementation may keep additional state
or credentials in its own locations; review that installation before sharing
an archive.

Binaries are omitted by default. Add `--include-binaries` for a same-platform
move when retaining the installed executables is useful. Binaries do not become
portable to another operating system or architecture by being archived.
Generated service registrations, endpoint files, lock files, process logs, old
backups, and old upgrade copies are omitted.

The JSON result gives the archive's SHA-256 hash. Retain it separately:

```sh
python3 scripts/checkpoint.py inspect \
  --archive /path/to/checkpoints/agent-2026-11-02.zip \
  --sha256 THE_HASH_FROM_EXPORT
```

Inspection verifies every payload file against the manifest. Supplying the
separately retained archive hash also detects a replaced archive/manifest pair;
the file hashes alone are integrity checks, not a signature identifying its
author.

## Restore the same individual

```sh
python3 scripts/checkpoint.py restore \
  --archive /path/to/checkpoints/agent-2026-11-02.zip \
  --home /path/to/restored-agent \
  --secrets exclude \
  --sha256 THE_HASH_FROM_EXPORT
```

The destination must not exist, even if empty. Restore preserves the instance
identity, native history identity, sessions, profiles, work, and memories. Use
`--secrets include` only with an archive that included credentials. Browser
login sessions are always cleared. Excluding credentials creates a fresh local
control token without printing it.

The restored service belongs to the OS account running the command. It remains
disabled, binds only to localhost with newly allocated ports when started, and
has no service registration. Background execution is paused and its prior
executable approval is cleared. Review pending work before separately enabling
it. Keep the original installation stopped before activating the restored
identity.

## Clone a new individual

```sh
python3 scripts/checkpoint.py clone \
  --archive /path/to/checkpoints/agent-2026-11-02.zip \
  --home /path/to/new-agent \
  --name "New project lead"
```

Clone preserves the profiles, workspace, conversations, work, memories, and
historical evidence but gives the instance and native history store new
identities. It removes inherited credentials and authorized conversation
bindings, clears the external context-provider registry, and disables active
assignments and peer relationship permissions. Native MCP grants and client
trust settings are cleared; tool use starts in `ask` mode. The new local
control token is its only initial instance token.

Historical records still identify their original actors. A clone does not
rewrite transcripts or make old statements into statements by the new agent.
Its existing work is retained for review, with background execution paused.
Reconnect experiences and grant this new individual its own access explicitly.

## Completing a move

Both commands write `checkpoint-result.json` in the destination and return the
same structured result on stdout. It lists identities, credential/binary
policy, external path dependencies, and remaining actions:

1. Install or verify the instance and runtime executables for the target OS and
   architecture. Ordinary installation/update tooling should be used only if it
   preserves this populated home; do not initialize over the restored files.
2. Reconnect provider credentials and any external workspace/bridge paths.
3. Configure the new address, certificates, ports, and experience connections.
4. Validate settings, register startup explicitly if wanted, and enable the
   agent. Review pending work and reapprove background execution separately.

Only recognized configuration fields are relocated: executable, runtime-home
and working-directory arguments, working roots, selected path environment
variables, TLS/account/bridge paths, native workspace roots, decision-service
CA paths, and saved session `cwd` metadata. `${AGENT_HOME}` remains portable.
External paths are reported rather than copied. Instructions, transcripts,
workspace text, provider continuation state, and arbitrary backend arguments
are not searched and rewritten. A changed provider account or continuation
fingerprint may require a new working session; the saved conversation remains.
The manifest retains the supplied source-home spelling when it differs from
its canonical path, so recognized paths through a parent alias such as macOS
`/tmp` can relocate without changing literal discussion text.

## Consistency and archive rules

Capture takes the installation and service locks, plus existing native session
locks. It refuses a running instance, a native session holding its lock, or an
active SQLite writer. It holds SQLite writer reservations across the capture
and uses SQLite's backup API, including committed WAL data. It checks that
ordinary files did not change while copying. These checks do not replace
stopping unrelated filesystem writers or a direct runtime that does not honor
the instance service lock.

Archives have schema `agent-instance.checkpoint/1`, per-file SHA-256 hashes,
sizes and modes, explicit credential policy, and source platform/architecture.
The supported bound is 100,000 paths and 20 GiB of uncompressed payload.
Paths must be portable relative paths. Symlinks, reparse points, hard links,
device files, sockets, unsafe Windows names, and case-insensitive collisions
are rejected rather than followed. Restore validates the entire archive before
extracting regular files into a private staging directory. Publication is
atomic and cannot replace an existing directory, including one created during
the operation.

Linux, macOS, and Windows have explicit no-overwrite publication and private
permission implementations. Twelve tests, including actual Rust service locks,
restored configuration and directory-alias checks, passed on Linux ARM64 and
native macOS ARM64 with Instance 0.11.0 and Constellation 0.7.0. The original
eleven cases also passed on native Windows x64; the Unix symlink fixture is
inapplicable there. See [validation](validation.md) for exact execution evidence.

## Validation

Run the synthetic fixtures without touching any live installations:

```sh
python3 -m unittest discover -s tests -p test_checkpoints.py -v
```

For actual Rust service-lock and configuration compatibility checks, build both
repositories and provide their binaries. The test uses disposable installations
with the deterministic mock provider, and starts only a localhost fixture:

```sh
CHECKPOINT_INSTANCE_BINARY=target/debug/agent-instance \
CHECKPOINT_RUNTIME_BINARY=../amplifier-constellation-runtime/target/debug/agent-runtime \
  python3 -m unittest discover -s tests -p test_checkpoints.py -v
```

The suite covers identity preservation, new clone identities, credentials and
conversation grants, workspace/profile/history preservation, literal transcript
paths, WAL snapshots, live service/session/writer refusal, tampering, traversal,
links, explicit CLI policy, and destinations that must remain untouched.
