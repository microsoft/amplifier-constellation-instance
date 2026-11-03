# Frozen briefs, artifact review, and requester closure

The optional `agent-instance.completion-contract/1` policy captures what was
requested, identifies exact delivered versions, records named reviewers' checks,
and requires requester acceptance before completion. Existing responsibilities
retain their review/verify/complete flow until a requester freezes a brief.

This is an instance management contract, not an ACP extension. It uses the
existing authenticated `POST /api/v1/work/{id}/commands` route and
`agent-instance api` CLI. Every command includes `expected_revision`; stale
writes fail. The dashboard's Work detail has **Set acceptance criteria**,
**Record delivered version**, **Record artifact review**, and **Accept and close**
controls. Each uses this same API and authenticated principal. Forms preserve
their ordinary drafts across reloads; submission checks the revision and exact
brief/artifact hashes captured when the form opened.

The semantic UI actions `completion.open` (work `id` and `command`) and
`completion.save`, with `completion.*` drafts, expose those controls to an
authorized agent or embedding experience. Opening a form does not approve it.

## Freeze the acceptance brief

```json
{
  "expected_revision": 1,
  "command": {
    "type": "freeze_brief",
    "acceptance_criteria": ["Deliver the agreed file.", "Check each item against its source."],
    "reviewer_actors": ["token:REVIEWER_ID", "token:SECOND_REVIEWER_ID"],
    "source": {"provider_id": "workshop", "conversation_id": "design", "message_id": "request-1", "url": "https://experience.example/design#request-1"}
  }
}
```

`source` is optional if the work already names a specific source message or URL.
Conversation-scoped work cannot link outside that conversation. The source is
an attributed reference; the store does not fetch or independently verify it.

The brief snapshots the title, goal, source, audience, assignment, template,
acceptance criteria, reviewer principals, and freezing principal. It receives a
version and SHA-256; previous briefs remain immutable in
`completion_contract.brief_versions`. A scope change makes the brief stale and
prevents starting work or recording review against it. Its current requester
must freeze a replacement.

The freezing principal becomes `completion_contract.requester_actor`: the
signed-in OS owner or an authenticated management token (`token:<token-id>`).
This does not infer a human identity from a conversation or display name.
Distinct reviewer token IDs likewise establish distinct principals, not proof
that different humans control them.

## Record the delivered versions

After normal acceptance/start, record a digest computed from actual bytes:

```json
{
  "expected_revision": 4,
  "command": {
    "type": "artifact_version", "artifact": "release-summary", "label": "Release summary",
    "uri": "https://artifacts.example/release-summary-v2.md", "version": "2",
    "sha256": "64_LOWERCASE_HEXADECIMAL_CHARACTERS"
  }
}
```

`artifact` is the stable local deliverable name. Another version with the same
name supersedes its review target while preserving earlier evidence; different
names identify separate deliverables. The current set is returned in
`completion_contract.artifact_set`, including exact evidence IDs, versions,
URIs, digests, recording principals, and a set SHA-256.

The service records `trust: reported_content_hash` and
`content_verified_by_store: false`. It does not download or execute arbitrary
artifacts. Add ordinary `evidence` records of kind `test` or `review` for the
actual checks, then submit the work with `review`.

## Review the brief and artifact set

Each named reviewer authenticates with their own identity:

```json
{
  "expected_revision": 7,
  "command": {
    "type": "review_artifacts", "brief_sha256": "CURRENT_BRIEF_HASH",
    "artifact_set_sha256": "CURRENT_ARTIFACT_SET_HASH", "outcome": "pass",
    "criterion_indices": [0, 1], "evidence_ids": ["EXISTING_TEST_OR_REVIEW_ID"],
    "note": "Checked these exact bytes against both acceptance criteria."
  }
}
```

Each review covers every zero-based acceptance criterion exactly once. Required
reviewers must be distinct from every principal recording the current artifacts.
All named reviewers must pass; the latest report from each wins for the current
round. Failed, missing, or stale reviews prevent closure.

Changes to progress, state, scope, evidence, artifacts, or execution reports
invalidate the review round. Previous reviews remain visible. Both hashes must
match, so a changed target causes a conflict. Ordinary `verify`/`complete` cannot
bypass this policy. Reviews retain `independently_verified: false`: the store
authenticates the report, but does not equate it with a test it ran itself.
Experiment Lab's actual evaluator/program hashes can be supplied as evidence;
their test scope still limits what they establish.

## Close with requester acceptance

```json
{
  "expected_revision": 9,
  "command": {
    "type": "close", "brief_sha256": "CURRENT_BRIEF_HASH",
    "artifact_set_sha256": "CURRENT_ARTIFACT_SET_HASH",
    "note": "I accept this result for the agreed scope."
  }
}
```

Only the current requester can close after all required reviews pass. Closure
records actor, hashes, round, note and time, then marks the responsibility
completed. Reopening clears current closure and requires fresh review. Nothing
automatically publishes a completion message to Slack or another experience.

After credential rotation or an explicit reassignment, an administrator can use
`transfer_requester` with `requester_actor` and `note`. The new principal must be
the configured OS owner or an unexpired token with `work`/`admin` scope. The
transfer preserves frozen history and invalidates current review/closure. The
new requester can freeze a replacement with new reviewer identities. Normal
work tokens cannot transfer this authority.

Conversation MCP tools can record artifacts and submit reviews as their actual
agent principal. They cannot freeze policy, transfer the requester, or close as
the human requester. Authorized automation can use the same management API
with the appropriate identity.

## Verified and remaining boundaries

`tests/work.rs` covers real file digests, two reviewer tokens, missing/failed/
stale review, artifact replacement, requester mismatch, immutable briefs,
restart persistence, management-only closure, credential recovery, and legacy
compatibility. Scheduler tests continue to establish that a completed ACP turn
produces a report for review, not completion.

`tests/completion_live.py` also starts the real 0.11.0 service in a disposable
home and uses trusted HTTPS with separate requester, builder and two reviewer
credentials. It retrieves a real fixture source, records two artifact versions,
runs separate file-check processes, rejects missing/failed/stale review and
wrong-requester closure, then confirms the completed record after restart.
Its retained subprocess receipts are separate from review reports. This is an
API integration fixture, not a model collaboration or human-acceptance study.

This policy does not run external checks, authenticate a peer from its chat
name, establish human acceptance from simulated agreement, or transport
delegation/closure messages. Connectors must map authenticated participants
before recording actions. Dedicated dashboard controls and experiment scenarios
with actual participating agents remain follow-up work.
