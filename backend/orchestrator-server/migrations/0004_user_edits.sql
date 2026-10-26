-- no-transaction
-- The person writes notes and files too: a note or version with no author agent, and a version
-- with no run, is theirs; a file they wrote is marked. A deleted note keeps its versions and is
-- hidden from every reader. SQLite cannot drop NOT NULL in place, so the two note tables are
-- rebuilt the documented way: foreign keys off, rebuild in one transaction, then on again.
PRAGMA foreign_keys = OFF;

BEGIN;

CREATE TABLE memory_entries_new (
    id              TEXT PRIMARY KEY,
    scope           TEXT NOT NULL,              -- private | shared
    team_id         TEXT NOT NULL REFERENCES teams (id), -- a private note: its owner's team
    owner_agent_id  TEXT REFERENCES agents (id),
    key             TEXT NOT NULL,
    content         TEXT NOT NULL,
    author_agent_id TEXT REFERENCES agents (id), -- null: the person wrote the current version
    trust           TEXT NOT NULL,
    version         INTEGER NOT NULL,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    deleted_at      TEXT,                       -- set: hidden from every reader, versions kept
    CHECK ((scope = 'private') = (owner_agent_id IS NOT NULL))
) STRICT;

INSERT INTO memory_entries_new
    (id, scope, team_id, owner_agent_id, key, content, author_agent_id, trust, version, created_at, updated_at)
SELECT id, scope, team_id, owner_agent_id, key, content, author_agent_id, trust, version, created_at, updated_at
FROM memory_entries;
DROP TABLE memory_entries;
ALTER TABLE memory_entries_new RENAME TO memory_entries;

-- A deleted note still holds its key, so an agent writing the key again revives it.
CREATE UNIQUE INDEX memory_private_key ON memory_entries (owner_agent_id, key) WHERE scope = 'private';
CREATE UNIQUE INDEX memory_shared_key ON memory_entries (team_id, key) WHERE scope = 'shared';
CREATE INDEX memory_team ON memory_entries (team_id, updated_at);

CREATE TABLE memory_versions_new (
    entry_id        TEXT NOT NULL REFERENCES memory_entries (id),
    version         INTEGER NOT NULL,
    content         TEXT NOT NULL,
    author_agent_id TEXT REFERENCES agents (id), -- null: the person
    run_id          TEXT REFERENCES runs (id),   -- null: the person, outside any task
    trust           TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    PRIMARY KEY (entry_id, version)
) STRICT;

INSERT INTO memory_versions_new SELECT * FROM memory_versions;
DROP TABLE memory_versions;
ALTER TABLE memory_versions_new RENAME TO memory_versions;

ALTER TABLE workspace_files ADD COLUMN written_by_user INTEGER NOT NULL DEFAULT 0;

COMMIT;

PRAGMA foreign_keys = ON;
