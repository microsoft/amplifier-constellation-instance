# Tools that follow the agent

An owner can configure optional HTTP MCP connections once on an Instance. The same
connections are supplied to its ACP backend through Slack, a browser, an editor,
or another experience when that client's ACP credential explicitly has `tools:use`.
No shared tool hub or particular agent implementation is required. The backend
must advertise HTTP MCP support. Its own tool policy and the tool server's
repository, resource, and execution permissions still apply.

Under Configuration → Advanced settings, edit **HTTP MCP connections**. The same
field is available as `config.tool_connections` in the configuration API/CLI:

```json
{
  "tool_connections": [
    {
      "name": "project-tools",
      "url": "https://tools.example/mcp",
      "header_secrets": {"Authorization": "project-tools-auth"}
    }
  ]
}
```

Store the complete header value, such as a service-issued bearer authorization,
as the named secret through Access & credentials or `PUT secrets/{name}`. Never
paste credential values into configuration or browser drafts. Only references
are included in config/status. The instance passes resolved headers directly to
the trusted ACP backend; it does not put them in prompts, events, or saved
conversation records. As with provider credentials, use a trusted backend and a
tool-server credential restricted to the intended work.

Create an ACP credential with `acp` and **Use connected tools** (`tools:use`). Add
`context:owner` only when owner-level conversation context is also intended.
Existing ACP credentials, the local control token, `context:owner`, broad
`admin` access, and a browser sign-in do not implicitly enable connected tools.
This separates managing an installation from giving its backend tool authority.

For a local connector, store that scoped credential in an owner-only file and
launch `agent-instance --home <installation> acp --token-file <private-file>`.
The file keeps the bearer value out of process arguments and connector
configuration. On Unix it must be owned by the invoking account with no group
or other access (for example mode `0600`); symlinks are rejected. The existing
local control token remains unchanged and does not gain tool authority.
Alternatively, explicitly save the scoped token as `secrets/acp-token` in a
local attach home. That file becomes the default for `acp` from that home,
allowing already-configured clients to retain their launch command and session
bindings. An explicit token or token-file takes precedence. An invalid default
ACP token fails rather than falling back to the more privileged control token.
Without this optional file, existing local attach behavior is unchanged.

On `session/new`, `session/load`, and `session/resume`, Instance adds ordinary
ACP HTTP MCP descriptors. Client-provided servers remain available, but cannot
replace installation-owned names or endpoints. The combined limit, including
continuity tools, is eight servers. A session that used these tools requires the
explicit scope again when resumed or addressed; an owner-context credential
alone cannot load it.

Removing or replacing tool settings, rotating a referenced secret, expiring or
revoking the ACP credential closes affected backend connections. Checks happen
before forwarded frames and at most every half-second while idle. Reconnect to
use updated settings. Other ACP connections are unaffected. This stops the
managed backend; revoke credentials at the tool service as well if they may have
been copied outside that trusted process. Already completed external actions are
not undone.

Endpoints must be HTTPS or literal loopback HTTP, with no embedded credentials,
query, or fragment. Each connection supports eight secret-backed headers; header
values are bounded to 8 KiB each and 32 KiB combined across connections. Transport
headers are controlled by the MCP client. Redirect and MCP protocol behavior
remain the backend's responsibility, so configure the exact trusted endpoint.

The optional capability is `agent-instance.tool-connections/1`, advertised in the
management descriptor. These are installation controls using standard ACP MCP
parameters, not new ACP protocol methods.
