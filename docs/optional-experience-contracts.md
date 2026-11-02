# Optional experience contracts alongside ACP

The base boundary is ACP initialization, authentication, sessions, prompts,
updates, callbacks, and advertised capabilities. A client can use an agent
without knowing this product's context, participation, management, or work
conventions. Optional contracts must not be reported as standard ACP features.

ACP permits custom metadata where `_meta` is defined and separately named
custom methods/capabilities. Our conversation integration uses namespaced
metadata and standard MCP configuration; it adds no standard ACP method for
sharing transcripts, accepting work, or choosing who speaks.
[ACP extensibility](https://agentclientprotocol.com/protocol/v1/extensibility),
[initialization](https://agentclientprotocol.com/protocol/v1/initialization),
[session setup](https://agentclientprotocol.com/protocol/v1/session-setup).

## Existing versioned profiles

| Profile | Owner and boundary | Without it |
| --- | --- | --- |
| `agent-instance.wss/1` | Attach helper/service; one JSON-RPC object per WebSocket text message, a custom transport profile | Use standard stdio through an ordinary ACP client. |
| `agent-instance.management/1` | Separate authenticated descriptor/API | ACP works; installation/configuration/UI control cannot be assumed. |
| `agent-instance.context/1` | Authorized conversation-provider HTTP query | Use current session and explicitly supplied context; assume no cross-room access. |
| `slack-conversation-decision/1` | Slack adapter's opt-in prompt convention, advertised in its management descriptor | Direct final text remains useful; ambient group participation requires deliberate configuration. |
| `agent-experiment-lab.scenario/1` | Lab scenario and response convention | Ordinary ACP final text is accepted and recorded as plain text. |
| `agent-instance.work/1`, optionally containing `agent-instance.completion-contract/1` | Management/work tools | ACP work continues without assuming durable responsibility/evidence/closure state. |

These suffixes version product profiles independently of ACP's
`protocolVersion`. Unknown versions must not be treated as compatible version 1.
Current integrations select contracts through configuration and management
descriptors; there is no universal ACP negotiation for these application
features.

## Context and identity

An adapter may attach
`_meta["io.github.bkrabach.conversation"] = {provider_id, grant}` during session
setup. The instance validates that wrapper hint with its registered provider,
removes it before forwarding to an unrelated backend, binds the verified
audience, and revalidates on prompts/retrieval. Provider bearer credentials and
opaque conversation grants stay out of prompts and history.

The provider owns conversation/thread IDs, source links, edits/deletions,
membership and identity mapping. `owner:true` follows explicit verified owner
linking. Group grants stay scoped to their provider/conversation. Presence in a
different room does not grant permission to retrieve that room. Replies stay in
the experience's selected destination; reading a source never grants posting
authority there.

`authorize`, `list`, `read`, and `orientation` retain provenance and coverage.
Partial history remains partial; failed sources are not empty conversations;
revocation never downgrades to owner access. Optional context MCP tools require
backend support for their transport. Exact payloads and lifecycle rules live in
[conversation continuity](conversation-continuity.md).

## Participation and delivery

Slack distinguishes direct messages/explicit address from ambient group turns.
Its opt-in group convention requests final text like:

```json
{"decision":"speak","text":"A useful contribution.","reason":"Answers the open question"}
```

or `{"decision":"pass","reason":"Already answered"}`. Direct exchanges use
normal final text, with an explicit request for silence as the defined exception.
Invalid group decisions are observed and withheld. Intermediate reasoning and
tool output are never published as chat.

Experiment Lab's version-1 scenario uses a different convention:
`{"action":"speak","text":"..."}` or
`{"action":"pass","reason":"..."}`. It also supports ordinary final text,
while malformed attempted envelopes remain private. It records drafts,
receipts, cancellation, scenario identity, and driver versions. Its seeded
rounds do not reproduce Slack timing. These two shapes are separate application
conventions; neither is a shared ACP wire type.

The experience decides prompt timing and what context was actually visible;
the agent chooses whether it has a useful contribution under the configured
policy. Inference, parsing a speak decision, publishing a message, and confirmed
delivery are distinct observations. Pass, malformed output, timeout,
cancellation, and unknown delivery remain distinguishable. No participation
decision grants work, tool, or posting authority.

## Work and evidence

The [completion contract](completion-contract.md) freezes a source-linked brief,
records versions and reviews, and separates requester closure. Background work
still needs its own grant. Plain ACP does not authenticate peer delegation or
human acceptance automatically.

A third-party runtime can participate using plain ACP, optionally follow
experience prompt conventions, and separately support authenticated context/
management/work contracts directly or through a wrapper. None requires a fleet
service to define its identity or access to another participant's private files.
Unsupported optional features must stay visible rather than being approximated
with hidden access or invented completion evidence.
