# The person marks content as trusted; saving never does

The person can create, edit and delete a team's notes and files, and mark an untrusted one as trusted. What the person creates is trusted, since it is their own words. An edit keeps the label: the editor starts from the old text, so an untrusted file's injected part may still be in it after a save. Only "Mark as trusted" promotes, as its own action, after a warning that shows the whole text. This keeps the rule of ADR 0001 that a label follows the data: the person's explicit vouching is the one way untrusted content becomes trusted, and it is never a side effect.

## Consequences

- A mark never clears a task's taint. A task that read the content before it was marked stays tainted, follow-ups included.
- A note or version the person wrote has no author agent and no run; a file they wrote is marked as theirs. The record says who wrote what.
- Deleting a note hides it from every reader and keeps its versions; an agent writing the same key later revives it. A tainted task may write over a deleted trusted note, since nothing trusted is left to replace.
- Each change names the version (notes) or `updated_at` (files) the person saw, so it never overwrites an agent's newer write unseen.
- The person writes only in a live team's own folder, with an agent's save rules; `data/` stays read only. `decide()` and the agents' tools do not change.

## Rejected alternatives

- **Saving promotes.** Simple, but a person tidying an injected note would launder it without meaning to.
- **Marking untrusted.** Deleting or editing covers a person who stops trusting something; a second label action would add wording without adding safety.
