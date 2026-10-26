# Architecture

What is built today, why, and what each choice costs. Production plan: `PRODUCTION.md`.

## 1. Constraints

- The brief requires agents that can be edited in the UI with an allow, ask or deny policy per tool, an orchestrator that delegates and is visible live, memory with private and shared scopes that records its author, tools that run in the backend rather than as xAI server-side tools, function calling against xAI Grok, and no agent frameworks.
- One developer built it against a fixed deadline.
- It serves one local user on 127.0.0.1. There are no accounts, no authentication and no tenants: whoever can reach the port is both operator and approver. A middleware refuses any request whose Host is not a loopback name and any state-changing request from another origin.
- Trusted by assumption: the task text the user types, and the seeded reference files in `data/`. Approval timeouts use one process clock.
- It runs locally with no infrastructure, and the demo scenarios replay the same way every time.
- The result is one process, one SQLite file and one local folder.

## Terms

- **Taint:** a flag on a task, set the first time any of its runs reads outside content (an email, a web page, an upload, or a note or file marked untrusted). It never clears.
- **Egress:** a call that sends data out of the system, such as `http_fetch`.
- **Projection:** a table derived from the event log, such as `runs` or `tool_calls`.
- **Card:** the approval request a human sees in the UI before a risky call runs.

---

## 2. System

```
browser ──HTTP + SSE──▶ axum /api ──▶ Engine (one tokio task per run)
                                        │
                     ┌──────────────────┼──────────────────┐
                     ▼                  ▼                  ▼
               ScenarioRouter       Gateway             Store::emit
               │          │     (policy, approval,   (event + projections,
             xAI      Scripted   taint, timeout,      one transaction)
           Responses   fixtures   audit)                   │
                                   │                  SQLite (WAL)
                                   ▼                       │
                              13 tools ──▶ workspace, memory, inbox/outbox
                                                           │
                                         broadcast ──▶ SSE (replay by Last-Event-ID)
```

A task is a tree of runs. The Lead plans and delegates; specialists do the work. Every tool call, including delegation, goes through the gateway.

---

## 3. Invariants

1. **The strictest rule wins.** The policy combines the agent's setting, the allowlist, taint, the tool's effect and draft-only mode, and takes the strictest result. No setting goes below the floor set by the tool's effect and the task's taint: a send always asks, and in a tainted task every file write, fetch and send asks. Agent settings, roles and the allowlist are configuration above that floor.
2. **Taint is per task and permanent.** Once a task is tainted, every file write, fetch and send needs approval. Memory writes still run, and are stored as untrusted.
3. **The label follows the data.** Anything a tainted task stores is marked untrusted, and reading it taints the reader's task.
4. **The tool set is closed and its risk is fixed in code.** A new tool does not compile until its effect and taint are classified.
5. **The Lead never receives raw outside content.** It reads trusted notes only, and tool results carrying outside content are dropped before they reach it. Specialists' reports, which can carry injected text, arrive wrapped as untrusted; in a tainted task a failed child's error is replaced by a fixed message. Taint, not the wrapper, is the control (4.7).
6. **The event log is the source of truth.** Projections are written in the same transaction as the event, and events are never rewritten.

---

## 4. Decisions

Each decision starts with what was built and why. **Cost** is what it gives up. **Changes when** names the event that would make it wrong, and the step in `PRODUCTION.md` that handles it.

### 4.1 Rust workspace split into core and server

The core crate holds the domain, engine and policy with no I/O, so the policy is a pure function and the engine is tested without a database. Exhaustive matching on tools means an unclassified tool fails to compile.

- **Cost:** the `Store` trait covers only the engine. The API, tools, recovery and workspace talk to SQLite directly, so switching databases is a port of 82 queries and 111 database tests, not a new trait implementation.
- **Changes when:** the move to Postgres (production step 2).

### 4.2 xAI client written by hand

