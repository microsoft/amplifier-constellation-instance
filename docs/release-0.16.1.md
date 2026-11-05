# Agent Instance 0.16.1

Peer conversation evidence can update an agent already working without starting
another task. The optional awareness capability binds each update to the original
ACP prompt request and loaded session. Finished prompts, different sessions and
new attention offers cannot use this path. Current authorization and approvals
are preserved. Connectors must negotiate the capability and must not turn an
awareness rejection into a new prompt.

General collaboration guidance now asks agents to agree who reports a group
outcome, record actual acceptance, and deliver a self-contained result. This is
behavioral guidance, not a guarantee of group consensus or a shared coordinator.

The default native runtime remains Constellation 0.10.0. No model change or
installation-state migration is required. Third-party engines remain supported
through their advertised capabilities.

Three new real-process boundary tests failed against the previous implementation
and pass with this change. An additional test covers malformed input and the
cancellation window with string-valued prompt IDs. Existing relay, authorization and context tests remain
part of the release checks. Browser and Slack adoption, model-quality evaluation,
publication and installation rollout are separately evidenced.
