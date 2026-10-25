# No LLM SDK or agent framework

The xAI client is plain reqwest against the Responses API, behind one `LlmProvider` trait, and orchestration, approvals and memory are our own code. An SDK or agent framework would own the loop where the approval gate has to sit, and would hide the request shape we need to echo provider items verbatim. The trait's main job is a scripted provider: tests, the drill and the no-key demo run on fixed replies without a fake HTTP server.

## Consequences

- Retries, cost accounting and error mapping are ours to write and test.
- Another vendor or an on-prem model means one new adapter, not a new loop.
- Frameworks own the agent loop and tool dispatch; the gateway must be the single choke point every call passes, so the loop stays ours.
