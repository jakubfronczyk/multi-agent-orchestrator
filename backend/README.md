# backend

Rust workspace with two crates:

- `orchestrator-core`: library with the domain logic, no I/O
- `orchestrator-server`: binary that runs the HTTP server

## Run

From the repo root:

```sh
just dev    # run the server
just test   # run the tests
```
