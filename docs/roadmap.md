# Separate products, common boundaries

Agent Instance manages one independently installed agent. The optional
[Observatory](https://github.com/microsoft/amplifier-constellation-observatory)
manages a fleet; [Persona Workshop](https://github.com/microsoft/amplifier-persona-workshop)
builds portable guidance; experiences and adapters own conversations. Each is
usable separately through documented interfaces.

The current family direction and remaining work live in the
[Constellation hub](https://github.com/microsoft/amplifier-constellation).
Detailed implemented contracts remain with their owning component.

- ACP is the work boundary. Management, provisioning and shared context are
  optional negotiated interfaces.
- Profiles supply communication and working-style guidance; they confer no
  authority or credentials. Versioned profile application is explicit and reversible.
- Work records distinguish suggestions, accepted responsibility, delivered
  artifacts, independent review and requester acceptance.
- Every management action and meaningful UI state is available programmatically.
- Validate transport correctness, authorized behavior, social usefulness and
  artifact quality separately.
