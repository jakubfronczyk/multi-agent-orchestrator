# SQLite in WAL mode, single process

Storage is one SQLite file in WAL mode, used by one server process. There is one local operator, so there is one writer, and `emit` already serializes writes to keep event ids ordered; anyone cloning the repo runs it with zero setup.

## Consequences

- Only one instance can run against the database.
- Storage sits behind the `Store` trait.
