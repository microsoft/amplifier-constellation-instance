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

When that capability also advertises `ordinaryPrompts:true`, an ordinary
`session/prompt` may supply the same metadata with a stable `inputId`. A supplied
steering `inputId` must agree. The host records the reference before forwarding;
it is authenticated caller provenance, not independent verification of message
text. In a source-bound context, a short prompt block makes this handle available
to the engine's existing work tools. No work is created automatically.

`promptRetry:"inspectOnly"` means a retained ordinary input cannot be forwarded
again: load the original authorized session and inspect its receipt. An earlier
steering input may become an ordinary prompt once only after explicit
`promptRequired` or `notAccepted`, with no observed incorporation and the same
source revisions. Unknown or incorporated inputs cannot use that fallback.
An authorized same-room group handoff may change the current participant, while
the input keeps its original attribution. Owner-private handoffs also bind the actor.
Explicit maintenance uses its own receipt and cannot carry source-delivery metadata.

Attention `context.messages[]` may include both `revision` and `occurred_at` (snake_case, original milliseconds) only when this capability is negotiated. Steering and receipt-query sources instead use `occurredAt` (camelCase). Instance adds its observed delivery facts to attention evidence only for matching source ID, revision and original time. A changed revision or reminder must not reuse an old delivery fact. A receipt query takes `sessionId` and the same bounded `sources`. Results distinguish received, submitted, remoteQueued, incorporated, notAccepted, promptRequired and unknown. No adapter-supplied claim may manufacture incorporation. Missing receipt capability or an ambiguous transport outcome remains unknown. Receipt evidence is not task completion, public delivery, or permission; it does not automatically suppress another prompt. A new source revision, new question or reminder remains eligible for social judgment. Preserve final-answer freshness checks.

With `workReferences:true`, each receipt also includes `work`: references to
responsibilities explicitly linked through [work creation](responsibilities.md#agent-access-through-ordinary-mcp).
Each reference reports its ID, owner agent, revision, current state and input ID.
A `proposed` record is not acceptance; an ordinary prompt `response` records only
the terminal kind/code or stop reason, never work completion. References include
all matching retained inputs for that source revision, subject to the advertised
limits. Existing work controls inspect, amend and cancel the responsibility.

Delivery inspection remains scoped to the authenticated principal and loaded
session: insertion prunes records older than 24 hours, the journal holds at most
8,192 records and queries examine the latest 256. Work references use immutable
provenance on existing work records, checking current conversation access; they
can survive the delivery journal without reconstructing a missing delivery fact.
Queries examine at most 1,000 matching work records and return up to 64 references
per source, with `workCoverage:"partial"` when truncated. Missing evidence stays
unknown. This profile does not promise cross-session recovery or unlimited history.

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
