-- Initial schema: the team, the event log with its projections, memory and the mock world.
-- Ids are uuid v7 text except run_events.id; times are RFC 3339 UTC; money is USD micros.

CREATE TABLE agents (
    id               TEXT PRIMARY KEY,
    role             TEXT NOT NULL,             -- lead | specialist
    slug             TEXT NOT NULL,             -- immutable after create
    name             TEXT NOT NULL,
    description      TEXT NOT NULL,
    system_prompt    TEXT NOT NULL,
    model            TEXT NOT NULL,
    reasoning_effort TEXT,
    max_steps        INTEGER NOT NULL,
    icon             TEXT NOT NULL,
    colour           TEXT NOT NULL,
    delegate_targets TEXT NOT NULL DEFAULT '[]', -- JSON array of slugs, Lead only
    version          INTEGER NOT NULL DEFAULT 1,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    deleted_at       TEXT
) STRICT;

CREATE UNIQUE INDEX agents_live_slug ON agents (slug) WHERE deleted_at IS NULL;
-- Backstop for the one-Lead check; 'lead' is to_text(Role::Lead).
CREATE UNIQUE INDEX agents_one_live_lead ON agents (role) WHERE role = 'lead' AND deleted_at IS NULL;

-- No row means the tool is off: not offered to the agent.
CREATE TABLE agent_tools (
    agent_id TEXT NOT NULL REFERENCES agents (id),
    tool     TEXT NOT NULL,
    decision TEXT NOT NULL,                     -- allow | ask | deny
    PRIMARY KEY (agent_id, tool)
) STRICT;

-- One row per run; a task is every run sharing a root_run_id.
CREATE TABLE runs (
    id            TEXT PRIMARY KEY,
    root_run_id   TEXT NOT NULL REFERENCES runs (id), -- own id on a root run
    parent_run_id TEXT REFERENCES runs (id),
    parent_call_id TEXT REFERENCES tool_calls (id), -- the delegate call that started it
    agent_id      TEXT NOT NULL REFERENCES agents (id),
    title         TEXT,
    task          TEXT NOT NULL,                -- the task text
    status        TEXT NOT NULL,                -- RunStatus
    outcome       TEXT,                         -- JSON RunOutcome once finished
    taint_sources TEXT,                         -- JSON, root run only
    started_at    TEXT NOT NULL,
    finished_at   TEXT
) STRICT;

CREATE INDEX runs_root ON runs (root_run_id);

-- Append-only; the id is the SSE event id and the replay key.
CREATE TABLE run_events (
    id          INTEGER PRIMARY KEY,
    root_run_id TEXT NOT NULL REFERENCES runs (id),
    run_id      TEXT NOT NULL REFERENCES runs (id),
    ts          TEXT NOT NULL,
    type        TEXT NOT NULL,
    data        TEXT NOT NULL                   -- JSON
) STRICT;

CREATE INDEX run_events_root ON run_events (root_run_id, type);

CREATE TRIGGER run_events_no_update BEFORE UPDATE ON run_events
BEGIN
    SELECT RAISE(ABORT, 'run_events is append-only');
END;

CREATE TRIGGER run_events_no_delete BEFORE DELETE ON run_events
BEGIN
    SELECT RAISE(ABORT, 'run_events is append-only');
END;

CREATE TABLE tool_calls (
    id             TEXT PRIMARY KEY,
    run_id         TEXT NOT NULL REFERENCES runs (id),
    agent_id       TEXT NOT NULL REFERENCES agents (id), -- copied from the run for the audit filter
    tool           TEXT NOT NULL,
    args           TEXT NOT NULL,               -- JSON
    action         TEXT,                        -- JSON Action; null when check() failed
    decision       TEXT,                        -- null when not offered or invalid
    rules          TEXT NOT NULL DEFAULT '[]',  -- JSON Vec<Rule>
    phase          TEXT NOT NULL,               -- CallPhase
    status         TEXT,                        -- CallStatus once finished
    result_preview TEXT,
    child_run_id   TEXT REFERENCES runs (id),
    duration_ms    INTEGER,
    requested_at   TEXT NOT NULL,
    finished_at    TEXT
) STRICT;

CREATE INDEX tool_calls_run ON tool_calls (run_id);

-- At most one card per call: a repeat is refused without one, a retry is a new call.
CREATE TABLE approvals (
    id           TEXT PRIMARY KEY,
    tool_call_id TEXT NOT NULL UNIQUE REFERENCES tool_calls (id),
    custody      TEXT NOT NULL,                 -- JSON Custody
    preview      TEXT NOT NULL,
    status       TEXT NOT NULL,                 -- ApprovalStatus
    note         TEXT,
    expires_at   TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    resolved_at  TEXT
) STRICT;

CREATE INDEX approvals_status ON approvals (status);

CREATE TABLE memory_entries (
    id              TEXT PRIMARY KEY,
    scope           TEXT NOT NULL,              -- private | shared
    owner_agent_id  TEXT REFERENCES agents (id),
    key             TEXT NOT NULL,
    content         TEXT NOT NULL,
    author_agent_id TEXT NOT NULL REFERENCES agents (id),
    trust           TEXT NOT NULL,
    version         INTEGER NOT NULL,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    CHECK ((scope = 'private') = (owner_agent_id IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX memory_private_key ON memory_entries (owner_agent_id, key) WHERE scope = 'private';
CREATE UNIQUE INDEX memory_shared_key ON memory_entries (key) WHERE scope = 'shared';

CREATE TABLE memory_versions (
    entry_id        TEXT NOT NULL REFERENCES memory_entries (id),
    version         INTEGER NOT NULL,
    content         TEXT NOT NULL,
    author_agent_id TEXT NOT NULL REFERENCES agents (id),
    run_id          TEXT NOT NULL REFERENCES runs (id),
    trust           TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    PRIMARY KEY (entry_id, version)
) STRICT;

-- Metadata only; file content stays on disk in the sandbox.
CREATE TABLE workspace_files (
    path           TEXT PRIMARY KEY,
    trust          TEXT NOT NULL,
    written_by_run TEXT REFERENCES runs (id),   -- null for seeded files
    updated_at     TEXT NOT NULL
) STRICT;

CREATE TABLE inbox_messages (
    id          TEXT PRIMARY KEY,
    sender      TEXT NOT NULL,
    subject     TEXT NOT NULL,
    body        TEXT NOT NULL,
    received_at TEXT NOT NULL
) STRICT;

CREATE TABLE outbox_messages (
    id        TEXT PRIMARY KEY,
    run_id    TEXT NOT NULL REFERENCES runs (id),
    channel   TEXT NOT NULL,
    recipient TEXT NOT NULL,
    subject   TEXT,
    body      TEXT NOT NULL,
    sent_at   TEXT NOT NULL
) STRICT;
