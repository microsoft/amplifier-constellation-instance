# Optional Linux installation containers

These administrator helpers put an already-configured agent installation and
its local connectors in one non-root Docker container. Each installation keeps
its own state, credentials, host login bridge, version and service units. There
is no shared enrollment service. This is an experimental deployment recipe,
not a new shared project-environment API or the default installer.

Validated on Ubuntu 24.04 ARM64 with Docker, AppArmor, systemd user services and
PAM. The existing native ACP runtime and third-party stdio ACP agents can run
inside the same kind of boundary. macOS and Windows deployment are not covered.

## What is isolated

Only the installation's selected directories are mounted. Mount executable and
release directories read-only. Give the container its own writable home; never
mount the owner's home, SSH directory, Docker socket or host password database.
The container drops all capabilities, enables no-new-privileges, has a read-only
root, private process/network namespaces, and CPU, memory, process and temporary
storage limits. Persistent installation data remains on the host for backup.

The custom seccomp and AppArmor profiles permit the native runtime's nested
bubblewrap user/mount namespaces. Docker's `systempaths` masking is disabled so
bubblewrap can mount a private `/proc`; AppArmor still denies sensitive kernel
paths. This deliberately differs from Docker's defaults. Containers share the
host kernel and are not a VM boundary. Review these profiles for your threat
model before installing. See [profile attribution](THIRD-PARTY-NOTICES.md).

The bridge network prevents peer traffic. `firewall.py` additionally blocks
host/private/link-local/metadata destinations except individually listed TCP
endpoints; public internet egress remains available. Install the rules **before**
starting containers. The scripts do not manage other Docker networks. Changes
are scoped to `172.30.240.0/24`; choose a different subnet throughout the recipe
if this conflicts with an existing network.

`bridge.py` runs as the owner outside each container. It checks only that owner's
PAM login and forwards only installer-selected fixed TCP endpoints. It accepts
no shell commands, target addresses or PAM-service selection from clients.
Credentials are neither saved nor logged by the bridge. Its private Unix socket
is available only to that installation. It is not a general host proxy.

## Prepare a host

Install Docker, AppArmor tooling, Python 3 and the distribution's `python3-pam`
package. The owner needs permission to administer Docker; that host permission
itself is privileged and must never be exposed to the agent. Keep user systemd
services available after logout using your host's normal service policy.

Build the image and persist the AppArmor profile:

```sh
docker build -t constellation-installation:local ops/container
sudo install -m 0644 ops/container/apparmor.profile /etc/apparmor.d/constellation-installation
sudo apparmor_parser -r /etc/apparmor.d/constellation-installation
```

Record the resulting image ID and use that immutable ID in private manifests.
Keep the bridge, firewall configuration and manifests outside repositories and
outside all container mounts. Persist `firewall.py` and its configuration in a
root-owned directory, invoke them from `constellation-container-firewall.service` after Docker starts and
before the owner's agent services start, and reapply after Docker restarts. Do
not add `--privileged`, host networking, or unrestricted private-network access
to work around connection failures.

The firewall configuration is an `agents` array; each entry has the container's
`ip` and optional `allow` pairs of destination IP and TCP port. The installation's
own host callbacks normally use private Unix routes instead. Keep both lists
narrow. Socket proxy destinations must never accept unauthenticated management
requests just because their callers are local.

## Move one installation

1. Verify it is idle, stop its original service units, and back up its complete
   state, history, credentials, certificates and unit definitions. A copy of a
   running SQLite file is not a sufficient backup.
2. Prepare a private manifest with the original command arguments, directories,
   published ports, mounted folders and any fixed loopback routes. Preserve
   agent IDs, conversation IDs, ACP authentication, and connector client scopes.
3. Keep TLS managed. Set `AGENT_INSTANCE_CERTIFICATE_NAMES` in the instance
   service's `env` to a JSON array of its published hostnames/IPs. They augment
   container-local names during normal renewal; the existing root CA remains
   unchanged. Do not pin a managed leaf as an operator certificate to avoid a
   name mismatch. External reverse proxies retain their own TLS configuration.
4. Bind each service to a reachable interface **inside** the container while
   preserving its original host publication. A local-only connector should still
   be published on host `127.0.0.1`, not all interfaces.
5. Disable incompatible in-app host-service updaters before migration. Executable
   mounts are read-only; this recipe uses owner-managed updates. Do not pass
   Docker or host systemd control through to the agent to make an updater work.
6. Run `python3 ops/container/install.py /private/path/installation.json` as the
   owner. Existing container names are rejected; replacement is explicit.
7. Verify login success/failure, authenticated ACP initialization, new and saved
   sessions, actual permitted tool calls, and each installed experience. Verify
   host SSH files, Docker controls and unapproved private endpoints are absent
   or unreachable. Restart the container's **systemd service** and verify its
   dependent agent/connector services recover.

The private manifest includes:

```json
{
  "container": "example-agent",
  "image": "sha256:REPLACE_WITH_BUILT_IMAGE_ID",
  "state": "/private/example-container",
  "ip": "172.30.240.10",
  "mounts": [
    {"path": "/private/example-agent"},
    {"path": "/private/example-agent/bin", "readonly": true}
  ],
  "ports": [{"host": 41000, "container": 41000, "bind": "127.0.0.1"}],
  "routes": [],
  "services": {
    "example-agent.service": {
      "argv": ["/private/example-agent/bin/agent-instance", "--home", "/private/example-agent", "serve"],
      "cwd": "/private/example-agent",
      "env": {"AGENT_INSTANCE_CERTIFICATE_NAMES": "[\"agent.example.test\"]"}
    }
  }
}
```

A fixed route specifies `listen` (the container loopback port) and `target`
(`[host IP, port]`). `hosts` optionally maps a hostname to a container-local
address, preserving the original TLS hostname while using that fixed route.
`env` sets container environment variables; service-specific `env` is preferred.
Private manifests can contain sensitive values and must remain mode 0600.

## Operations and rollback

The owner starts/stops/restarts `example-agent` through its original unit;
`constellation`-named container units are not required by the interface. The
container unit's dependencies bring the installation services back after a
restart. Use systemd, not `docker restart`, for coordinated restarts.

For upgrades, stop the installation, back up its state, replace only verified
host-side binaries, then restart and test. Image changes require replacing the
stopped container with the same mounts and manifest; never delete persistent
folders as part of that replacement. Keep a prior image and binary for rollback.

To revert containment, stop the installation/container, remove only the added
`50-container.conf` service drop-ins, restore original bind/TLS/update settings,
reload systemd, and start the original services. Keep backups until a real
conversation and saved-session recovery succeed. Disable the unused container
and bridge units after rollback. Do not remove shared Docker networks or flush
unrelated firewall rules.

GitHub is connected separately per installation through its development setup
(or the third-party agent's supported credential path). A shared GitHub account
still has that account's permissions; separate local copies are not separate
GitHub identities. Read/write tools require the appropriate ACP client scopes
and repository authorization. Existing nested command/network restrictions
continue to apply; containerization does not silently turn them off.

## Tests

```sh
python3 ops/container/test_container.py
cargo test --locked
```

The model-free tests cover mount validation, unit input handling, firewall
refresh ordering/private-address denial and service lifetime. Host acceptance
also requires the live checks above; unit tests do not certify containment.
