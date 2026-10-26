-- A path names one file whatever its letter case, as on a case-insensitive disk, so a trust row
-- can never sit beside a second row for the same bytes. Writes refuse a case twin of a recorded
-- path, so on a case-sensitive disk no two files share a row either.
CREATE TABLE workspace_files_nocase (
    path           TEXT PRIMARY KEY COLLATE NOCASE,
    trust          TEXT NOT NULL,
    written_by_run TEXT REFERENCES runs (id),   -- null for seeded files
    updated_at     TEXT NOT NULL
) STRICT;

INSERT INTO workspace_files_nocase SELECT path, trust, written_by_run, updated_at FROM workspace_files;
DROP TABLE workspace_files;
ALTER TABLE workspace_files_nocase RENAME TO workspace_files;
