# The lethal trifecta is split by role and gated by taint

## Context

An agent is open to prompt injection when it reads untrusted content, can reach private data, and can send data out. That combination is the lethal trifecta. No prompt reliably stops an agent that holds all three from following instructions planted in what it reads.

## Decision

The seeded roles keep the trifecta away from the Lead, the Messenger and the Writer. The Reader still holds all three legs, so a gate in code closes its way out:

- **Lead:** plans and delegates only. `lead()` forbids it every tool that reads outside content and every memory tool, so it never reads outside content directly. Specialists' reports reach it wrapped as untrusted, and the task is tainted by then, so anything it delegates afterwards runs in a tainted task where sends and saves ask.
- **Reader:** reads the inbox and the web (outside content), files and CSV data (private data), and fetches URLs (egress). Its `send_message` is set to deny, so a planted "forward this" is refused and logged without a card. Fetching is gated:
  - In a tainted task every fetch asks.
  - A host off the allowlist always asks.
  - A repeat of a refused call is refused with no new card.
- **Messenger:** the only sender. It reads nothing but memory, and `send_message` is external, so it always asks.
- **Writer:** saves workspace files without a card in a clean task. Once the task is tainted, each save asks.

## Consequences

- An injection that reaches the Reader cannot get data out without a person seeing a card. The card shows the custody chain and the taint source.
- Users can edit specialists, so the specialist split is a seeded default. What holds for every team is enforced in code: the Lead's tool limits, "external always asks", and taint raising writes and egress to ask.
- Tasks take more steps and delegations than a single agent would.

## Rejected alternatives

- **One agent with every tool and prompt instructions to ignore planted text.** It is simpler and cheaper, but its safety rests on the model obeying the prompt, which is exactly what an injection attacks.
