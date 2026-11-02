# Attributed memory

`agent-instance.memory/1` stores lasting knowledge inside one independent instance. Sessions remain separate working contexts. The memory store has no peer database, external fetcher, sharing transport, or authority to execute work.

Each topic has an immutable audience, normalized topic key, UUID, revision, candidate statements, attributed sources and revision history. A private owner session may inspect this individual's records. A group session may inspect only records with its verified provider/conversation binding. A similarly named topic in another audience is a separate record. Threads use their parent conversation's audience. Work/personal agents remain independent installations; a relationship does not grant memory access.

`POST /api/v1/memory` accepts:

```json
{
  "key": "release date",
  "statement": "The agreed release date is Tuesday.",
  "source_note": "Alex corrected the date during the planning conversation.",
  "source": {"provider_id":"slack","conversation_id":"planning","message_id":"123.456","url":"https://example.invalid/source"},
  "audience": {"kind":"conversation","provider_id":"slack","conversation_id":"planning"}
}
```

Omitting audience uses private owner context. Source is optional for an operator's own observation; a nonempty source note is always required. A conversation source must match the audience. References are attributed claims; the store does not fetch or independently verify them.

Repeated text under the same topic/audience accumulates sources. Different text adds an alternative and changes the topic to `conflicted`, clearing its selected answer. Older statements and sources remain intact. A fresh corroboration of a previously rejected alternative reopens the conflict. This uses exact normalized topic keys and statement equality; it is not semantic contradiction detection or automatic summarization.

`GET /api/v1/memory` lists at most 1,000 topics. `GET /api/v1/memory/{id}` includes alternatives and provenance. Reads require `memory:read` or `memory`; writes require `memory`. `observe`, work permissions, and UI control confer no memory authority. Administrative credentials include it.

`POST /api/v1/memory/{id}/commands` uses optimistic revisions:

```json
{"expected_revision":2,"command":{"type":"resolve","candidate_id":"CANDIDATE_UUID","reason":"Alex confirmed this supersedes the earlier date."}}
```

```json
{"expected_revision":3,"command":{"type":"archive","reason":"This project's decision is no longer current."}}
```

A resolution records the selected account and the caller's reason, not independent proof. Archiving removes a topic from prompt orientation but retains its audit history. Archived topics cannot be silently revived. Limits: 1,000 topics, 16 candidates per topic, 32 sources per candidate, 500 history entries per topic, 8 KiB per statement.

## Agent tools and UI parity

HTTP-MCP-capable ACP backends receive `memory_list`, `memory_get`, `memory_remember`, and `memory_command`. Existing context grants are rechecked on every call. Tool creation derives its audience and source conversation from the authenticated session; a model cannot supply an alternate audience. `memory_remember` accepts key, statement, source_note and optional message_id/url. Model-supplied references remain attributed reports.

Prompt orientation includes at most 12 recent non-archived topics. Conflicts include no selected statement; the agent must inspect alternatives before relying on them. Stored memories are source material, never permissions or higher-priority instructions.

The bundled Memory screen adds, inspects, resolves, and archives the same records. `window.agentInstanceControl.perform` exposes `memory.new`, `memory.select`, `memory.action`, `memory.save`, and `memory.archived`. The selected topic, archive filter, modal and `memory.*` drafts are ordinary revisioned view state. Generic CLI/API requests offer the identical operations:

```sh
agent-instance --home ./my-agent api GET memory
agent-instance --home ./my-agent api POST memory --file memory.json
agent-instance --home ./my-agent api POST memory/MEMORY_UUID/commands --file resolution.json
```

Memory change events contain IDs/revisions/state, never statements or sources. This is a bounded explicit consolidation mechanism, not an embedding index or automatic inference of a person's beliefs.
