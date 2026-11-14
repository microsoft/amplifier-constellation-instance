# Responsibilities and explicit coordination

For work that needs a frozen acceptance brief, exact artifact versions, named
reviewers, and explicit requester acceptance, opt into the
[completion contract](completion-contract.md). Existing work retains the simpler
flow below. Neither flow is an ACP protocol feature; see the
[optional experience contracts](optional-experience-contracts.md).

These optional management features live inside one independent Agent Instance. They do not change ACP, run a fleet scheduler, send messages to a peer, or share a database. The same API is used by the bundled UI, the CLI's `api` command, other management interfaces, and the agent's scoped MCP tools.

## Individual, discipline, assignment

`GET /api/v1/organization` returns `agent-instance.organization/1`, a `revision`, immutable `individual` identity, and an `organization` containing three arrays:

- `templates`: `{id,name,version,instructions}`. These are local reusable discipline snapshots, not identities or credentials. An operator can distribute the same template independently to other instances.
- `assignments`: `{id,name,domain,purpose,template_id,audience,active}`. A role belongs to this individual and has an explicit audience. A work item captures the assignment and template at creation, so subsequent edits do not rewrite existing obligations.
- `relationships`: `{id,label,peer_agent_id,assignment_ids,send,receive,allow_details,active}`. Each relationship names one peer and explicit local assignments. No wildcard, hierarchy-based inheritance, or transitive permission exists.

Local IDs use 1–80 letters, numbers, hyphens or underscores. There are at most 100 records of each type. An audience is either `{"kind":"owner"}` or `{"kind":"conversation","provider_id":"...","conversation_id":"..."}`. These are actual connector identifiers. Threads inherit the parent conversation's audience. Used assignments may be archived, but cannot be removed or change audience.

`PUT /api/v1/organization` accepts `{expected_revision,organization}` and requires `admin`. Stale edits return 409. Reading requires `work:read` or `work`; ordinary `observe` access does not expose private roles, peer policies or task descriptions.

For a personal/work setup, create independent work, personal, and coordinating instances. Each work/personal instance explicitly names its coordinator in its own relationships. The coordinator configures its reciprocal inbound relationships. Neither subordinate gains access to the other's records. The coordinator's ability to receive one person's summary does not authorize relaying it to someone else.

## Durable work

`GET /api/v1/work` lists at most 1,000 recently updated records and accessible assignments. `GET /api/v1/work/{id}` reads a specific record. Reading requires `work:read` or `work`; mutation requires `work`. This is intentionally separate from runtime execution and installation administration.

`POST /api/v1/work` creates a **proposed** responsibility:

```json
{
  "title": "Verify the shipped artifact",
  "goal": "Check the delivered version against the agreed acceptance cases.",
  "assignment_id": "release",
  "audience": {"kind": "owner"},
  "source": null,
  "due_at": null
}
```

An assignment is optional. If present, it must be active and the work must retain its audience. A source, when supplied, is `{provider_id,conversation_id,message_id?,url?}`. It records provenance; it does not authenticate a person or grant access. `due_at` is a millisecond timestamp for observation, not a scheduled trigger.

Creation accepts an optional `request_id` (1–512 bytes) for one operation. Repeating
the same authenticated creation with the same content returns the existing work
in its current state; different content under the same key returns 409. The
original fingerprint stays on the work record even after amendment or Stop.
Different creation keys can legitimately produce several responsibilities from
one input. This does not make a human message globally unique or authorize execution.
Unlinked keys are scoped to the current caller/context. Source-linked keys use
the immutable input origin and agent caller, while rechecking current access;
an authorized participant change in the same room does not duplicate that creation.

The record has a stable UUID, monotonic revision, immutable `owner_agent_id`, captured role instructions, source/audience, progress, blocker, evidence, recorded verification, delegation references, timestamps, and change history. SQLite writes and optimistic revision checks make concurrent changes fail visibly. Records survive service restart and are independent of ACP working sessions.

`POST /api/v1/work/{id}/commands` accepts `{expected_revision,command}`:

| Command | Required values | Result |
| --- | --- | --- |
| `accept` | — | Proposed → accepted |
| `start` | — | Accepted/review → in progress; clears previous verification |
| `block` | `reason` | Accepted/in progress/review → blocked |
| `resume` | — | Blocked → in progress |
| `progress` | `summary` | Records progress and invalidates a prior check |
| `amend` | `title,goal` | Changes scope and invalidates a prior check; review returns to in progress |
| `evidence` | `label,uri,kind` | Adds a recorded artifact/test/review/source reference, invalidating a prior check |
| `review` | — | Accepted/in progress → ready for review |
| `verify` | `outcome,evidence_ids,note` | Records pass/fail against existing evidence while in review |
| `complete` | — | Review → completed only with a passing recorded check |
| `cancel` | `reason` | An open responsibility → cancelled |
| `reopen` | `reason` | Completed/cancelled → accepted, without carrying forward old verification |

Evidence URIs accept HTTP(S), file and URN references without embedded credentials. The service does not fetch or execute them. Verification records the authenticated caller, references and assertion; `independently_verified:false` remains explicit. The UI says **Reported complete**. A saved reference, another agent's statement, and a model's turn ending are not independent proof of success. There is no automatic transition from an ACP response or a peer's report to completed work.

Each item supports 100 evidence references and 2,000 changes; create a follow-up for longer work. Observation events contain IDs, actor and revision, never goals, summaries, profile bodies or evidence contents.

Example CLI use:

