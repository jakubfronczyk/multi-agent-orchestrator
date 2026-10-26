# Files per team, opened by grant

Each team has its own folder, and its agents reach only that folder, the shared reference folder `data/` (read only), and the folders of teams the user has granted to it, read or read and write. This amends ADR 0007, which kept one workspace for every team. That choice held while teams were variants of one desk. Once users build unrelated teams, one shared workspace means clutter and silent overwrites between teams. It also means a task tainted in one team can read every other team's files without a card, because reads never ask. Folder reach is a limit on what a task can reach, which ADR 0007 already allows a team to change, so `decide` still takes no team and every gate is unchanged.

## Consequences

- Agents name paths in their team's view: `data/...` maps to `shared/data/...`, `teams/<slug>/...` to that team's folder, and every other path to `teams/<own id>/...`. Script paths did not change.
- A path out of reach is refused before the decision (`no_access`), so it raises no card, starts nothing and taints nothing. `run` checks again, so a grant revoked in between fails closed.
- A saved path is ASCII, and trust rows compare paths ignoring letter case. A save whose path differs only in case from a recorded file is refused, so on any disk one file has one trust row and tainted bytes can never sit behind a trusted one.
- A save checks reach under the workspace's exclusive lock, which grant changes and team deletes also take, so a revoke never lands between the check and the bytes.
- Grants are read on every call, so a revoke applies at a running task's next file step. A no-access refusal does not count as a repeat, so a later grant lets the same call through.
- A grant never changes a label. Reading a granted team's untrusted file taints the reader, and a write into a granted folder still asks when the task is tainted.
- Uploads land in the team's own `uploads/`. The inbox and the outbox stay shared; their label already taints any reader.
- Deleting a team drops every grant to or from it. Its folder stays, and the user can still see it, but no agent can reach it.

## Rejected alternatives

- **Path rules per agent.** Globs per agent would give finer control, but they add a second policy layer to build, explain and show on cards. Grants per team cover what the user decides.
- **A snapshot of grants at task start.** It would keep a running task consistent, but a revoked task could keep reading.
