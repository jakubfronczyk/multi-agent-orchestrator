db := "sqlite://var/ci.db"
migrations := "orchestrator-server/migrations"

# install the cargo tools ci and deny need
setup:
    cargo install sqlx-cli --locked --no-default-features --features sqlite,rustls
    cargo install cargo-deny --locked

# run the backend
[working-directory('backend')]
dev:
    cargo run -p orchestrator-server

# run the backend and the web app; Ctrl-C or either one exiting stops both
up:
    #!/usr/bin/env bash
    # background jobs of a script ignore SIGINT, so the trap stops them
    trap 'trap - INT TERM EXIT; kill 0' INT TERM EXIT
    just dev & server=$!
    # the web app starts once the server answers, so its proxy never logs a refused connection
    until curl -sf -o /dev/null http://127.0.0.1:8787/api/health; do
        kill -0 $server 2>/dev/null || exit 1
        sleep 1
    done
    just web & web=$!
    # macOS ships bash 3.2, which has no `wait -n`
    while kill -0 $server 2>/dev/null && kill -0 $web 2>/dev/null; do sleep 1; done

# delete the app database and workspace together; refuses while the server holds them
[working-directory('backend')]
reset:
    @if lsof -nP -iTCP:8787 -sTCP:LISTEN >/dev/null; then echo "the server is running on :8787; stop it first" >&2; exit 1; fi
    rm -f var/orchestrator.db var/orchestrator.db-wal var/orchestrator.db-shm
    rm -rf var/workspace

# run the web app
[working-directory('web')]
web:
    pnpm dev

# run the backend tests
[working-directory('backend')]
test:
    cargo test --workspace

# scripted checks of tools, file access, races and the event stream; free, needs `just up`
[working-directory('web')]
eval-race *args:
    node eval/race.ts {{args}}

# the eval end to end with scripted agents in place of Grok; free, needs `just up`
[working-directory('web')]
eval-dry *args:
    node eval/run.ts --dry {{args}}

# the eval on live Grok; costs money, capped by --budget in USD (default 2), needs `just up`
[working-directory('web')]
eval *args:
    node eval/run.ts {{args}}

# everything that must be green before a commit
[working-directory('backend')]
ci:
    cargo fmt --all --check
    SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings
    SQLX_OFFLINE=true cargo test --workspace
    git diff --exit-code -- ../web/src/generated
    test -z "$(git ls-files --others --exclude-standard -- ../web/src/generated)"
    mkdir -p var
    DATABASE_URL={{db}} cargo sqlx database reset -y --source {{migrations}}
    DATABASE_URL={{db}} cargo sqlx prepare --check --workspace -- --all-targets

# regenerate the offline query cache after any SQL change
[working-directory('backend')]
prepare:
    mkdir -p var
    DATABASE_URL={{db}} cargo sqlx database reset -y --source {{migrations}}
    DATABASE_URL={{db}} cargo sqlx prepare --workspace -- --all-targets

# licenses and advisories; needs network, run at checkpoints
[working-directory('backend')]
deny:
    cargo deny check
