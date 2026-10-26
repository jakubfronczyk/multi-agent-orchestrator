# multi-agent-orchestrator

A multi-agent bot on the xAI Grok API, with a Rust backend and a React web app.

## Run

Needs Rust, [just](https://github.com/casey/just), Node 22.22+ and pnpm.

```sh
cp .env.example .env    # add your XAI_API_KEY
just dev                # backend
cd web && pnpm install  # once
just web                # web app
```

`LLM_CONCURRENCY` (default 16) caps model calls in flight across all teams; lower it on a rate-capped key.

## Teams

Each team has one Lead and its own specialists, and every task runs inside one team. Shared notes, delegation and files stay within a team. Each team has its own folder, and every team reads the shared reference folder `data/`, which no agent can change. A team opens another team's folder only if you grant it on Your team, read or read and write. A path out of reach is refused before any approval card, and a revoke applies at a running task's next file step. The inbox is shared, and every file keeps its trust label wherever it is read, so a grant never launders outside content. The approval rules are the same for every team.
