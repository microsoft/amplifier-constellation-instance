# The supplied agent host

For engines, connectors, experiences and management clients using Instance.
This is the supplied implementation of the [Agent Instance boundary](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/agent-instance.md):
one installation exposes its agent and supported controls without requiring
consumers to understand private host state. The selected-engine layout is local
to this implementation, not part of the universal agent definition.

**Example:** An experience attaches to an existing individual, obtains the
backend's capabilities and uses a granted conversation source. Another management
client can inspect the installation without taking over the engine session.

## Local commitments

1. **Separate the agent and management faces.** The ACP attach path reaches the
   configured engine. Instance relays its advertised backend capabilities and
   owns only its declared additions, context preparation and routing. The
   management API controls the installation independently of ACP sessions.
   The [ACP boundary](../docs/acp-boundary.md) defines ordinary stdio and the
   explicitly named authenticated remote hop.
2. **Bind context to its source.** Instance validates configured provider and
   conversation bindings before supplying authorized context or source tools.
   It does not copy a source platform's transcript into a universal host history.
   [Conversation continuity](../docs/conversation-continuity.md) and
   [tool connections](../docs/tool-connections.md) define binding, grant and trust
   operations; the source still owns membership and domain reads.
3. **Expose records and execution distinctly.** Responsibility and memory records
   have their own scoped operations. Editing a responsibility is not scheduling
   it. Optional background execution has separate authorization and a host-owned
   lifecycle; backend foreground work remains governed by the ACP connection.
   Clients discover enabled surfaces through the management descriptor.
4. **Make changes and observations inspectable.** The [management API](../docs/api.md)
   defines authentication, revisions, saved versus effective configuration and
   observations. The reference client's view state is a separate resource.
   Clients use these interfaces instead of opening the installation database.

Shared behavior is authoritative in the umbrella's
[contract map](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/README.md).
Its [ACP hosting](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/profiles/acp-hosting.md),
[managed installation](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/installations.md)
and [engine compatibility](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/engine-compatibility.md)
agreements apply to this implementation. This contract owns the host-specific
responsibilities above; exact operations stay in their linked specifications.

**Check:** Attach an ordinary ACP client and an optional management client to the
same installation. Compare the backend and host capabilities, exercise a granted
and revoked source, and distinguish a work-record change from execution. Verify
configuration revision conflicts and saved/effective state through the actual API.
Use the [interface map](README.md#technical-interfaces) for exact operations and
[validation](../docs/validation.md) for executable checks.

**Left open:** Engine family, product UX, hosting technology and which optional
services an installation enables. Instance does not guarantee a capability its
selected backend lacks.
