//! Memory tools (§5.1): notes private to an agent or shared by the team, every version kept,
//! with owner and author from `ToolCtx`. A note stored untrusted taints the task that reads it,
//! and the Lead's reads exclude untrusted notes in SQL, as if they were missing.

use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use orchestrator_core::agents::Role;
use orchestrator_core::policy::{TaintSource, Trust};
use orchestrator_core::tools::{
    Action, MemoryScope, NoteKey, Target, Tool, ToolCtx, ToolError, ToolName, ToolOutput, ToolSpec,
    parse_args,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use uuid::Uuid;

use super::{is_trusted, spec, storage_failed};
use crate::sqlite::begin_write;
use crate::sqlite::text::{from_text, timestamp, to_text};

const TIMEOUT: Duration = Duration::from_secs(2);
const CONTENT_BYTES: usize = 16 * 1024;
const SEARCH_LIMIT: usize = 20;
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
    /// Words to find in keys and contents. A note matching any word is returned, those matching
    /// the most words first; `OR`, `AND`, quotes and punctuation are ignored.
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

/// The Lead sees trusted notes only; the filter is in each query, so no untrusted row leaves SQLite.
fn trusted_only(ctx: &ToolCtx) -> Result<(bool, String), ToolError> {
    let trusted = to_text(&Trust::Trusted).map_err(storage_failed)?;
    Ok((ctx.role == Role::Lead, trusted))
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
        let args: ReadArgs = parse_args(args)?;
        Ok(note_target(args.scope, args.key, "read"))
    }

    /// Another agent's private note reads exactly like a missing one.
    async fn run(&self, ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: ReadArgs = parse_args(&args)?;
        let scope = to_text(&args.scope).map_err(storage_failed)?;
        let owner = owner(args.scope, ctx)?;
        let key = args.key.as_str();
        let (trusted_only, trusted) = trusted_only(ctx)?;
        // Shared notes are the caller's team's; a private note's owner implies its team.
        let team = to_text(&ctx.team_id).map_err(storage_failed)?;
        let row = sqlx::query!(
            r#"SELECT e.content, e.trust, e.version, a.slug AS "author!"
               FROM memory_entries e JOIN agents a ON a.id = e.author_agent_id
               WHERE e.scope = ? AND e.key = ? AND e.owner_agent_id IS ? AND e.team_id = ?
                 AND (NOT ? OR e.trust = ?)"#,
            scope,
            key,
            owner,
            team,
            trusted_only,
            trusted
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failed)?;
        let Some(row) = row else {
            return Ok(ToolOutput {
                content: json!({ "found": false, "key": key }),
                taint: Vec::new(),
            });
        };
        let taint = if is_trusted(Some(&row.trust)) {
            Vec::new()
        } else {
            vec![TaintSource::Note {
                scope: args.scope,
                key: args.key.clone(),
            }]
        };
        Ok(ToolOutput {
            content: json!({
                "found": true,
                "key": key,
                "content": row.content,
                "author": row.author,
                "trust": row.trust,
                "version": row.version,
            }),
            taint,
        })
    }
}

#[async_trait]
impl Tool for MemorySearch {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn check(&self, args: &Value) -> Result<Action, ToolError> {
        let args: SearchArgs = parse_args(args)?;
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
        let args: SearchArgs = parse_args(&args)?;
        let (private, shared) = match args.scope {
            SearchScope::Private => (true, false),
            SearchScope::Shared => (false, true),
            SearchScope::All => (true, true),
        };
        let words = search_words(&args.query);
        let private_scope = to_text(&MemoryScope::Private).map_err(storage_failed)?;
        let shared_scope = to_text(&MemoryScope::Shared).map_err(storage_failed)?;
        let caller = to_text(&ctx.agent_id).map_err(storage_failed)?;
        let (trusted_only, trusted) = trusted_only(ctx)?;
        let team = to_text(&ctx.team_id).map_err(storage_failed)?;
        let rows = sqlx::query!(
            r#"SELECT e.scope, e.key, e.content, e.trust, a.slug AS "author!"
               FROM memory_entries e JOIN agents a ON a.id = e.author_agent_id
               WHERE ((?1 AND e.scope = ?2) OR (?3 AND e.scope = ?4 AND e.owner_agent_id = ?5))
                 AND (NOT ?6 OR e.trust = ?7)
                 AND e.team_id = ?8
               ORDER BY e.updated_at DESC"#,
            shared,
            shared_scope,
            private,
            private_scope,
            caller,
            trusted_only,
            trusted,
            team
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failed)?;
        // Most words matched first; the stable sort keeps the newest first among equals.
        let mut ranked: Vec<_> = rows
            .into_iter()
            .map(|row| (matched(&words, &row.key, &row.content), row))
            .filter(|(hits, _)| *hits > 0)
            .collect();
        ranked.sort_by_key(|(hits, _)| std::cmp::Reverse(*hits));
        let mut taint = Vec::new();
        let mut notes = Vec::with_capacity(SEARCH_LIMIT);
        for (_, row) in ranked.into_iter().take(SEARCH_LIMIT) {
            // One source per untrusted note returned; trusted notes add none.
            if !is_trusted(Some(&row.trust)) {
                let scope = from_text(&row.scope).map_err(storage_failed)?;
                let key = NoteKey::new(&row.key).map_err(storage_failed)?;
                taint.push(TaintSource::Note { scope, key });
            }
            let (start, cut) = match row.content.char_indices().nth(SEARCH_NOTE_CHARS) {
                Some((end, _)) => (&row.content[..end], true),
                None => (row.content.as_str(), false),
            };
            notes.push(json!({
                "scope": row.scope,
                "key": row.key,
                "content": start,
                "truncated": cut,
                "author": row.author,
                "trust": row.trust,
            }));
        }
        Ok(ToolOutput {
            content: json!({ "notes": notes }),
            taint,
        })
    }
}

