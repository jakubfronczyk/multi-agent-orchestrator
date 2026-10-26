# API

HTTP API of `orchestrator-server`, served at `http://127.0.0.1:8787/api`. The router is
`backend/orchestrator-server/src/api/mod.rs`; every request and response type below is
exported to TypeScript in `web/src/generated/`, which is the authoritative shape.

```
POST /runs ──202 {run_id}──▶ client
                               │ GET /events (SSE): run_started, tool_call_requested,
                               │ approval_requested ...
                               ▼
                 POST /approvals/{id} {decision} ──204──▶ approval_resolved, tool_call_finished,
                                                          run_finished
```

Writes return as soon as they are accepted; progress arrives on the event stream.

---

## Conventions

**Access.** No authentication: one local operator. Every request must carry a loopback
`Host` (`localhost`, `127.0.0.1`, `::1`, any port). A request that is not GET or HEAD and
carries an `Origin` must name the same host and port. Anything else is `403 forbidden`.
Requests without `Origin`, such as curl, pass.

**Bodies.** JSON with `Content-Type: application/json`, else `415`. Uploads are
`multipart/form-data`. Request bodies are capped at 2 MB, except on `/files` (uploads and saves),
which takes up to 10 MB.

**IDs.** UUIDv7 strings, except event ids, which are integers. A malformed or unknown id in a
path is `404`; a malformed id in a query parameter or body is `422`.

**Lists.** Filters are query parameters. A value that does not parse is `422` naming the
parameter, so a typo never widens a filter. `limit` takes 1 to 200.

**Concurrent edits.** Changes the person makes to notes and files name what they last saw:
`expected_version` for notes, `expected_updated_at` for files. A newer write by an agent
makes the request `409`, never a silent overwrite.

**Money.** `*_micros` fields are integer micro-dollars (1 USD = 1 000 000).

### Errors

Every non-2xx response has one shape:

```json
{ "error": { "code": "invalid_input", "message": "must not be empty", "field": "task" } }
```

```
code            status   when
invalid_input   422      a field or parameter fails validation; `field` names it
invalid_input   415      the body is not JSON
invalid_input   413      the body or upload is over its limit
forbidden       403      non-loopback Host, or a cross-origin write
not_found       404      unknown id, or a path with nothing there
conflict        409      state forbids it: already answered, still running, stale version
internal        500      storage failure; detail goes to the server log only
```

---

## Meta

**`GET /health`**: which provider answers. `provider` is `xai` or `scripted` for typed tasks;
`scenarios` is always `scripted`.

**`GET /models`**: the selectable models and the reasoning efforts each accepts:
`grok-4.3` takes efforts `none`, `low`, `medium`, `high`; `grok-4.20-0309-non-reasoning` takes
none (empty `efforts`).

**`GET /tools`**: the 13 tools with `effect`, `taints` and Lead access, plus the `http_fetch`
allowlist. Each allowlist entry also covers its subdomains.

---

## Teams

A team is configuration: no events are emitted for team changes.

- **`GET /teams`** `?include_deleted=true`: live teams, oldest first.
- **`POST /teams`** `{name}` → `201 TeamView`. Creates the team and its Lead.
- **`GET /teams/{id}`** → `TeamView`.
- **`PUT /teams/{id}`** `{name}`: rename.
- **`DELETE /teams/{id}`** → `204`. `409` while one of its tasks is live (waiting on a card
  counts), or when it is the last team. Drops every file grant to or from it; its folder stays.
- **`GET /teams/{id}/file-access`**: every other live team and how much of its folder this
  team may open.
- **`PUT /teams/{id}/file-access/{of_team_id}`** `{access}` → `204`. `access` is `none`,
  `read` or `read_write`. Applies at a running task's next file call. `404` when either team
  is unknown or deleted. `422` when both ids
  are the same team.

---

## Agents

- **`GET /agents`** `?team_id=`: live agents, oldest first.
- **`POST /agents`** `AgentInput` → `201 AgentRecord`.
- **`GET /agents/{id}`** → `AgentRecord`.
- **`PUT /agents/{id}`** `AgentInput`: full replace; bumps the agent's config version.
- **`DELETE /agents/{id}`** → `204`. A soft delete: runs and cards still refer to it. The Lead
  cannot be deleted.