```sh
agent-instance api GET organization --home /path/to/instance
agent-instance api POST work --file new-work.json --home /path/to/instance
agent-instance api POST work/WORK_ID/commands --file command.json --home /path/to/instance
```

## Delegation without hidden coupling

`POST /api/v1/work/{id}/export` accepts `{expected_revision,relationship_id,disclosure,summary}`. The assignment must appear in an active relationship permitting send to the named peer. Disclosure is:

- `summary`: only the explicitly written summary, source/audience and correlation metadata. No automatic title, goal or evidence copy.
- `details`: additionally title, goal and evidence, allowed only when the relationship explicitly permits details.

The `agent-instance.delegation/1` envelope includes its UUID, source agent/work/revision, target agent, source audience and conversation provenance, disclosure choice, creation actor and time. `sharing` says `named_recipient_only` and `forwarding:false`. The sender's responsibility remains locally owned, and its delegation has `delivery:"not_confirmed"`. Exporting a packet neither sends a message nor transfers responsibility. A user or authorized adapter transports it using an experience/connector they already control.

The recipient's `POST /api/v1/work/import` accepts `{relationship_id,assignment_id,packet}`. Its independently configured active inbound relationship must name the claimed sender and local assignment and permit that disclosure. Received material enters an **owner-only** assignment as proposed work. The envelope is retained under `upstream` with `trust:"unverified_remote_report"`, `source_identity:"claimed_by_envelope"` and `authenticated_transport:false`. This API records a local operator's authorized import; it does not authenticate a remote person or app based on an ID in JSON. A connector must establish that mapping independently before invoking it.

Retries with the same source agent and envelope ID return the same local work; changed contents under that key are rejected. The local import, dedupe receipt and outbound preparation each commit atomically. Imported work cannot be re-exported, preventing an overarching coordinator from automatically becoming an onward-sharing path. More granular onward consent and automated authenticated delivery are deliberately not part of this contract.

## Agent access through ordinary MCP

Backends supporting HTTP MCP receive `work_list`, `work_get`, `work_create`, and `work_command` alongside existing continuity tools. Every call rechecks the active ACP grant and conversation authorization. Private owner context can access this individual's work; a group context can only access work bound to its verified provider and conversation. Tool-created work derives its audience from the binding, not model-supplied metadata. The agent cannot edit sharing policy or export/import packets through these tools.

`work_create` also accepts `source_input_id` together with `request_id`. The context
lease supplies the authenticated principal/session; the host resolves the input
from its [source receipt](harness.md#source-delivery-facts) or retained work origin.
The caller cannot supply an audience or forge a `source_input` object. The work
retains immutable original references, revisions and attribution in `source_input`,
separate from the editable brief. A sole source ID also fills `source.message_id`;
multiple source references remain in `source_input.sources`.

Use the advertised source receipt query to recover linked work after a lost reply.
Creation still yields `proposed`; `work_command accept` is a separate transition.
After a lost command reply, inspect the current work/revision instead of assuming
acceptance or replaying an effect. Cancellation remains the existing work command,
distinct from cancelling one ACP turn. No automatic source publication is added.

Prompt orientation provides at most 12 open, scoped responsibility summaries. Use `work_get` for the captured role instructions and evidence. Runtime profile context, prompts and MCP results remain ordinary model context, not an authority channel. Non-HTTP-MCP backends remain ACP compatible but cannot use these optional tools.

## Profile delivery observations

`GET /api/v1/profile-use` requires `observe`. Optional `session_id` chooses a particular working context; otherwise the server selects the **latest started** observed turn across this instance. A delayed completion or a session reload cannot replace that selection. `max_age_seconds` defaults to 86,400 and can range from 60 seconds to 30 days.

The result includes the installed version and content digest, relevant turn and delivery time, content/version comparisons, and one of:

- `not_configured`: no non-whitespace communication, working-style, or identity guidance is installed. This takes precedence even if an empty hash matches a recorded turn.
- `unknown`: guidance is configured, but no recorded turn exists for the selection.
- `partial`: the selected turn lacks content-delivery evidence.
- `stale`: its start is older than the selected freshness window.
- `supplied_current`: the actual delivered profile text matches current guidance.
- `supplied_previous`: the actual delivered text differs from current guidance.

`installed.configured` reports whether effective guidance exists. Package identity alone does not establish this: legacy local files and standalone identity guidance can be configured without an imported package, while a saved empty version remains unconfigured. Whitespace-only files are omitted from the delivered profile context. For `not_configured`, content/version comparisons are null; any earlier `relevant_turn` and its age remain available, including after guidance is cleared.

Equal text with a different package revision is reported as matching content and nonmatching version, not a behavioral failure. Delivery is recorded only after the relay writes the prompt to backend input. It proves neither model consumption nor compliance. `behavior_quality:"not_evaluated"` and `source_draft_status:"not_observed"` are explicit: this service knows its installed copy, not an unsaved or saved Persona Workshop draft. Pre-upgrade sessions have unknown turn evidence until their next prompt.

## Explicit background execution

Accepting or starting a ledger responsibility does not authorize autonomous execution. The optional [background-work capability](background-work.md) adds a durable instance-owned ACP client with separate `work:execute` authority, bounded runs, cancellation and explicit recovery. Returned reports remain unverified and enter the existing review/evidence flow. The scoped `work_execution` tool observes runs without granting execution authority.

## Lasting knowledge

The separate [memory capability](memory.md) keeps attributed knowledge and conflicts within this individual's audience boundaries. A remembered statement is not a responsibility, verification or execution grant.
