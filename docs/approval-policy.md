# Approvals across an agent’s chats

An installed agent can ask before actions, remember a matching approval across
its chats, or automatically approve permitted actions. The installation owner
chooses the policy in **Access & credentials → Approvals**. Upgrading does not enable automatic
approval or delegate permission to remember: the default is **Ask**, with no
delegated clients.

This policy covers chats connected through the installed agent's ACP endpoint.
An engine console that bypasses Instance retains its own approval controls;
connecting to the installed agent uses the shared policy.

The policy setting does not bypass an engine’s hard resource restrictions,
budgets, operating-system privileges, or background-work authority. Automatic
mode can approve an offered resource or conversation grant within those limits. A question or sign-in
flow is not an action approval. The existing scheduler still refuses interactive
permission requests. Each installation owns its policy in its private SQLite
store; there is no required central approval service.

## Controls

The administrator-only API advertises `agent-instance.approval-policy/1`:

- `GET /api/v1/approval-policy` returns the current revision, `mode`, selected
  `delegated_clients`, current eligible ACP client IDs, and bounded rule summaries.
- `PUT /api/v1/approval-policy` accepts exactly `expected_revision`, `mode`
  (`ask` or `automatic`), and `delegated_clients` (existing ACP/admin token IDs).
- `POST /api/v1/approval-policy/rules/{id}/revoke` accepts `expected_revision` of
  the whole policy, not the rule revision.

Every policy change, new remembered rule and revocation advances the revision.
A stale mutation returns HTTP 409; read and review before submitting a new change.
An interrupted mutation is unknown, not a reason to replay it automatically.
Browser management uses the existing same-origin/CSRF protection. Foreground ACP
uses its existing Bearer authentication; the caller’s presentation name or source
metadata cannot confer policy authority.

The owner explicitly chooses which connected clients may create remembered
rules. Ordinary approval for the current request needs no such delegation. A
revoked or expired issuing token invalidates its future matches. Removing its
delegation also revokes its existing rules; selecting it again does not restore
them. Selecting a different backend/configuration or rotating a referenced secret
invalidates the old binding. There are at most 128 delegated clients and 256
stored rules. Adding a rule may discard the oldest revoked history to free space;
active rules are never silently evicted. The read API returns no credential
values, raw tool arguments, or reusable semantic digests.

**Future matching requests will ask again; already approved work or conversation
grants can remain valid until completed/stopped/revoked in engine.** This is the
revocation behavior in Ask mode. While Automatic remains selected, it continues
to approve valid current requests independently of remembered rules. To stop
that, explicitly select Ask; to stop already approved work, use the engine’s
Stop/revoke controls.

## One policy owner and trusted semantic matching

Remembering is optional ACP metadata under
`com.microsoft.amplifier.approval-policy`, version 1. Instance sends this in the
initialize request `_meta`:

```json
{"version":1,"owner":"instance","instanceId":"installed UUID","automaticChoice":true}
```

A supporting engine responds in initialize result `_meta`:

```json
{"version":1,"owner":"instance","producer":"stable engine contract identity","reusableRequests":true}
```

For a reusable `session/request_permission`, the engine supplies this namespace
in `params.toolCall._meta`:

```json
{
  "version":1,
  "producer":"same initialized identity",
  "key":"64 lowercase hexadecimal SHA256 characters",
  "description":"bounded plain-language description without secrets",
  "allowOptionId":"exact current allow_once option ID",
  "authorityLifetime":"request"
}
```

`authorityLifetime` is `request` or `conversation`; the latter truthfully states
that this single decision creates an engine conversation grant. Instance does
not reinterpret its scope or promise immediate revocation of that engine grant.

The engine, not a model or connector, constructs the key from resolved operation,
arguments, resources, workspace/grants, permission mode, tool registry identity,
and any relevant MCP endpoint/transport/configuration. Human wording and request
IDs are not authority. Changed effects, endpoint, descriptor trust or resource
scope must change the key. The description is display text, never a matching
criterion. The engine must keep secrets out of it. Instance additionally binds
the rule to its selected backend configuration fingerprint (including the exact
secret values supplied at launch), producer and metadata version. The rule is
not keyed by chat, session, caller request ID or tool-call ID.

