# Agent Instance 0.11.0

New installations use **Constellation 0.7.0**, the independent Rust ACP engine.
Each individual keeps its own executable, settings, provider credentials,
workspace, saved conversations and stable identity. Native setup does not require
Codex or Node; another ACP implementation remains an explicit choice.

- Native saved conversations are discoverable through the authorized context
  layer. Private owner history stays separate from shared-room context. External
  conversation systems still use connectors.
- Whole-agent checkpoints preserve files, profiles, history, work and memories.
  Restore keeps the individual; clone creates a new identity and clears inherited
  credentials/context grants. Capture requires stopped writers and never
  overwrites a destination or registers a service.
- Optional completion contracts freeze the source-linked brief and exact
  artifact versions, require named peer reviews, and reserve closure for the
  authenticated requester. Execution receipts and review reports remain distinct.
- Native platform validation covers Windows x64 install/update/rollback and
  Linux/macOS/Windows checkpoint paths with real binaries. Disposable HTTPS
  validation covers the complete authenticated review/closure API flow.

The completion controls are currently API/CLI features; dedicated dashboard
controls and richer agent-driven scenarios remain to be built. Checkpoints are
offline owner operations. They are not encrypted and do not redact arbitrary
workspace or conversation contents. Mobile devices use the browser UI connected
to a host, not a background mobile runtime. Local Mac builds are not signed or
notarized, and successful owner-password sign-in on Mac/Windows is not established
by these token-authenticated fixtures.

These changes do not add private methods to ACP. Standard ACP over stdio remains
the interoperability boundary; management, authorized context, participation
metadata and remote framing are separately versioned optional contracts.

[Validation](validation.md) · [Native platforms](native-platforms.md) ·
[Checkpoints](checkpoints.md) · [Completion contract](completion-contract.md) ·
[Optional experience contracts](optional-experience-contracts.md)
