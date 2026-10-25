//! Memory tools (§5.1): notes private to an agent or shared by the team, every version kept.
//! The owner and author always come from `ToolCtx`, never from arguments. A note stored
//! untrusted is returned wrapped as data; reading it does not taint the reader's task.

use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use orchestrator_core::policy::{Trust, untrusted};
use orchestrator_core::tools::{
    Action, MemoryScope, NoteKey, Target, Tool, ToolCtx, ToolError, ToolName, ToolOutput, ToolSpec,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use uuid::Uuid;

use super::{parse, spec, storage_failed};
use crate::sqlite::begin_write;
use crate::sqlite::text::{timestamp, to_text};

const TIMEOUT: Duration = Duration::from_secs(2);
const CONTENT_BYTES: usize = 16 * 1024;
const SEARCH_LIMIT: i64 = 20;
/// Search shows the start of each note; memory_read returns the whole note.
const SEARCH_NOTE_CHARS: usize = 1000;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    /// `private` for your own notes, `shared` for the team's.
    scope: MemoryScope,
    /// The note's key: 1 to 128 of a-z, 0-9, _ . / and -.
    key: NoteKey,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    /// `private` for a note only you can read, `shared` for the whole team.
    scope: MemoryScope,
    /// The note's key: 1 to 128 of a-z, 0-9, _ . / and -. Writing an existing key makes a new version.
    key: NoteKey,
    /// The note, at most 16 KB.
    content: String,
}

#[derive(Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SearchScope {
    Private,
    Shared,
    All,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    /// Words to find in keys and contents, matched literally.
    query: String,
    /// `private` for your notes, `shared` for the team's, `all` for both.
    scope: SearchScope,
}

pub struct MemoryRead {
    pool: SqlitePool,
    spec: ToolSpec,
}

pub struct MemorySearch {
    pool: SqlitePool,
    spec: ToolSpec,
}

pub struct MemoryWrite {
    pool: SqlitePool,
    spec: ToolSpec,
}

impl MemoryRead {
    pub fn new(pool: SqlitePool) -> Self {
        let purpose = "Read one note by scope and key. Use it when you know the key; use memory_search when you do not.";
        Self {
            pool,
            spec: spec::<ReadArgs>(ToolName::MemoryRead, purpose, TIMEOUT),
        }
    }
}

impl MemorySearch {
    pub fn new(pool: SqlitePool) -> Self {
        let purpose = "Find notes whose key or content contains the query. Do not use it to read a key you already know.";
        Self {
            pool,
            spec: spec::<SearchArgs>(ToolName::MemorySearch, purpose, TIMEOUT),
        }
    }
}

impl MemoryWrite {
    pub fn new(pool: SqlitePool) -> Self {
        let purpose = "Save a note for later tasks. Shared notes are visible to every teammate; do not store secrets.";
        Self {
            pool,
            spec: spec::<WriteArgs>(ToolName::MemoryWrite, purpose, TIMEOUT),
        }
    }
}

/// The owner a lookup in this scope uses: the caller for private notes, none for shared.
fn owner(scope: MemoryScope, ctx: &ToolCtx) -> Result<Option<String>, ToolError> {
    match scope {
        MemoryScope::Private => to_text(&ctx.agent_id).map(Some).map_err(storage_failed),
        MemoryScope::Shared => Ok(None),
    }
}

fn note_target(scope: MemoryScope, key: NoteKey, verb: &str) -> Action {
    Action {
        verb: verb.to_string(),
        target: Target::Note { scope, key },
    }
}

#[async_trait]
impl Tool for MemoryRead {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn check(&self, args: &Value) -> Result<Action, ToolError> {
        let args: ReadArgs = parse(args)?;
        Ok(note_target(args.scope, args.key, "read"))
    }

    /// Another agent's private note reads exactly like a missing one.
    async fn run(&self, ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: ReadArgs = parse(&args)?;
        let scope = to_text(&args.scope).map_err(storage_failed)?;
        let owner = owner(args.scope, ctx)?;
        let key = args.key.as_str();
        let row = sqlx::query!(
            r#"SELECT e.content, e.trust, e.version, a.slug AS "author!"
               FROM memory_entries e JOIN agents a ON a.id = e.author_agent_id
               WHERE e.scope = ? AND e.key = ? AND e.owner_agent_id IS ?"#,
            scope,
            key,
            owner
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failed)?;
        let content = match row {
            Some(row) => json!({
                "found": true,
                "key": key,
                "content": as_stored(key, &row.content, &row.trust),
                "author": row.author,
                "trust": row.trust,
                "version": row.version,
            }),
            None => json!({ "found": false, "key": key }),
        };
        Ok(ToolOutput {
            content,
            taint: Vec::new(),
        })
    }
}

#[async_trait]
impl Tool for MemorySearch {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn check(&self, args: &Value) -> Result<Action, ToolError> {
        let args: SearchArgs = parse(args)?;
        if args.query.trim().is_empty() {
            return Err(ToolError::InvalidArgs(
                "query: must not be empty".to_string(),
            ));
        }
        Ok(Action {
            verb: "search".to_string(),
            target: Target::Query { text: args.query },
        })
    }

