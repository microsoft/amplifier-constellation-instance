# Agent Instance 0.12.0

New installations pin Constellation 0.8.0. Each installation continues to own its
runtime files, provider credentials, profiles and history. Updating an instance
does not update other agents on the same machine.

The Work dashboard now lets an owner inspect and edit acceptance criteria,
attach delivered artifacts and versions, record independent review, and record
requester closure. These controls use the same authenticated completion APIs
available to CLI clients and other agents. A successful tool call or a builder's
statement alone does not mark work accepted. The fleet dashboard can open the
same responsibility in this instance's UI.

Model & capabilities includes PDF input and an advanced native-computer binding.
PDF support depends on the provider protocol and selected model. Computer use is
opt-in, needs an explicitly configured MCP environment, and requires approval
for every batch; saving its configuration does not grant unattended control.
OpenAI/Azure Responses are the supported native computer protocols in 0.8.0.

The pinned runtime preserves computer action cards and screenshots across
reconnects, without replaying side effects. Observe-only watch evaluations no
longer consume evidence needed by a later live poll. Recheck an older still-armed
watch when promoting it after a prior shadow evaluation; the upgrade cannot
reconstruct missing provenance from the older cursor.

Validation includes actual-service browser checks for completion forms,
persisted drafts, programmable view controls, narrow layouts and provider
settings, alongside the Rust and installer suites. Release artifacts identify
the exact source commit and checksums. Signing/notarization and physical-device
microphone verification remain separate deployment requirements.
