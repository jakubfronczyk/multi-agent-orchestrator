# The event log is the source of truth

Every state change in a task is an append-only event. The runs, tool calls and approvals tables are projections written in the same transaction as the event that changes them, and the live stream to the UI is the same events. We chose this over writing those tables directly, so the audit log can never disagree with the state, a reconnecting client replays from its last event id, and in-memory task state changes only after its event commits.

## Consequences

- Events are never updated or deleted; database triggers enforce it.
- Totals that only events carry, such as cost, are summed from events instead of kept in their own table.
- Writes outside a task (agent edits, notes, files, the outbox) stay plain table writes.
