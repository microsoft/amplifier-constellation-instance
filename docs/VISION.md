# Keep an agent available across places

Constellation Instance gives people a continuing agent they can reconnect with,
inspect and direct without setting up a new collaborator for every conversation.
Its installed identity, configuration, connections and accepted responsibilities
remain available across the places where people work.

This repository supplies one implementation of the umbrella's
[Agent Instance](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/agent-instance.md)
boundary: an independent host with a selected ACP engine. That is this service's
design, not a requirement on every agent. Other implementations can use an inner
team or a different architecture behind the same outward boundary.

## Make continuity useful

A person returns to an agent through another experience. Instance exposes the
same identity, configured connections and permitted responsibilities; its engine
continues through supported execution sessions. The experience presents that
work, while conversation sources retain their exchanges and audiences. Tools
and connections provide their own capabilities through declared interfaces.

People should be able to understand what the agent has accepted, inspect relevant
evidence and redirect supported work. Useful memory and responsibility records
support that continuity without becoming substitutes for source records or action
authority. An experience need not become an installation manager to collaborate
with the agent.

## Keep this host focused

Instance supplies hosting, routing and exposed controls; its selected engine owns
reasoning and execution. Improving one should not require rebuilding the other.
The bundled administration client uses those controls without prescribing the UX
of future experiences. A fleet service remains optional.

Judge improvement by the effort to connect, understand, operate and recover the
same agent, including administrator effort. Exercise a complete client–host–engine–
connection journey. The [hosting agreement](../contracts/hosting.md) defines this
implementation's promises; the [umbrella](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/README.md)
owns shared composition, interaction, authority and continuity.
