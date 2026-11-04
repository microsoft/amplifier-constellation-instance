# Development setup

In **Connections → Work with code**, choose **Connect GitHub**. An owner can reuse
the GitHub CLI account already signed in on the installation's computer, or enter
a GitHub token privately. For a fresh computer without that sign-in, install the
GitHub CLI and run `gh auth login` there, or use a token. This first version does
not provision a GitHub OAuth application or offer a browser device-code flow.

The setup checks the official GitHub MCP endpoint, authenticates with `get_me`,
and verifies that repository and pull-request tools are advertised. It does not
write a repository during verification. It preserves unrelated tools, agent
identity, profiles, workspace contents, and conversation bindings.

The connected credential determines repository access. Selecting a small tool
set is **not** a repository authorization boundary. Use a repository-restricted
credential where required. The selected tools can inspect code, create branches,
write files, and open pull requests; they do not include merge, delete, or repository
administration operations.

The UI explicitly enables tools for this installation's local ACP attach clients.
Existing scoped local credentials keep their token identity, audience grants, and
expiry. Other remote credentials remain unchanged. Local clients without a scoped
credential receive a separate `acp`, `context:owner`, `tools:use` credential; the
management credential is never given tool scope. Active sessions reconnect to load
the updated tools. Generic clients must still support ordinary ACP HTTP MCP servers.

## Programmatic interface

```sh
agent-instance --home ./my-agent development status
agent-instance --home ./my-agent development connect-github --enable-local-clients
agent-instance --home ./my-agent development check-github
agent-instance --home ./my-agent development disconnect-github
```

`connect-github --credential-file /private/github-token` uses a private token file
instead of the host GitHub CLI. The token is never a command-line argument. The
same owner actions are `GET /api/v1/development`, `POST/DELETE
/api/v1/development/github`, and `POST /api/v1/development/github/check`.
Connection changes require the current `expected_revision`. POST accepts optional
`credential` and `enable_local_clients` fields. All operations require admin;
browser mutations also require the normal origin/CSRF checks. OpenAPI describes
the bodies. Browser semantic actions use the `development.*` prefix and view
control can open the Connections route and `development-github` modal. Credentials
are never saved in view drafts or returned in status.

Credentials are stored privately per installation. Reconnect replaces the saved
credential after re-verifying it. This version does not automatically renew a copied
GitHub credential; **Check access** detects a rejected account and **Reconnect**
repairs it. Disconnect removes the tool connection and invalidates its active
backend connections; it does not sign the owner out of GitHub or revoke the source
credential at GitHub. Prior private secret values remain in the owner's secret
store until removed there.

## Readiness means evidence

A configured/verified GitHub connection does not establish checkout, dependency
installation, test execution, repository write permission, or successful PR creation.
Native local commands keep workspace file isolation. With an updated Linux runtime,
**Model & capabilities → Command network access** can separately use the
installation’s network (including its container firewall). Networking stays off
until the owner enables it. The same `command_network` setting is exposed through
the native configuration API, CLI, and semantic `native.command_network` UI draft.
The enclosing environment determines reachable addresses; this is not a separate
internet-only firewall. Credentials remain managed by their respective tools.
`ready_to_develop` and `environment.build_verified` remain false until a future
project/environment acceptance check can establish them. Status describes that
limit instead of presenting account connectivity as full development readiness.