#[async_trait]
impl Tool for MemoryWrite {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn check(&self, args: &Value) -> Result<Action, ToolError> {
        let args: WriteArgs = parse_args(args)?;
        if args.content.len() > CONTENT_BYTES {
            return Err(ToolError::InvalidArgs("content: at most 16 KB".to_string()));
        }
        Ok(note_target(args.scope, args.key, "write"))
    }

    /// One transaction: the upsert bumps the version, so parallel writes to one key never
    /// share a version number, and the version row records who wrote it in which run.
    /// The upsert skips an untrusted write over a trusted note in the same statement, so no
    /// trusted note is ever replaced by a tainted task.
    async fn run(&self, ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: WriteArgs = parse_args(&args)?;
        let scope = to_text(&args.scope).map_err(storage_failed)?;
        let owner = owner(args.scope, ctx)?;
        let author = to_text(&ctx.agent_id).map_err(storage_failed)?;
        let run = to_text(&ctx.run_id).map_err(storage_failed)?;
        let trust = to_text(&ctx.trust).map_err(storage_failed)?;
        let team = to_text(&ctx.team_id).map_err(storage_failed)?;
        let trusted = to_text(&Trust::Trusted).map_err(storage_failed)?;
        let untrusted = to_text(&Trust::Untrusted).map_err(storage_failed)?;
        let id = Uuid::now_v7().to_string();
        let key = args.key.as_str();
        let now = timestamp(Utc::now());

        let mut tx = begin_write(&self.pool).await.map_err(storage_failed)?;
        // The conflict targets repeat the partial indexes' predicates exactly, as SQLite requires,
        // so their scope values stay literals.
        let stored = match args.scope {
            MemoryScope::Private => sqlx::query!(
                r#"INSERT INTO memory_entries
                     (id, scope, team_id, owner_agent_id, key, content, author_agent_id, trust, version, created_at, updated_at)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)
                   ON CONFLICT (owner_agent_id, key) WHERE scope = 'private' DO UPDATE SET
                     content = excluded.content, author_agent_id = excluded.author_agent_id,
                     trust = excluded.trust, version = memory_entries.version + 1, updated_at = excluded.updated_at
                   WHERE NOT (memory_entries.trust = ? AND excluded.trust = ?)
                   RETURNING id, version"#,
                id,
                scope,
                team,
                owner,
                key,
                args.content,
                author,
                trust,
                now,
                now,
                trusted,
                untrusted
            )
            .fetch_optional(&mut *tx)
            .await
            .map(|row| row.map(|row| (row.id, row.version))),
            MemoryScope::Shared => sqlx::query!(
                r#"INSERT INTO memory_entries
                     (id, scope, team_id, owner_agent_id, key, content, author_agent_id, trust, version, created_at, updated_at)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)
                   ON CONFLICT (team_id, key) WHERE scope = 'shared' DO UPDATE SET
                     content = excluded.content, author_agent_id = excluded.author_agent_id,
                     trust = excluded.trust, version = memory_entries.version + 1, updated_at = excluded.updated_at
                   WHERE NOT (memory_entries.trust = ? AND excluded.trust = ?)
                   RETURNING id, version"#,
                id,
                scope,
                team,
                owner,
                key,
                args.content,
                author,
                trust,
                now,
                now,
                trusted,
                untrusted
            )
            .fetch_optional(&mut *tx)
            .await
            .map(|row| row.map(|row| (row.id, row.version))),
        }
        .map_err(storage_failed)?;
        let Some((entry_id, version)) = stored else {
            return Err(ToolError::Failed(format!(
                "{key} is a trusted note; a task that read outside content cannot replace it, so use a new key"
            )));
        };
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

/// The query's distinct words, lowercased; connectives and one-letter words are dropped. A
/// query with no such word is searched as one phrase.
fn search_words(query: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for word in query
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
    {
        if word.chars().count() > 1
            && !["or", "and", "not"].contains(&word.as_str())
            && !words.contains(&word)
        {
            words.push(word);
        }
    }
    if words.is_empty() {
        words.push(query.trim().to_lowercase());
    }
    words
}

/// How many of `words` appear in the key or the content, ignoring case.
fn matched(words: &[String], key: &str, content: &str) -> usize {
    let text = format!("{key}\n{content}").to_lowercase();
    words
        .iter()
        .filter(|word| text.contains(word.as_str()))
        .count()
}

#[cfg(test)]
mod search_tests {
    use super::*;

    #[test]
    fn a_search_engine_query_finds_a_note_by_any_word() {
        let words = search_words(r#"invoices OR "big invoices" OR CFO OR 5k"#);
        assert_eq!(words, ["invoices", "big", "cfo", "5k"]);
        let note = "anything over 5k goes to marta lis now, she's the new cfo.";
        assert_eq!(matched(&words, "cfo_approval", note), 2);
        assert_eq!(matched(&words, "lunch", "pizza on friday"), 0);
        assert_eq!(search_words("OR"), ["or"]);
    }
}
