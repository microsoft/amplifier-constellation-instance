# Long work and conversation maintenance

An active prompt no longer expires merely because the ACP connection has been
quiet. The configured connection idle limit applies between prompts. Backends
and task owners still enforce their own budgets; a silent backend is not proof
of progress or failure.

The native engine keeps durable history separate from its provider working
window. Its content-free report is available through
`GET /api/v1/native-observations` (observe scope) and the dashboard's **How the
model is working** card. Advanced model settings expose connection silence,
whole-turn limits, native compaction and caching. The regular configuration
API/CLI remains authoritative; UI actions and view state remain programmable.

Optional capabilities in the ACP initialize response describe source-delivery
receipts and guarded maintenance. Source receipts bind a connector's original
source revision/time to its authenticated session and stable input ID. Verified
incorporation can inform attention; it never means completion or suppresses a
new prompt by itself. Native receipts and third-party missing/unknown receipts
remain distinguishable. The journal is bounded and does not store source text.

For backends advertising `compact`, Instance forwards an exact single `/compact`
block without installation/context decoration. It requires an idle session
across connections, blocks overlapping turns and rejects client callbacks until
settlement. An operation ID is durable and may not be replayed. Completion needs
a matching successful typed compaction status and successful prompt terminal;
acceptance or a transport close is insufficient. Query the negotiated receipt
method after an ambiguous disconnect. This controls the client boundary, not
arbitrary execution inside a third-party backend.

See the [family harness contract](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/harness-lifecycle.v1-candidate.md)
for exact optional fields and the standard ACP fallback. Existing clients need
no custom methods for ordinary conversation.
