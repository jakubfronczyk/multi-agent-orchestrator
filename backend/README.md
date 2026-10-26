# backend

Rust workspace: the engine, policy and tools behind `/api`. Design and tradeoffs:
[`../docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md).

```
orchestrator-core     no I/O: domain, engine, policy, Store and LlmProvider traits
        ▲
orchestrator-server   axum, SQLite (sqlx), xAI client, tool implementations, seed
```

The policy is a pure function in core, so it is tested without a database or a server.
Server code implements core's traits; core never names SQLite, HTTP or xAI.

## orchestrator-core

```
src/policy/decide.rs   decide(): strictest of setting, effect, taint, allowlist, draft-only
src/tools.rs           ToolName, the closed tool set; effect(), taints(), lead() are exhaustive
src/agents.rs          agent config, roles, validation, step limits
src/engine/
  run.rs               one run's loop: model step, calls, retries, budget
  gateway.rs           every call: parse, decide, approve, timeout, panic isolation, audit
  approvals.rs         pending cards and their waiters, 300 s expiry
  native.rs            delegate and plan, the tools the engine runs itself
  task.rs              task state: taint, budget, run slots, refused calls
  history.rs           rebuilds a task from its events for a follow-up
  envelope.rs          what the model sees per call: result envelope, untrusted wrapping, caps
  prompts.rs           system prompts and fixed failure texts
src/store.rs           Store trait: emit(event) writes the event and its projections
src/llm.rs             LlmProvider trait, models, scenario keys
src/money.rs           cost in micro-dollars
```

## orchestrator-server

```
src/main.rs            boot: .env, migrate, seed, recover, pick provider, listen 127.0.0.1:8787
src/api/               REST + SSE handlers; origin.rs rejects non-loopback Host and cross-origin writes
src/sqlite/            Store impl and every query; store.rs::emit is the one write path for events
src/llm/xai/           Responses API over reqwest, response parsing, retry classification
src/llm/scripted.rs    fixed replies for the drill, scenarios and tests
src/llm/router.rs      scenario tasks to scripted, free text to xAI when a key is set
src/tools/             the 13 tools; fetch.rs is the SSRF guard, workspace.rs maps team paths
src/tools/convert.rs   xlsx to CSV per sheet, docx to Markdown, pdf to text
src/recovery.rs        on boot, closes unfinished runs as interrupted, expires cards
src/seed.rs            first-start teams, agents, inbox and files from fixtures/
migrations/            schema; triggers make events append-only
fixtures/              seeded inbox, team files, shared data/
tests/it/              integration tests
```

## Commands

From the repo root:

```bash
just dev                                          # run the server
just test                                         # cargo test --workspace
just ci                                           # everything CI checks
just prepare                                      # refresh .sqlx after any SQL change
just deny                                         # licences and advisories
```

From `backend/`:

```bash
cargo test -p orchestrator-core                   # policy and engine units, no database
cargo test -p orchestrator-server --test it taint # one integration file by name filter
RUST_LOG=orchestrator_server=debug cargo run -p orchestrator-server
```

## Tests

`tests/it/` runs the real engine, gateway, tools and SQLite with the scripted provider.
`#[sqlx::test]` gives each test a fresh migrated database, and timings are milliseconds,
so no test calls the network or needs a key. `xai.rs` has one live test that sends every
tool schema to Grok; it is skipped unless `XAI_API_KEY` is set.

## Generated and checked-in files

- **`.sqlx/`**: the offline query cache. Queries are checked at compile time, and CI builds
  with `SQLX_OFFLINE=true`, so run `just prepare` after changing any SQL. `just ci` fails if
  the cache is stale.
- **`../web/src/generated/`**: TypeScript types for every API type, written by `ts-rs` when
  the tests run (`.cargo/config.toml` sets the export directory). `just ci` fails if they
  differ from the Rust types.

## Lints

`unsafe_code = "forbid"` and `clippy::unwrap_used = "deny"` for the whole workspace, and
clippy runs with `-D warnings`.
