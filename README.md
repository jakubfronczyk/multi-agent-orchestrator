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

Each team has one Lead and its own specialists, and every task runs inside one team. Teams organise work: shared notes and delegation stay within a team, while workspace files, uploads and the inbox are shared and keep their trust label wherever they are read. The approval rules are the same for every team.