For a valid negotiated descriptor and an explicitly delegated ACP client,
Instance advertises one host-owned `allow_always` choice: **Always allow matching
requests across this agent’s chats**. It records that choice durably and maps the
current response to the engine’s advertised `allow_once`; it never asks the engine
to create a second persistent cache. When `reusableRequests` is negotiated,
engine-owned persistent choices are removed. Unsupported descriptors cannot create remembered rules; normal
current-request choices remain available.

## Optional default choice for Automatic

Automatic mode is explicitly selected by the owner. Without per-request default
metadata, it may answer a valid current ACP action permission with exactly one
unique `allow_once`, including engines without either extension. It never guesses
from an option's wording or position. Other methods, such as questions or login,
retain their original behavior.

Some engines offer several materially different `allow_once` decisions for the
same request. To designate the engine's ordinary Automatic decision, a producer
may negotiate `automaticChoice: true` in the version 1 initialize profile. The
Instance initialize request advertises support as shown above; a supporting
engine responds under the same result `_meta` namespace:

```json
{"version":1,"owner":"instance","producer":"stable engine contract identity","automaticChoice":true}
```

This capability is independent of `reusableRequests`. An engine may negotiate
either or both. Negotiating only `automaticChoice` does not hide engine-owned
persistent options, create an Instance Always choice, or transfer rule ownership.
The producer identity is a nonempty, control-free string of at most 160 UTF-8
bytes, bound to this initialized backend connection and selected configuration.

For the current `session/request_permission`, the engine may include this
independent object in `params.toolCall._meta["com.microsoft.amplifier.approval-policy"]`:

```json
{
  "automaticChoice": {
    "version": 1,
    "producer": "same initialized identity",
    "allowOptionId": "exact current allow_once option ID"
  }
}
```

The nested object has exactly these three fields. Its version must be 1, its
producer must match the negotiated identity, and its single `allowOptionId` must
name exactly one current offered option of kind `allow_once`. The normal option
validation still rejects duplicate option IDs and malformed requests. Arrays,
unknown fields or versions, empty/unknown IDs, producer drift, missing negotiation
and references to persistent or rejection choices cannot authorize Automatic.
When the object is present but invalid, Automatic leaves the request for normal
manual handling, even if the request also has a single ordinary choice or a
matching remembered rule. When the object is absent, the existing unique-choice
fallback remains. Ask mode retains its existing manual and remembered-rule
behavior.

The engine constructs this metadata from its actual approval decision contract;
model text, option labels, connector hints and raw arguments are not authoritative.
It must define the selected option's semantics explicitly, including any turn or
conversation grant. This field selects one current decision; it does not assert
a reusable semantic scope or change the lifetime of the engine's grant. A
reusable descriptor, if also supplied, remains a separate sibling in the same
namespace and still requires exactly one unique `allow_once`. This extension
does not make ambiguous requests rememberable.

The existing active-prompt, source, authentication, cancellation, backend and
policy-revision checks apply at the same durable authorization boundary. No new
policy store, setting, resource permission, budget, retry or delegation is added.
Instance support alone does not enable this behavior in an existing adapter:
that producer must implement the negotiation and per-request metadata too.

## Lifetime and uncertainty

Only the exact loaded, active, uncancelled prompt/source may receive host policy
authorization. Permission callbacks get unique frontend IDs; stale, duplicate,
forged original-backend and unmapped answers cannot approve a later request.
Cancellation, prompt completion, teardown, authentication loss or changed backend
binding prevents reuse. Outstanding prompts are not restored or replayed after
restart; durable rules are.

Policy/rule revision and issuer validity are checked in the durable decision
transaction. Managed configuration/secret changes serialize with that decision.
Source authorization is revalidated using the existing provider boundary; this
is not a lease against arbitrary owner filesystem writes or external changes.
The commit is authorization, not proof of transport delivery or effects. The
bounded local decision receipts retain `delivery: unknown` and `effects: unknown`;
a failed send never causes an automatic retry. Revocation affects decisions not
yet authorized, rather than retroactively undoing a committed action.