The Responses API is called over raw HTTP because the design needs fields that SDKs hide: encrypted reasoning returned as-is, `store: false` and `prompt_cache_key`. Provider messages are passed back unchanged, so the core never interprets xAI formats. Cost is read from each reply rather than a price table. Rate-limited and server-error calls retry with jittered backoff (5, 10, 20, 40 s), because xAI sends no Retry-After header. The no-framework rule comes from the brief; only the client layer is a choice.

- **Cost:** retries and response types are written by hand. A reply without a cost fails the call. Backoff is paced per process, not per API key.
- **Changes when:** several processes share one key (production step 3), or a second provider is added (production step 5). In production any client works, as long as every tool call still goes through one gateway.

### 4.3 Scripted provider for scenarios

Scenario tasks replay recorded model responses; free-text tasks go to Grok when a key is set. The demo and the security drill behave the same on every run, cost nothing and work without a key.

- **Cost:** scripted runs test the permission checks, not the prompts.
- **Changes when:** a prompt change passes CI but fails against the live model. Production step 1 adds live-model evals.

### 4.4 One Lead, delegation as a tool

The Lead is the only agent that can delegate, and delegation is a tool call like any other. That gives one plan and one summary per task, and delegation is logged, budgeted and limited by the same gateway. A task allows depth 2, 12 runs, 12 steps for the Lead and 8 per specialist, and a budget. Follow-up messages start a new Lead turn in the same task.

- **Cost:** specialists cannot talk to each other directly.
- **Changes when:** specialists need to negotiate. Peer messaging would become one more tool behind the same gateway.

### 4.5 Agents and models

Deleting an agent marks it deleted rather than removing it, because runs, events and cards refer to it. Each run records the agent's config version, and the team is loaded once per turn, so the log always shows the configuration that actually ran. The seeded team uses `grok-4.3`: the Lead with medium reasoning effort because it plans, and specialists with low or none because their steps are narrow.

- **Cost:** editing an agent does not affect a turn already running; cancelling does. Model and effort choices are not benchmarked.
- **Changes when:** cost or latency per task gets a target. Production step 1 adds the metrics.

### 4.6 One gateway for every tool call

Tools run in the backend, as the brief requires. Every call goes through one path that parses arguments, applies the policy, waits for approval, enforces a per-tool timeout, isolates panics and writes the audit record. Failures are classified by who can fix them: the model (bad arguments), the request, or the server. A timeout, cancel or panic ends the call, not the run.

- **Cost:** a tool that ignores cancellation keeps running until its next await point.

### 4.7 Taint and the role split

Each role sees only part of the risk. The Reader reads outside content but cannot send. The Messenger sends, and always asks first. The Writer saves files, and asks once the task is tainted. The Lead plans and delegates. Its only tools are delegate, plan, and reading or searching trusted notes.

A specialist's report does reach the Lead, labelled untrusted, and it may carry an injected instruction. The control is taint, not the label: once outside content is read, every write, send and fetch in the task needs approval.

