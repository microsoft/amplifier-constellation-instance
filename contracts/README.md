# Instance agreements and interfaces

Start with the [vision](../docs/VISION.md) and the local
[hosting agreement](hosting.md). Shared promises live in the
[Constellation contracts](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/README.md);
Instance links them rather than defining another family policy.

## Technical interfaces

These documents specify operations, schemas and compatibility. Read the boundary
you will use or change; they are not a mandatory onboarding sequence.

| Boundary | Definition |
| --- | --- |
| ACP attach, remote framing and host additions | [ACP boundary](../docs/acp-boundary.md) |
| Management discovery, authorization and revisions | [API](../docs/api.md) |
| Source bindings, context and exact tool trust | [Conversation continuity](../docs/conversation-continuity.md), [tool connections](../docs/tool-connections.md) |
| Optional participation and in-turn observations | [Attention](../docs/attention.md), [maintenance/receipts](../docs/harness.md) |
| Responsibility records and optional completion policy | [Responsibilities](../docs/responsibilities.md), [completion](../docs/completion-contract.md) |
| Separately authorized background execution | [Background work](../docs/background-work.md) |
| Attributed memory and identity | [Memory](../docs/memory.md), [identity](../docs/agent-identity.md) |
| Connection/update lifecycle and offline restore | [Connections](../docs/service-connections.md), [updates](../docs/updates.md), [checkpoints](../docs/checkpoints.md) |
| Engine configuration and account setup | [Native configuration](../docs/native-configuration.md) |
| Reference-client controls, view state and embedding | [UI interface](../docs/ui-contract.md) |

Capability discovery and enabled-state checks determine which optional surfaces
are usable. Their names do not extend standard ACP. Connector payloads remain
with the connector. Follow [validation](../docs/validation.md) and the relevant
consumer when changing a boundary; exact technical promises remain in the linked
specifications.
