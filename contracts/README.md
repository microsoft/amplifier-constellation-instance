# Instance contract map

The [Instance vision](../docs/VISION.md) describes intended value. Shared service
roles, information ownership and interface responsibilities live in the
[family agreement](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/family.v1-candidate.md).
Concrete Instance definitions remain in their existing homes below; this map
adds no competing specification.

| If your change affects… | Read the local contract |
| --- | --- |
| ACP relay, backend capability discovery, context insertion or remote framing | [ACP boundary](../docs/acp-boundary.md) |
| Management resources, authentication, schemas or revisions | [Management API](../docs/api.md) |
| Source authorization, conversation bindings or context retrieval | [Conversation continuity](../docs/conversation-continuity.md) |
| Installation-owned HTTP MCP connections and trust | [Tool connections](../docs/tool-connections.md) |
| Optional engagement and prompt admission | [Attention](../docs/attention.md) |
| Accepted responsibilities, evidence, review or closure | [Responsibilities](../docs/responsibilities.md), [completion](../docs/completion-contract.md) |
| Background execution and its recovery | [Background work](../docs/background-work.md) |
| Attributed memory or identity presentation | [Memory](../docs/memory.md), [agent identity](../docs/agent-identity.md) |
| Fixed connector actions and saved connection lifecycle | [Service connections](../docs/service-connections.md) |
| Enrolled updates, lifecycle participants or rollback | [Updates](../docs/updates.md) |
| Offline installation backup, restore or clone | [Checkpoints](../docs/checkpoints.md) |
| Engine settings and account setup | [Native configuration](../docs/native-configuration.md) |
| Supported administration-client state and embedding | [UI control contract](../docs/ui-contract.md) |

The [optional interface map](../docs/optional-experience-contracts.md) helps clients
find relevant definitions. ACP and MCP retain their upstream meanings. Family-wide
SDK, input, delivery, work-authority and installation promises belong in the
[umbrella contract map](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/README.md),
not a duplicate local policy. Connector-specific payloads belong to the connector.

Guides and reference screens explain or exercise these interfaces; they do not
add commitments or prescribe future product UX. When a promise changes, inspect
its linked examples in the same change. Select meaningful checks from
[validation](../docs/validation.md) and the affected consumer rather than treating
this index as a release or gap report.
