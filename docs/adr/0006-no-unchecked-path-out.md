# No agent can read outside content and send data out unchecked

## Context

An agent is open to prompt injection when it has three abilities at once: it reads outside content (mail, web pages), it reaches private data (files, notes), and it can send data out (email, web requests). This combination is known as the lethal trifecta (Simon Willison, 2025). No prompt reliably stops an agent with all three from following instructions planted in what it reads.

## Decision

The seeded roles keep the three abilities apart for the Lead and the Writer. The Reader and the Messenger can hold all three; each one's way out is gated in code:

- **Lead:** plans and delegates only. `lead()` forbids it every tool that reads outside content and every memory tool, so it never reads outside content directly. Specialists' reports reach it wrapped as untrusted, and the task is tainted by then, so anything it delegates afterwards runs in a tainted task where sends and saves ask.
- **Reader:** reads the inbox and the web (outside content), files and CSV data (private data), and fetches URLs (egress). Its `send_message` is set to deny, so a planted "forward this" is refused and logged without a card. Fetching is gated:
  - In a tainted task every fetch asks.
  - A host off the allowlist always asks.
  - A repeat of a refused call is refused with no new card.
- **Messenger:** the only sender. It reads nothing but memory, and its way out, `send_message`, is external and always asks, so even a laundered note it reads cannot leave without a card.
- **Writer:** saves workspace files without a card in a clean task. Once the task is tainted, each save asks.

## Consequences

- An injection that reaches the Reader or the Messenger cannot get data out without a person seeing a card. The card shows the custody chain and the taint source.
- Users can edit specialists, so the specialist split is a seeded default. What holds for every team is enforced in code: the Lead's tool limits, "external always asks", and taint raising writes and egress to ask.
- Tasks take more steps and delegations than a single agent would.

## Rejected alternatives

- **One agent with every tool and prompt instructions to ignore planted text.** It is simpler and cheaper, but its safety rests on the model obeying the prompt, which is exactly what an injection attacks.