- **Cost:** in a tainted task, reads (including CSV queries over the team's files and granted folders), delegation and memory writes (stored as untrusted) still run without approval. The role split is seed configuration: validation keeps the Lead and specialists apart, but one specialist could be given both `inbox_read` and `send_message`; the send would still ask.

### 4.8 Sending and approvals

Sending a message can be set to ask or deny, never allow. This is stricter than the brief, because a send cannot be undone and it is the last step of any data leak. Validation rejects allow, and the policy turns allow into ask if validation were bypassed.

An approval is a database row plus a waiter in memory. The card shows which agent asked, the path of delegations, what outside content the task read, which rules fired, and a preview of the action. If the answer and the 300 s timeout arrive together, whichever claims the approval first decides it.

- **Cost:** no unattended sending. No approver identity is recorded, since there is one user. The waiter lives in the process that runs the task.
- **Changes when:** an answer reaches a different node, which returns a false 409 while the real card expires (production step 3). For bulk trusted sends, a scoped allow (fixed recipient, untainted task) would be a new rule in the policy.

### 4.9 Outside content: uploads, web fetch, draft-only

Uploaded files (csv, md, txt, json) are stored as untrusted, so reading one taints the task like an email. An upload to an existing path is refused. `http_fetch` resolves the host, blocks private address ranges, pins the resolved address, follows only redirects to the same host and scheme (each hop checked again), uses no proxy and caps responses at 1 MiB. A host outside the allowlist needs approval. Draft-only mode refuses every lasting change without asking.

- **Cost:** the allowlist is compiled in (`example.com` and its subdomains), so changing it needs a rebuild.
- **Changes when:** users need their own hosts (production step 6), or uploads from known colleagues must be trusted, which needs authentication (production step 1).

### 4.10 SQLite and the event log

SQLite needs no setup and is transactional, which is enough for one node. Writes take the lock up front, and database constraints enforce one Lead, unique agent names and one approval per call. Each event and its projections are written in one transaction. A single lock covers the write and the broadcast, so events reach subscribers in id order. A reconnecting browser sends its last event id and receives everything after it.

- **Cost:** one writer, one node, no high availability. All writes in the process are serialised. Events cannot be deleted.
- **Changes when:** high availability or a second node is needed (production steps 2 and 3), or someone asks for their data to be erased (production step 1).

### 4.11 Restart interrupts unfinished work

At startup, every unfinished run is closed as interrupted and pending approvals expire, so the log, the tables and the UI agree after a crash.

- **Cost:** work in progress is lost, and a send interrupted mid-call has to be checked in the outbox by hand. Resuming is not possible: the log does not store raw model output or full tool results, and sending, writing files, writing memory and delegating are not safe to repeat.
- **Changes when:** a second node starts, because it would close the first node's live runs (production step 3).

### 4.12 Memory

Notes have private and shared scopes, keep every version, record the authoring agent from the call context (an agent cannot claim to be another), and carry a trust label. Search is substring matching, because the corpus is small enough for agents to search with tools.

- **Changes when:** substring search stops finding what agents need (production step 7).

### 4.13 Tests, CI and UI

Integration tests run real tools against a real SQLite database with the scripted provider, including scenarios for a clean task, an injection drill and data passed between tasks. CI runs formatting, clippy with warnings denied, the tests, a check that generated TypeScript types match the Rust types, and the sqlx query check. CI also builds and lints the web app. The UI is React with TypeScript types generated from Rust, with pages for tasks, the task thread, approvals, team, notes, files, sent messages, activity and the drill.

- **Cost:** the dependency audit runs by hand.

### 4.14 Evaluation

Evaluation is deterministic and scenario-based. Five scripted scenarios (`s1`, `s1-followup`, `clean`, `launder`, `drill`) replay recorded model responses through the real engine, gateway, tools and SQLite, and integration tests assert the outcome in CI. The drill plays a model that obeys an instruction planted in an email, so it measures the permission checks, not the model. Its report lists every exit the task attempted (any call to a tool that sends data out), whether it ran, and who stopped it: the policy or a person on a card. One live test sends every tool schema to Grok, skipped unless `XAI_API_KEY` is set, so a schema xAI rejects fails there first.

A separate harness in `web/eval/` runs real tasks on live Grok (about $0.04 each) and grades what the backend recorded, not what the agents said: calls, cards, sent messages, saved files and the event stream, plus an LLM judge. Each case runs on a fresh team and answers approval cards the way that case's person would. A scripted mode runs the same pipeline for free. Its first run caught a Messenger claiming "Email sent" with no send call behind it.

- **Cost:** live evals run by hand, not in CI, because they cost money. Prompts are not versioned, so a prompt change cannot be compared with the previous one, and results live in log folders rather than a tracked history. The event log is the only trace.
- **Changes when:** prompts or models change more than occasionally, or more than one person edits them (production step 1).

### 4.15 Email is mocked

The inbox is seeded from fixture files and `send_message` writes to an outbox table; nothing leaves the machine. The brief asks for a mocked send, and the security story only needs the decision point, not delivery: the approval card, the policy and the audit record are real, and the outbox shows exactly what would have been sent. Each outbox row references the tool call that wrote it, and that reference is unique, so one call can send at most one message.

- **Cost:** no delivery, bounces or replies. Incoming mail is a fixed set of fixtures, not a live mailbox.
- **Changes when:** a real recipient is needed (production step 8).

### 4.16 Teams and file grants

A team groups agents, delegation targets and shared notes; it is not a security boundary, because there is one user and nobody to keep out (ADR 0007). The policy takes no team as input, so adding teams can only remove paths. Each team has its own folder; agents reach it, the read-only `data/` folder, and the folders of teams the user has granted (ADR 0008). A path out of reach is refused before the policy runs, and grants are read on every call, so a revoke applies at the next file step. The inbox, outbox, approvals, audit log and event stream are shared across teams, each record labelled with its team.

- **Cost:** no per-team permissions on the shared inbox or approvals.
- **Changes when:** a second user exists; teams then need owners and approvals need authorization (production step 1).

---

## 5. Known limits

**Open**
- The client can set any task budget; there is no server maximum.
- Task, follow-up, title, system prompt and description have no length limit beyond the 2 MB request cap.
- SIGTERM is not handled.
- There is no Dockerfile; run with `just dev` and `just web`.

**Left out**
- Authentication (see §1).
- Token streaming; the UI shows events per step.
- Editing the allowlist at runtime.

**Accepted at this size**
- One node only.
- Listing files queries trust once per file.
- Events cannot be erased; the fixtures contain no personal data.
- The drill and scenario selection are open to any caller.

---

## 6. How it was built

- Built solo with coding agents. Issues are local plan files kept out of git, decisions are ADRs 0001–0008, commits are small and conventional, and `just ci` runs on every push. Review meant checking agent output against the ADRs and the tests.

---

## Appendix: where to look

Paths: `core/` = `backend/orchestrator-core/src/`, `srv/` = `backend/orchestrator-server/src/`, `mig` = `backend/orchestrator-server/migrations/0001_init.sql`.

- Policy: `core/policy/decide.rs` (`decide`, `raised_by_taint`, `leaves_a_change`)
- Tool risk and parsing: `core/tools.rs` (`effect`, `taints`, `lead`, `parse_args`, `ToolError`)
- Gateway, timeouts, Lead result guard: `core/engine/gateway.rs`
- Delegation and failure text: `core/engine/native.rs` (`delegate`, `tainted_label`), `core/engine/prompts.rs` (`failure_text`)
- Step limits per role: `core/agents.rs` (`Role::max_steps`, `validated`)
- Approvals: `core/engine/approvals.rs`, `srv/api/approvals.rs`
- Retries: `core/engine/run.rs`, `srv/llm/xai/mod.rs` (`is_retryable`), `srv/config.rs` (`retry_backoff`)
- xAI client and cost: `srv/llm/xai/mod.rs`, `core/money.rs`
- Scenario routing: `srv/llm/router.rs`, `core/llm.rs` (`ScriptKey`)
- Event write and broadcast: `srv/sqlite/store.rs` (`emit`)
- SSE replay: `srv/api/events.rs`
- SQLite setup: `srv/sqlite/mod.rs` (`begin_write`)
- Restart recovery: `srv/recovery.rs` (`interrupt_unfinished`)
- Agents: `srv/sqlite/agents.rs` (`delete`), `srv/seed.rs`
- Uploads and fetch: `srv/api/files.rs` (`upload`), `srv/tools/fetch.rs`, `srv/config.rs` (`allowlist`)
- Memory: `srv/tools/memory.rs`; tables `memory_entries`, `memory_versions` in `mig`
- Store boundary: `core/store.rs` (`Store`), `srv/api/mod.rs` (`AppState`)
- Tests and CI: `backend/orchestrator-server/tests/it/`, `justfile`, `.github/workflows/ci.yml`
- Loopback guard: `srv/api/origin.rs` (`loopback_only`)
- Live eval harness: `web/eval/`
- ADRs: `docs/adr/0001`–`0008`
