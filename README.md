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
