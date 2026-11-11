# Optional interfaces alongside ACP

This is a navigation aid for clients composing Instance with an engine or an
experience. The [ACP boundary](acp-boundary.md) defines the standard relay and
Instance's optional behavior. The [family interface agreement](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/family.v1-candidate.md#interface-responsibilities)
assigns ownership; this page adds no wire fields or requirements.

## Find the serving contract

| Interface | Definition and owner |
| --- | --- |
| Ordinary ACP initialization, sessions, input, updates and callbacks | [ACP boundary](acp-boundary.md), the upstream protocol and the selected backend's advertised capabilities |
| `agent-instance.wss/1` remote framing | [ACP boundary](acp-boundary.md) |
| `agent-instance.management/1` descriptor and management operations | [Management API](api.md) |
| `agent-instance.context/1` conversation context and bindings | [Conversation continuity](conversation-continuity.md) |
| `com.microsoft.amplifier.attention` version 1 engagement/admission | [Attention](attention.md) |
| `agent-instance.work/1` responsibility records | [Responsibilities](responsibilities.md) |
| `agent-instance.completion-contract/1` acceptance briefs and evidence | [Completion](completion-contract.md) |
| Saved connection and update controls | [Service connections](service-connections.md), [updates](updates.md) |

Use the serving contract for version discovery, enabled-state checks, authority,
errors and fallback behavior. Application profiles and metadata do not become
standard ACP merely because they accompany an ACP exchange. The [upstream
extensibility specification](https://agentclientprotocol.com/protocol/v1/extensibility)
and the [shared SDK agreement](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/protocol-sdk.v1-candidate.md)
define that boundary.

## Follow source and experience ownership

The context contract owns how Instance validates a provider binding and supplies
authorized context. The source provider owns its membership, source IDs and
query payloads. A connector owns its platform input and delivery conventions;
those detailed definitions belong in its repository, located through the
[family registry](https://github.com/microsoft/amplifier-constellation/blob/main/family.toml).
This page does not reproduce Slack, Lab or other experience response envelopes.

Use the [interaction agreement](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/experience.v1-candidate.md)
for the shared meanings of receipt, response, delivery and completion. A client's
supported composition can be smaller than this map; inspect its actual interfaces
rather than inferring capabilities from a shared product name.
