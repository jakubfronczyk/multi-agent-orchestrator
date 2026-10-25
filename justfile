# run the backend
dev:
    cd backend && cargo run -p orchestrator-server

# run the web app
web:
    cd web && pnpm dev

# run the backend tests
test:
    cd backend && cargo test --workspace