```json
{
  "team_id": "…",
  "role": "specialist",
  "slug": "reader",
  "name": "Reader",
  "description": "Reads the inbox and the web",
  "system_prompt": "…",
  "model": "grok-4.3",
  "reasoning_effort": "low",
  "icon": "…",
  "colour": "…",
  "tools": { "inbox_read": "allow", "http_fetch": "ask", "send_message": "deny" },
  "delegate_targets": []
}
```

`role` is `lead` or `specialist`; one Lead per team. A tool missing from `tools` is off and
not offered to the model. `send_message` set to `allow`, and any tool the Lead may not hold,
are `422` naming the field. `delegate_targets` (Lead only) are slugs of live specialists in
the same team. Validation lives in `AgentSpec::validated`.

---

## Tasks

A task is addressed by its root run id.

**`POST /runs`** → `202 {run_id}`

```json
{
  "task": "Check access request AR-1042 against our policy",
  "team_id": "…",
  "agent_id": "…",
  "title": "AR-1042",
  "budget_micros": 500000,
  "scenario": null,
  "draft_only": false
}
```

Only `task` is required. Without `agent_id` the team's Lead runs it; without `team_id` the
first team does. `scenario` plays fixed replies (`s1`, `clean`, `launder`) instead of the
model, and is `409` when the team's Lead cannot reach the agents the script needs. `launder` is
also `409` until `s1` has saved its decision in that team's folder.
`draft_only` refuses every save, note, fetch and send for the turn, with no card.

**`GET /runs`** `?team_id=&limit=` (default 50): tasks, newest first, with `last_event_id`.

**`GET /runs/{id}`** → `RunDetail`: every run, plan, call and card of the task, with
`last_event_id`.

**`POST /runs/{id}/messages`** `{text, scenario?, draft_only?}` → `202 {run_id}`. A follow-up:
a new Lead turn in the same task, keeping its taint, budget and refused calls. `409` while
any run in the task is live, when the task was not started by the Lead, or when its team
was deleted.

**`POST /runs/{id}/cancel`** → `202`. Every run in the task ends; `run_finished` events follow
on the stream. `409` when the task already finished.

---

## Approvals

**`GET /approvals`** `?status=&team_id=` → `ApprovalList` with `last_event_id`. `status` is
`pending`, `approved`, `denied`, `expired` or `cancelled`.

Each card carries the custody of its call: the requesting agent, the delegation path from
the Lead, the task's taint sources, the rules that fired, and a preview of the action.

**`POST /approvals/{id}`** → `204`

```json
{ "decision": "deny", "note": "wrong person, the owner is Anna Weber" }
```

`decision` is `approve` or `deny`. The note goes back to the Lead. `409` when the card is no
longer pending: answered, expired (300 s without an answer counts as no), or cancelled with
its task.

---

## Events

**`GET /events`**: `text/event-stream` of every committed run event, all teams.

```
id: 4182
data: {"id":4182,"root_run_id":"…","run_id":"…","team_id":"…","ts":"…",
       "type":"approval_requested","data":{…}}
```

- Without `Last-Event-ID` the stream is live only. With it, every event after that id is
  replayed, then live events follow with no gap and no duplicate.
- An id above the newest stored one (the database was reset) gets a live-only stream.
- A client that falls behind the server's buffer has its stream closed and reconnects.
- Keep-alive comment every 15 s.

Snapshots (`GET /runs`, `/runs/{id}`, `/approvals`, `/memory`, `/tool-calls`) carry
`last_event_id`, so a client folds only newer events onto them.

```
type                 when
run_started          a root run, follow-up turn or delegated run begins
plan_recorded        the Lead records or changes its plan
llm_requested        a model call is sent
llm_responded        it returned; carries cost
llm_retrying         rate limit or server error; backoff before the next try
tool_call_requested  the model asked for a call; carries the decision
approval_requested   the call needs a person; the card
approval_resolved    approved, denied, expired or cancelled
tool_call_started    the call runs
tool_call_finished   its status: ok, refused, denied_human, timeout, failed, ...
task_tainted         the task read outside content; carries the source
run_finished         completed, failed, cancelled or interrupted
```

