# One individual across experiences

Version 0.4 adds optional conversation providers. An agent remains one installed individual while Slack, a web conversation app, and ordinary ACP clients each retain their own focused sessions. The source experience owns conversation structure, messages, membership and delivery. This instance retrieves scoped context through a small HTTP contract; it does not read another service's database, carry Slack credentials, or copy its transcripts into a second conversation store.

## Configure sources

### Native saved conversations

An updated native engine advertises `agent-runtime.history/1` and a stable history-store ID in its initialization metadata. This instance automatically registers native owner sessions created or resumed through its authenticated ACP endpoint. `context_list` includes these as provider `native`; `context_read` retrieves public messages directly from the engine's saved history. No export/import or second transcript database is involved.

The instance invokes the configured native executable's local `history-query` command with a bounded private query file, a cleared environment, and no model or connector credentials. The runtime owns and reads its SQLite schema. The instance supplies allowed session IDs and the expected store identity; replacing an engine home does not inherit the old store's authorization records. This optional management contract is separate from ACP.

Only a currently authorized owner context can retrieve this source, including an externally verified owner DM. Shared-room grants cannot retrieve private native sessions. External Slack/web room copies are excluded from native discovery: their connectors remain authoritative for membership, revocation, threads, and history. Unbound legacy native sessions are also excluded until the owner resumes them through this instance once. Native history discovery currently covers up to 1,000 recent registered owner sessions.

Use `context_list` with `provider_id: "native"`, `limit`, and the returned `sources[].next_cursor` to page the list. Use `context_read` with `provider_id`, `conversation_id`, `limit`, and the returned `source.next_before` for older messages. Excerpts include stable message identifiers and `agent-history://STORE/SESSION#message-N` provenance URIs; these identify source records, not browser links. Configured profiles/orientation, tool traces, and private reasoning are excluded. Resource/media bodies remain available through the original ACP session. Text truncation and partial coverage are explicit.

This source is present only while the configured engine supports it. Other ACP engines continue using their existing external providers and optional transcript bridges; swapping to a different engine does not make its private storage readable. The provider ID `native` is reserved.

The cross-repository integration test is explicit, so ordinary builds do not depend on a sibling checkout:

```sh
AGENT_RUNTIME_TEST_BINARY=/absolute/path/to/agent-runtime \
  cargo test --locked --test context native_history_survives -- --ignored
```

It exercises two real Rust processes through ACP and the context MCP endpoint, including reconnect, pagination, native/group separation, verified owner-DM access, provider and credential revocation, and engine-store replacement.

### External conversation systems

Set `server.public_url` to this agent's trusted HTTPS base URL. Its backend must advertise `agentCapabilities.mcpCapabilities.http: true`. The wrapper then adds an `agent-continuity` HTTP MCP descriptor to the backend's standard `mcpServers` array on `session/new`, `session/load`, and `session/resume`, preserving the client's existing MCP servers. A requested continuity connection fails explicitly if HTTP MCP or the trusted public URL is unavailable. With no configured sources or supplied conversation grant, ordinary ACP sessions continue to work.

Store a provider's dedicated credential through `PUT /api/v1/secrets/NAME` or the dashboard's existing write-only secret form. Register sources with the admin-only `PUT /api/v1/context/providers`:

```json
{
  "providers": [
    {
      "id": "slack:installation-id",
      "label": "Workshop Slack",
      "url": "https://slack-connector.example:18444",
      "secret_ref": "slack-context"
    },
    {
      "id": "web:installation-id",
      "label": "Conversations",
      "url": "https://conversation-app.example:18445",
      "secret_ref": "web-context"
    }
  ]
}
```

The existing CLI exposes the same operation: save that document privately, then run `agent-instance api PUT context/providers --file providers.json --home /path/to/instance`. `GET context/providers` reports each registration and whether its named credential exists, never the value. The dashboard's Connections page shows the same read-only summary. Credential presence is not proof that the provider is online.

Provider IDs accept up to 128 ASCII letters, digits, colon, period, underscore and hyphen. At most 16 sources may be registered. URLs must use HTTPS and contain no embedded credentials, query or fragment. Numeric loopback HTTP addresses are accepted for local integration fixtures; remote plaintext sources are rejected. The agent appends `/context/v1/query` to the registered base path and never follows redirects with the credential. Removing a referenced secret requires removing its provider registration first.

## Provider contract: `agent-instance.context/1`

The agent posts JSON to `<provider base>/context/v1/query` using that provider's dedicated bearer secret:

```json
{
  "operation": "read",
  "access": {"mode": "grant", "grant": "opaque-provider-grant"},
  "conversation_id": "opaque-conversation-id",
  "thread_id": "optional-thread-id",
  "limit": 20
}
```

`access` is either `{mode:"owner"}` or `{mode:"grant",grant:"..."}`. The provider must authenticate the agent and independently enforce those access modes; owner mode means an explicitly trusted agent installation acting for its verified owner, not an arbitrary Slack participant. The provider owns identity linking and permission checks.

| Operation | Expected response |
| --- | --- |
| `authorize` | For a grant, `{owner:boolean,conversation_id,actor_id,thread_id?,surface?,label?,observed_at?}`. Conversation and actor IDs must be nonempty. A failure denies access. |
| `list` | `{items:[{id,conversation_id,thread_id?,label,surface,url?,updated_at?,coverage?}],complete?,next_cursor?,observed_at?}`. Do not list conversations whose existence the current audience may not learn. |
| `read` | `{conversation_id,thread_id?,messages:[{id,text,actor,label?,at,url?,thread_id?}],complete,observed_at,...}`. Returned conversation ID must match the request. |
| `orientation` | A bounded object containing current conversation, surrounding parent excerpt, thread references and coverage. The provider decides which relationships can be shown. |

