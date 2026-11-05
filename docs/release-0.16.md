# Agent Instance 0.16.0 candidate

This candidate keeps follow-up input inside the active agent session when the
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
cannot consume the restored offer twice.

The native runtime pin in this candidate remains Constellation 0.9.0 pending its
separate coordinated release. That engine does not implement the new native
steering path. A steering-capable backend and a client that preserves ordered
input are required to exercise this feature. Each installation continues to own
its runtime files, credentials, history and updates independently.

Validation includes 127 Rust tests, with one existing opt-in native-runtime
history test excluded from the default suite. Real-subprocess fixtures exercise
pending approval continuity, source/audience checks, receipt forwarding, prompt
completion races, ticket expiry/revocation and unchanged admission budgets.
Clippy with warnings denied and formatting checks pass. Five new relay
regressions fail against the previous boundary; the completion-race regression
also fails before the negative-acknowledgment repair. These are protocol-fixture
results, not live-provider or fleet-deployment claims.

See [ACP boundary](acp-boundary.md), [attention](attention.md), and the family
[input continuity contract](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/input-continuity.v1-candidate.md).
Publication, platform CI and installation rollout are separate release gates.
