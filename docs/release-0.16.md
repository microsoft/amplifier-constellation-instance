# Agent Instance 0.16.0

This release keeps follow-up input inside the active agent session when the
connected backend advertises the pre-standard `_session/steering` extension.
The Instance boundary applies the same identity/profile context, current
conversation authorization, tool-access checks and optional attention admission
used for an ordinary prompt. It preserves the original work lifecycle, pending
permission callbacks, source metadata and backend input receipts.

Steering does not start an extra task or authorize additional work. Requests use
`idleBehavior: "promptRequired"`; Instance never silently creates a fallback
prompt. Clients without this extension and engines without steering continue to
use ordinary ACP. The backend must advertise its capability; upgrading Instance
alone does not add steering to an older engine.

If the active prompt ends before an ambient follow-up can be injected, a definite
backend `promptRequired` response can restore that input's original attention
ticket for the client's deferred prompt. The source timestamp, original expiry,
audience, policy and claimed budget slot stay unchanged. Expired or revoked
offers, changed source evidence, explicit cancellation, failed responses and
unknown delivery never receive a new retry authorization. Repeated delivery
cannot consume the restored offer twice. This includes the real connector's
new-transport handoff: only a proven-unapplied ticket with a verified saved
source binding can survive disconnect. Reloading the same session as the same
principal with a fresh audience grant atomically consumes it once. Concurrent
reconnects, changed source/authority and missing proof cannot reuse that ticket.

Optional input-receipt reads and recovery acknowledgments require a loaded
session and current conversation/tool authorization. They cannot bypass a grant
revoked after session load and never start an additional work lifecycle.

The native model editor uses the installed engine's support catalog for model
suggestions and ChatGPT account choices. Provider defaults stage a compatible
current model only when explicitly selected; loading the page never migrates a
saved provider, model or authentication route. Engines that return validation
details during inspection allow retired settings to remain visible and repairable
while execution stays disabled until a valid configuration is explicitly saved.
Empty or malformed account/support intersections cannot choose an unsupported
model. Existing sessions and their original provider bindings stay intact.

The native runtime pin is [Constellation 0.10.0](https://github.com/microsoft/amplifier-constellation-runtime/releases/tag/v0.10.0),
with verified archive and binary hashes for Linux x64/ARM64, macOS ARM64 and
Windows x64. That engine implements a durable ordered input inbox, native
OpenAI Responses WebSocket steering and legal next-call input for other native
providers. A steering-capable client remains necessary. Slack 0.9.0 supports
ordered input; current Conversations and Observatory Talk wait until active work
finishes before accepting another browser message. Each installation continues
to own its runtime files, credentials, history and updates independently.

Validation includes 134 Rust tests, with one existing opt-in native-runtime
history test excluded from the default suite. Real-subprocess fixtures exercise
pending approval continuity, source/audience checks, receipt forwarding, prompt
completion races, real two-transport and concurrent reconnects, ticket
expiry/revocation and unchanged admission budgets.
Clippy with warnings denied and formatting checks pass. Five new relay
regressions fail against the previous boundary; the completion-race regression
also fails before the negative-acknowledgment repair. These are protocol-fixture
results, not live-provider or fleet-deployment claims.
Native-account API and real-browser fixtures additionally cover retired-setting
repair, explicit provider defaults, supported-account filtering and unchanged
saved settings until an owner saves.

See [ACP boundary](acp-boundary.md), [attention](attention.md), and the family
[input continuity contract](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/input-continuity.v1-candidate.md).
Publication, platform CI and installation rollout are separate release gates.