    /// `all` is the caller's private notes plus shared notes, never another agent's private notes.
    async fn run(&self, ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: SearchArgs = parse(&args)?;
        let (private, shared) = match args.scope {
            SearchScope::Private => (true, false),
            SearchScope::Shared => (false, true),
            SearchScope::All => (true, true),
        };
        let pattern = format!("%{}%", escape_like(&args.query));
        let private_scope = to_text(&MemoryScope::Private).map_err(storage_failed)?;
        let shared_scope = to_text(&MemoryScope::Shared).map_err(storage_failed)?;
        let caller = to_text(&ctx.agent_id).map_err(storage_failed)?;
        let rows = sqlx::query!(
            r#"SELECT e.scope, e.key, e.content, e.trust, a.slug AS "author!"
               FROM memory_entries e JOIN agents a ON a.id = e.author_agent_id
               WHERE (e.key LIKE ?1 ESCAPE '\' OR e.content LIKE ?1 ESCAPE '\')
                 AND ((?2 AND e.scope = ?3) OR (?4 AND e.scope = ?5 AND e.owner_agent_id = ?6))
               ORDER BY e.updated_at DESC
               LIMIT ?7"#,
            pattern,
            shared,
            shared_scope,
            private,
            private_scope,
            caller,
            SEARCH_LIMIT
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failed)?;
        let notes: Vec<Value> = rows
            .into_iter()
            .map(|row| {
                let (start, cut) = match row.content.char_indices().nth(SEARCH_NOTE_CHARS) {
                    Some((end, _)) => (&row.content[..end], true),
                    None => (row.content.as_str(), false),
                };
                json!({
                    "scope": row.scope,
                    "key": row.key,
                    "content": as_stored(&row.key, start, &row.trust),
                    "truncated": cut,
                    "author": row.author,
                    "trust": row.trust,
                })
            })
            .collect();
        Ok(ToolOutput {
            content: json!({ "notes": notes }),
            taint: Vec::new(),
        })
    }
}

#[async_trait]
impl Tool for MemoryWrite {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn check(&self, args: &Value) -> Result<Action, ToolError> {
        let args: WriteArgs = parse(args)?;
        if args.content.len() > CONTENT_BYTES {
            return Err(ToolError::InvalidArgs("content: at most 16 KB".to_string()));
        }
        Ok(note_target(args.scope, args.key, "write"))
    }

    /// One transaction: the upsert bumps the version, so parallel writes to one key never
    /// share a version number, and the version row records who wrote it in which run.
    async fn run(&self, ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: WriteArgs = parse(&args)?;
        let scope = to_text(&args.scope).map_err(storage_failed)?;
        let owner = owner(args.scope, ctx)?;
        let author = to_text(&ctx.agent_id).map_err(storage_failed)?;
        let run = to_text(&ctx.run_id).map_err(storage_failed)?;
        let trust = to_text(&ctx.trust).map_err(storage_failed)?;
        let id = Uuid::now_v7().to_string();
        let key = args.key.as_str();
        let now = timestamp(Utc::now());

        let mut tx = begin_write(&self.pool).await.map_err(storage_failed)?;
        // The conflict targets repeat the partial indexes' predicates exactly, as SQLite requires.
        let stored = match args.scope {
            MemoryScope::Private => sqlx::query!(
                r#"INSERT INTO memory_entries
                     (id, scope, owner_agent_id, key, content, author_agent_id, trust, version, created_at, updated_at)
                   VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?)
                   ON CONFLICT (owner_agent_id, key) WHERE scope = 'private' DO UPDATE SET
                     content = excluded.content, author_agent_id = excluded.author_agent_id,
                     trust = excluded.trust, version = memory_entries.version + 1, updated_at = excluded.updated_at
                   RETURNING id, version"#,
                id,
                scope,
                owner,
                key,
                args.content,
                author,
                trust,
                now,
                now
            )
            .fetch_one(&mut *tx)
            .await
            .map(|row| (row.id, row.version)),
            MemoryScope::Shared => sqlx::query!(
                r#"INSERT INTO memory_entries
                     (id, scope, owner_agent_id, key, content, author_agent_id, trust, version, created_at, updated_at)
                   VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?)
                   ON CONFLICT (key) WHERE scope = 'shared' DO UPDATE SET
                     content = excluded.content, author_agent_id = excluded.author_agent_id,
                     trust = excluded.trust, version = memory_entries.version + 1, updated_at = excluded.updated_at
                   RETURNING id, version"#,
                id,
                scope,
                owner,
                key,
                args.content,
                author,
                trust,
                now,
                now
            )
            .fetch_one(&mut *tx)
            .await
            .map(|row| (row.id, row.version)),
        }
        .map_err(storage_failed)?;
        let (entry_id, version) = stored;
        sqlx::query!(
            "INSERT INTO memory_versions (entry_id, version, content, author_agent_id, run_id, trust, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            entry_id,
            version,
            args.content,
            author,
            run,
            trust,
            now
        )
        .execute(&mut *tx)
        .await
        .map_err(storage_failed)?;
        tx.commit().await.map_err(storage_failed)?;

        Ok(ToolOutput {
            content: json!({ "key": key, "version": version, "trust": trust }),
            taint: Vec::new(),
        })
    }
}

/// Untrusted text goes back as data, so a clean task never reads it as instructions.
fn as_stored(key: &str, content: &str, trust: &str) -> String {
    match to_text(&Trust::Untrusted) {
        Ok(untrusted_text) if trust == untrusted_text => untrusted(&format!("note:{key}"), content),
        Ok(_) | Err(_) => content.to_string(),
    }
}

/// `%`, `_` and the escape character itself match literally.
fn escape_like(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}
