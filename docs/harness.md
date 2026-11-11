# Long work and conversation maintenance

Instance exposes optional maintenance and source-delivery profiles alongside its
[ACP boundary](acp-boundary.md). These technical definitions are not requirements
for every engine. Negotiate the installed capabilities; the umbrella owns
[work-continuity behavior](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/work-continuity.md).

## Execution and observations

An active prompt does not expire merely because its ACP connection has been
quiet. The configured connection idle limit applies between prompts. Backends
and task owners still enforce their own budgets; silence is not proof of progress
or failure.

The native engine keeps history separate from its model working window. Its
content-free report is available through `GET /api/v1/native-observations`
(`observe` scope). Configuration API/CLI controls expose connection silence,
whole-turn limits, compaction and caching; the administration UI uses them too.
The source-delivery journal is bounded and does not retain source text.

## Explicit maintenance

Use standard ACP `clientCapabilities.session.compaction` and `compaction_update` when supported. A completed compaction update does not itself settle `session/prompt`. Instances may preserve the exact advertised `/compact` command before prompt decoration. An opt-in instance capability under `io.github.bkrabach.agent-instance.maintenance` describes this route: `version:1`, `promptMetadata:"com.microsoft.amplifier.maintenance"`, `requiresAdvertisedCommand:true`. Clients send one `/compact` text block in `session/prompt`, with that metadata containing `version:1`, `operation:"compact"`, and a stable `operationId`. The session must be idle across all connections, and the operation ID cannot be reused. Query `_amplifier/maintenance_receipt` with `sessionId` and `operationId` to inspect settlement after a lost response. The Instance prevents competing input and denies effectful client callbacks until the matching prompt settles. A first `completed` compaction update is valid without an earlier `in_progress` update; a previously observed compaction ID cannot settle a new operation. The result metadata reports completed only after a matching successful compaction update AND successful prompt terminal; other outcomes remain failed/cancelled/unknown. A backend owns its own internal execution; this profile is not a general sandbox for arbitrary third-party implementations.

Adapters close their action authority before maintenance, never reserve a work action lease for it, and recheck existing assignment authority afterward. Native Runtime compacts its provider window automatically; it does not pretend to implement a backend command that it does not advertise.

## Source delivery facts

Instances may advertise `inputContinuity.sourceDelivery` version 1 with metadata `com.microsoft.amplifier.source-delivery` and receipts method `_amplifier/source_receipts`. This is optional, not standard ACP. On steering input the metadata contains `version:1` and up to 40 `sources`, each with `id`, opaque `revision`, and original `occurredAt` (milliseconds). The normal stable steering input ID and active-prompt/session binding still apply. Source IDs and revisions belong to the connector; Instance binds them to the authenticated session and audience.

Attention `context.messages[]` may include both `revision` and `occurred_at` (snake_case, original milliseconds) only when this capability is negotiated. Steering and receipt-query sources instead use `occurredAt` (camelCase). Instance adds its observed delivery facts to attention evidence only for matching source ID, revision and original time. A changed revision or reminder must not reuse an old delivery fact. A receipt query takes `sessionId` and the same bounded `sources`. Results distinguish received, submitted, remoteQueued, incorporated, notAccepted, promptRequired and unknown. No adapter-supplied claim may manufacture incorporation. Missing receipt capability or an ambiguous transport outcome remains unknown. Receipt evidence is not task completion, public delivery, or permission; it does not automatically suppress another prompt. A new source revision, new question or reminder remains eligible for social judgment. Preserve final-answer freshness checks.

## Boundaries and verification

Maintenance acceptance, a compaction notification, prompt settlement and external
work completion are separate facts. Verify idle-session exclusion, unavailable
commands, matching and reused operation IDs, lost acknowledgments, failed/cancelled
terminals and a completed notification without an earlier in-progress notification.
For source delivery, verify authenticated session/audience binding, original
revision/time, unsupported capability, unknown transport delivery and independent
receipt states. Neither profile permits replay of uncertain effects or elevates
retrieved evidence into participant instructions.

Implementation and fixtures: [maintenance](../src/maintenance.rs) and
[source delivery](../src/source_delivery.rs).