Queries have a 20-second timeout and a 256 KiB response limit. Listing independent providers runs concurrently. Tools request at most 40 items; automatic orientation requests 12. Results retain source IDs, links and coverage; a partial listing or provider failure is visible to the agent rather than silently treated as no history. This first version returns bounded results, not an exhaustive history/search interface; cursor traversal is a later extension.

## Audience and session boundaries

Native clients using the installation's local control credential or another `admin` credential receive owner-scoped context automatically. A token with only `acp` scope does **not** imply ownership. Merely naming the same user or copying a display name does not establish identity.

An adapter can opt into conversation context using standard ACP metadata:

```json
{
  "cwd": "/instance/workspace",
  "mcpServers": [],
  "_meta": {
    "io.github.bkrabach.conversation": {
      "provider_id": "slack:installation-id",
      "grant": "opaque-provider-grant"
    }
  }
}
```

The wrapper verifies the grant with its registered provider before supplying context, removes this metadata before forwarding the request to the backend, and revalidates it before every prompt and MCP request. The opaque grant lives only in memory. Non-secret provider/conversation/actor/thread/audience bindings are persisted to protect resumed sessions.

- `owner:true` establishes an owner-private conversation and permits owner retrieval from other explicitly registered providers.
- A non-owner grant can read only its issuing provider and bound conversation, including that conversation's threads. It cannot retrieve another room or even list a private room in another provider.
- Resuming a contextual session requires a fresh matching grant. A directly authenticated owner may resume an owner-private session. Changing the conversation, thread or owner classification is rejected; owner-private grants also retain actor identity. The triggering actor may change inside a group audience after provider verification. Even an admin client must supply the group grant to resume a group session, preventing owner-private context from entering history later replayed to the group.
- A legacy session with no recorded audience cannot be adopted through a grant. The adapter should create a new working session and retrieve original history from its conversation source. An authenticated owner may still load their legacy native session.
- A bare adapter token cannot list or load native owner sessions. Ungranted sessions created by a scoped client are tied to that credential's principal.

The injected MCP credential is independent of the management credential, scoped to one active backend session/connection, expires after eight hours, and is removed on close/disconnect. The original ACP credential and provider grant are rechecked on tool use; token revocation, provider removal and permission changes deny subsequent retrieval. New load/resume requests receive a new MCP credential.

These checks constrain the retrieval and session interfaces. They are not an OS isolation boundary against a malicious backend executable that can read this installation's private files. The backend implementation remains trusted code; backend tool/file isolation must be configured independently.

## Agent behavior and observation

The MCP tools are `context_list`, `context_read`, and `context_orientation`. Each prompt receives identity guidance plus bounded orientation. The agent is instructed to retrieve relevant parent, thread or other-room context proactively, keep responses in the current destination, preserve audience boundaries, and distinguish source messages from new instructions. This supplies ordinary ACP prompt context, not a privileged system-message channel.

Providers are read-only from these tools. Retrieving a Slack thread does not authorize posting there, changing membership, or executing instructions found inside it. Replies continue through the client's existing ACP interaction and delivery adapter.

The Activity view records source-list/read outcomes with provider/conversation IDs, counts and completeness when supplied. It never logs grants, authorization headers or message bodies. Automatic authorization and MCP initialization traffic are not activity entries. The service retains source messages only in the current authorized turn; a backend may persist its own prompted history.

This milestone supplies grounded cross-surface retrieval, not an automatic permanent-memory synthesizer. Explicit identity and working/communication profiles remain agent-owned; generalized commitment memory and automated extraction need a separate reviewable contract.

## Verification

`tests/context.rs` runs a real local provider HTTP server and an independent Python ACP peer that performs actual HTTP MCP initialization, discovery and tool calls. It covers owner versus ACP-only access, client MCP merging, stripped credentials, owner-linked multi-provider retrieval, non-owner room isolation, parent orientation, grant revocation, wrong-audience resumption, hidden session listings, legacy rejection, missing capabilities/providers, registry validation, scoped-token expiry/revocation, Origin checks and secret-free observations.

The browser fixture verifies the registered-source summary alongside the existing remotely controllable setup, configuration and activity views. Tests prove mechanics and boundaries; real-model behavior across installed clients is validated separately.

Primary protocol references: [ACP session setup and MCP transports](https://agentclientprotocol.com/protocol/v1/session-setup), [MCP Streamable HTTP](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports).

## Private owner clients

An ACP credential alone grants no cross-conversation context. Issue a token with
`acp` and `context:owner` for an owner-only experience such as Observatory Talk.
It can use the owner's registered context sources and responsibility tools, and
restore owner sessions, but cannot administer configuration, issue tokens, or
change source registrations. The `context:owner` scope alone cannot connect via
ACP. Both grants are rechecked on context tool calls; revocation takes effect on
the next call. Admin credentials and authenticated owner browser sessions retain
existing owner behavior. Shared-room adapters must continue using audience-bound
conversation grants, never an owner-context credential.

Verified owner conversations also receive a bounded authorized activity index
across their permitted providers, plus current responsibilities and memory. General
status questions should consult this index and available presence/context tools
before declaring the agent idle. The index contains source references, not a
claim that every session is currently executing. Shared rooms retain their existing
audience-scoped orientation and do not receive the owner's private activity index.