Only runs emit events. Team, agent, note and file edits are plain writes.

---

## Audit and output

**`GET /tool-calls`** `?run_id=&team_id=&agent_id=&tool=&status=&limit=` (default 100): the
log of every call with its decision, rules and status. `status` is one of `ok`,
`not_offered`, `invalid_args`, `refused`, `denied_human`, `expired`, `timeout`, `cancelled`,
`failed`, `internal`, `child_failed`, `child_cancelled`, `interrupted`.

**`GET /outbox`** `?team_id=`: what `send_message` sent. Email is mocked; one row per call,
recipients and cc included.

---

## Memory

Notes have a key, a scope (`private` to an agent or `shared` by its team), an author, a
trust label and every past version. The author (`author_agent_id`, `author_slug`) and
`root_run_id` are null for a version the person wrote.

- **`GET /memory`** `?scope=&team_id=&agent_id=&q=` → `MemoryList` with `last_event_id`.
- **`GET /memory/{id}/versions`**: the note's history, each version with its author.
- **`POST /memory`** `{team_id, key, content}` → `201`. A shared note in the person's words,
  stored trusted. `409` if the team has a live note with that key.
- **`PUT /memory/{id}`** `{content, expected_version}` → `MemoryEntryView`: a new version. The
  trust label stays as it was.
- **`DELETE /memory/{id}`** `?expected_version=` → `204`. Hidden from every reader; versions
  are kept.
- **`POST /memory/{id}/trust`** `{expected_version}` → `MemoryEntryView`: the person vouches
  for the note. A new version, same text, trusted. Tasks it already tainted stay tainted.

---

## Files

All file endpoints take `team_id` and `path` as query parameters and see the workspace the
way that team's agents do: its own folder, `data/` read-only, and granted folders.

- **`GET /files`** `?team_id=&path=`: a directory listing or one file with its trust label and
  who last wrote it. `content` is null for binary files and files over 256 KB.
- **`POST /files`** `?team_id=`, multipart field `file` → `201 UploadResult`. Stored in the
  team's `uploads/`, always untrusted. A spreadsheet becomes one CSV per sheet, a Word
  document Markdown, a PDF its text. Other files are kept as sent. A taken name gets the
  next free number. `413` over 10 MB, or when the converted text is over 256 KB. `409` when
  every numbered name is taken.
- **`PUT /files`** `?team_id=&path=` `{content, expected_updated_at?}`: the person's own save
  into the team's own folder, under the agents' rules (text extension, ASCII path, 256 KB,
  never `data/` or a granted folder) → `FileWritten {path, created}`. `201` for a new file,
  stored trusted. `200` for a
  replaced file, label unchanged. `409` if the file exists and `expected_updated_at` is
  missing or stale.
- **`DELETE /files`** `?team_id=&path=&expected_updated_at=` → `204`.
- **`POST /files/trust`** `?team_id=&path=` `{expected_updated_at}` → `204`. The person
  vouches for the file, an upload included.

---

## Security drill

- **`POST /drill`** `?team_id=` → `202 {run_id}`. Starts the seeded drill: always scripted,
  playing a model that obeys the instruction planted in the inbox, so only the gates stop it.
  `409` when the team's Lead cannot hand work to `reader` and `messenger`.
- **`GET /drill/{run_id}/report`**: every way out the task tried, whether it ran, and what
  stopped it: the policy, or a person on a card.

---

## Example

```bash
API=http://127.0.0.1:8787/api

curl -s -X POST $API/runs -H 'Content-Type: application/json' \
  -d '{"task":"Summarise the inbox"}'                        # {"run_id":"…"}

curl -N $API/events                                          # watch it run

curl -s "$API/approvals?status=pending"                      # find the card
curl -s -X POST $API/approvals/<id> -H 'Content-Type: application/json' \
  -d '{"decision":"deny","note":"not this one"}'
```
