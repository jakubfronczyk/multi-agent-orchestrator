# A team organises work; it is not a security boundary

There is one user and no auth, so a team cannot protect anything from anyone. The one boundary the app enforces is outside content versus the user's own actions (ADR 0001, ADR 0006), and every gate on it reads the task's taint, the agent's setting, the tool's effect, the allowlist and draft-only. None of them reads a team. A team changes what a task can reach (its own agents and its own shared notes), never how a call is decided, so adding teams can only remove paths.

## Consequences

- `decide` takes no team. Its inputs are pinned by a field-exhaustive test, so a team field would fail to compile.
- Agents, delegation and shared notes are per team; slugs are unique within a team, and a task's snapshot holds one team.
- Files are per team, and another team's folder opens only by the user's grant (ADR 0008). The inbox is shared by every team. A trust label travels with the data: a file a tainted task saved is untrusted, and any team that reads it is tainted.
- Approvals, the audit log and the event stream are one global view for the one user, each record labelled with its team.
- The UI says "organised by team", never that a team is isolated.

## Rejected alternatives

- **A workspace folder per team.** Rejected at first because every script path and the demo's shared reference data would change. Adopted later in ADR 0008, with a path mapping that keeps the script paths.
- **Teams as tenants with their own access rules.** There is no second user to keep out, so the rules would protect nothing and add a second policy layer to explain.
