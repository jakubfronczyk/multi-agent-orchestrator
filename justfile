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

# run the web app
[working-directory('web')]
web:
    pnpm dev

# run the backend tests
[working-directory('backend')]
test:
    cargo test --workspace

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
