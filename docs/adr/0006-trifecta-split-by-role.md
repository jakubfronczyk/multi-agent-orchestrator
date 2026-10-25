# The lethal trifecta is split by role

## Context

An agent is open to prompt injection when it reads untrusted content, can reach private data, and can send data out. That combination is the lethal trifecta. No prompt reliably stops an agent that holds all three from following instructions planted in what it reads.

## Decision

No seeded agent holds all three, and the riskiest parts of the split are enforced in code rather than by prompts:

- **Lead:** plans and delegates only. `lead()` forbids it every tool that reads outside content and every memory tool, so it never sees outside text, not even laundered through a stored note.
- **Reader:** reads the inbox, files and the web. Its `send_message` is set to deny, so a planted "forward this" is refused and logged without a card.
- **Messenger:** the only sender, and it holds no reading tools. `send_message` is external, so it always asks.
- **Writer:** saves workspace files with no approval in a clean task. Once the task is tainted, each save asks.

## Consequences

- An injection that reaches the Reader can only ask the Lead to delegate. Any send or save that follows appears on a card showing custody and the taint source.
- Users can edit specialists, so the specialist split is a seeded default. The Lead's limits, "external always asks" and taint are enforced for every team.
- Tasks take more steps and delegations than a single agent would.

## Rejected alternatives

- **One agent with every tool and prompt instructions to ignore planted text.** It is simpler and cheaper, but its safety rests on the model obeying the prompt, which is exactly what an injection attacks.
