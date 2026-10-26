//! Notes for the API: reads, and the person's own writes (create, edit, delete, mark as trusted).
//! Agents write notes only through the memory tools.

use chrono::{DateTime, Utc};
use orchestrator_core::ids::{AgentId, EventId, RunId, TeamId};
use orchestrator_core::policy::Trust;
use orchestrator_core::tools::{MemoryScope, NoteKey};
use serde::Serialize;
use sqlx::SqlitePool;
use ts_rs::TS;
use uuid::Uuid;

use super::begin_write;
use super::events;
use super::read::{ReadError, decode, decode_time, encode_opt, text_opt};
use super::text::{escape_like, timestamp, to_text};

const LIST_LIMIT: u32 = 200;
/// The same cap as an agent's note.
const NOTE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct MemoryEntryView {
    pub id: String,
    pub scope: MemoryScope,
    /// The team the note belongs to; a private note's is its owner's.
    pub team_id: TeamId,
    /// The agent a private note belongs to; `None` for shared notes.
    pub owner_agent_id: Option<AgentId>,
    pub key: String,
    pub content: String,
    /// `None` when the person wrote the current version.
    pub author_agent_id: Option<AgentId>,
    pub author_slug: Option<String>,
    pub trust: Trust,
    pub version: i64,
    /// The task that wrote the current version; `None` when the person did, outside any task.
    pub root_run_id: Option<RunId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// `last_event_id` is read before the list, as in `RunList`, and stays global when filtered.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct MemoryList {
    pub entries: Vec<MemoryEntryView>,
    pub last_event_id: Option<EventId>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct MemoryVersionView {
    pub version: i64,
    pub content: String,
    /// `None` for a version the person wrote.
    pub author_agent_id: Option<AgentId>,
    pub author_slug: Option<String>,
    pub run_id: Option<RunId>,
    /// The task's root run, so a row can open its task.
    pub root_run_id: Option<RunId>,
    pub trust: Trust,
    pub created_at: DateTime<Utc>,
}

/// Every version of one note, newest first.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct MemoryVersionList {
    pub entry_id: String,
    pub versions: Vec<MemoryVersionView>,
}

/// List filters; each one that is set must match.
#[derive(Debug, Clone, Default)]
pub struct MemoryFilter {
    pub scope: Option<MemoryScope>,
    pub team_id: Option<TeamId>,
    /// The owner of private notes; shared notes have none, so they never match.
    pub agent_id: Option<AgentId>,
    /// Matched literally against key and content.
    pub q: Option<String>,
}

/// Notes, most recently updated first, at most 200.
pub async fn list(pool: &SqlitePool, filter: MemoryFilter) -> Result<MemoryList, ReadError> {
    let scope = encode_opt(filter.scope.as_ref())?;
    let owner = encode_opt(filter.agent_id.as_ref())?;
    let team = encode_opt(filter.team_id.as_ref())?;
    let pattern = filter.q.as_deref().map(|q| format!("%{}%", escape_like(q)));
    let mut tx = pool.begin().await?;
    let last_event_id = events::last_id(&mut *tx).await?;
    let rows = sqlx::query!(
        r#"SELECT e.id, e.scope, e.team_id, e.owner_agent_id, e.key, e.content, e.author_agent_id,
                  a.slug AS "author_slug?", e.trust, e.version, e.created_at, e.updated_at,
                  r.root_run_id AS "root_run_id?"
           FROM memory_entries e
           LEFT JOIN agents a ON a.id = e.author_agent_id
           JOIN memory_versions v ON v.entry_id = e.id AND v.version = e.version
           LEFT JOIN runs r ON r.id = v.run_id
           WHERE e.deleted_at IS NULL
             AND (?1 IS NULL OR e.scope = ?1) AND (?2 IS NULL OR e.owner_agent_id = ?2)
             AND (?3 IS NULL OR e.key LIKE ?3 ESCAPE '\' OR e.content LIKE ?3 ESCAPE '\')
             AND (?5 IS NULL OR e.team_id = ?5)
           ORDER BY e.updated_at DESC, e.id
           LIMIT ?4"#,
        scope,
        owner,
        pattern,
        LIST_LIMIT,
        team
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let entries = rows
        .into_iter()
        .map(|row| {
            Ok(MemoryEntryView {
                id: row.id,
                scope: decode(&row.scope)?,
                team_id: decode(&row.team_id)?,
                owner_agent_id: text_opt(row.owner_agent_id.as_deref())?,
                key: row.key,
                content: row.content,
                author_agent_id: text_opt(row.author_agent_id.as_deref())?,
                author_slug: row.author_slug,
                trust: decode(&row.trust)?,
                version: row.version,
                root_run_id: text_opt(row.root_run_id.as_deref())?,
                created_at: decode_time(&row.created_at)?,
                updated_at: decode_time(&row.updated_at)?,
            })
        })
        .collect::<Result<Vec<_>, ReadError>>()?;
    Ok(MemoryList {
        entries,
        last_event_id,
    })
}

/// The note's versions, or `None` when no note has this id.
pub async fn versions(
    pool: &SqlitePool,
    entry_id: &str,
) -> Result<Option<MemoryVersionList>, ReadError> {
    let mut tx = pool.begin().await?;
    let exists = sqlx::query_scalar!(
        r#"SELECT 1 AS "found!: i64" FROM memory_entries WHERE id = ?"#,
        entry_id
    )
    .fetch_optional(&mut *tx)
    .await?;
    if exists.is_none() {
        return Ok(None);
    }
    let rows = sqlx::query!(
        r#"SELECT v.version, v.content, v.author_agent_id, a.slug AS "author_slug?", v.run_id,
                  r.root_run_id AS "root_run_id?", v.trust, v.created_at
           FROM memory_versions v
           LEFT JOIN agents a ON a.id = v.author_agent_id
           LEFT JOIN runs r ON r.id = v.run_id
           WHERE v.entry_id = ?
           ORDER BY v.version DESC"#,
        entry_id
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let versions = rows
        .into_iter()
        .map(|row| {
            Ok(MemoryVersionView {
                version: row.version,
                content: row.content,
                author_agent_id: text_opt(row.author_agent_id.as_deref())?,
                author_slug: row.author_slug,
                run_id: text_opt(row.run_id.as_deref())?,
                root_run_id: text_opt(row.root_run_id.as_deref())?,
                trust: decode(&row.trust)?,
                created_at: decode_time(&row.created_at)?,
            })
        })
        .collect::<Result<Vec<_>, ReadError>>()?;
    Ok(Some(MemoryVersionList {
        entry_id: entry_id.to_string(),
        versions,
    }))
}

/// Why a person's note write was refused.
#[derive(Debug, thiserror::Error)]
pub enum NoteError {
    #[error("{field}: {message}")]
    Invalid {
        field: &'static str,
        message: String,
    },
    #[error("note not found")]
    NotFound,
    /// The note changed since the person loaded it.
    #[error("the note changed since you opened it")]
    Stale,
    #[error("{0}")]
    Conflict(&'static str),
    #[error("storage failed: {0}")]
    Storage(#[from] sqlx::Error),
    #[error(transparent)]
    Read(#[from] ReadError),
}

/// The live note a person may change: shared notes only for an edit or a mark, any note for a
/// delete. The version must still be the one the person saw.
struct Live {
    scope: String,
    content: String,
    trust: String,
    version: i64,
}

async fn live(
    tx: &mut sqlx::SqliteConnection,
    id: &str,
    expected_version: i64,
) -> Result<Live, NoteError> {
    let row = sqlx::query!(
        "SELECT e.scope, e.content, e.trust, e.version FROM memory_entries e
         JOIN teams t ON t.id = e.team_id
         WHERE e.id = ? AND e.deleted_at IS NULL AND t.deleted_at IS NULL",
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(NoteError::NotFound)?;
    if row.version != expected_version {
        return Err(NoteError::Stale);
    }
    Ok(Live {
        scope: row.scope,
        content: row.content,
        trust: row.trust,
        version: row.version,
    })
}

/// A new version written by the person: no author agent, no run.
async fn person_version(
    tx: &mut sqlx::SqliteConnection,
    id: &str,
    version: i64,
    content: &str,
    trust: &str,
    now: &str,
) -> Result<(), NoteError> {
    sqlx::query!(
        "UPDATE memory_entries SET content = ?, author_agent_id = NULL, trust = ?, version = ?, updated_at = ?
         WHERE id = ?",
        content,
        trust,
        version,
        now,
        id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "INSERT INTO memory_versions (entry_id, version, content, author_agent_id, run_id, trust, created_at)
         VALUES (?, ?, ?, NULL, NULL, ?, ?)",
        id,
        version,
        content,
        trust,
        now
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

fn check_content(content: &str) -> Result<(), NoteError> {
    if content.trim().is_empty() || content.len() > NOTE_BYTES {
        return Err(NoteError::Invalid {
            field: "content",
            message: "use 1 character to 16 KB".to_string(),
        });
    }
    Ok(())
}

/// The note's id after a write, read back as the list shows it.
async fn view(pool: &SqlitePool, id: &str) -> Result<MemoryEntryView, NoteError> {
    let team = sqlx::query_scalar!("SELECT team_id FROM memory_entries WHERE id = ?", id)
        .fetch_one(pool)
        .await?;
    let team: TeamId = decode(&team)?;
    list(
        pool,
        MemoryFilter {
            team_id: Some(team),
            ..MemoryFilter::default()
        },
    )
    .await?
    .entries
    .into_iter()
    .find(|entry| entry.id == id)
    .ok_or(NoteError::NotFound)
}

/// A shared note the person writes for a live team: trusted, since they are the person's own
/// words. A deleted note with the key is revived as its next version; a live one is a conflict.
pub async fn create(
    pool: &SqlitePool,
    team: TeamId,
    key: &str,
    content: &str,
) -> Result<MemoryEntryView, NoteError> {
    let key = NoteKey::new(key)
        .map_err(|e| NoteError::Invalid {
            field: "key",
            message: e.message,
        })?
        .as_str()
        .to_string();
    check_content(content)?;
    let team_text =
        to_text(&team).map_err(|e| NoteError::Read(ReadError::Corrupt(e.to_string())))?;
    let trusted =
        to_text(&Trust::Trusted).map_err(|e| NoteError::Read(ReadError::Corrupt(e.to_string())))?;
    let shared = to_text(&MemoryScope::Shared)
        .map_err(|e| NoteError::Read(ReadError::Corrupt(e.to_string())))?;
    let now = timestamp(Utc::now());
    let mut tx = begin_write(pool).await?;
    let live_team = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM teams WHERE id = ? AND deleted_at IS NULL",
        team_text
    )
    .fetch_one(&mut *tx)
    .await?;
    if live_team == 0 {
        return Err(NoteError::NotFound);
    }
    let existing = sqlx::query!(
        "SELECT id, version, deleted_at FROM memory_entries WHERE scope = 'shared' AND team_id = ? AND key = ?",
        team_text,
        key
    )
    .fetch_optional(&mut *tx)
    .await?;
    let id = match existing {
        Some(row) if row.deleted_at.is_none() => {
            return Err(NoteError::Conflict(
                "the team already has a note with this key",
            ));
        }
        Some(row) => {
            sqlx::query!(
                "UPDATE memory_entries SET deleted_at = NULL WHERE id = ?",
                row.id
            )
            .execute(&mut *tx)
            .await?;
            person_version(&mut tx, &row.id, row.version + 1, content, &trusted, &now).await?;
            row.id
        }
        None => {
            let id = Uuid::now_v7().to_string();
            sqlx::query!(
                "INSERT INTO memory_entries
                   (id, scope, team_id, owner_agent_id, key, content, author_agent_id, trust, version, created_at, updated_at)
                 VALUES (?, ?, ?, NULL, ?, ?, NULL, ?, 0, ?, ?)",
                id,
                shared,
                team_text,
                key,
                content,
                trusted,
                now,
                now
            )
            .execute(&mut *tx)
            .await?;
            person_version(&mut tx, &id, 1, content, &trusted, &now).await?;
            id
        }
    };
    tx.commit().await?;
    view(pool, &id).await
}

/// A new version of a shared note in the person's words. The label stays: the editor starts from
/// the old text, so an untrusted note's injected part may still be in it.
pub async fn edit(
    pool: &SqlitePool,
    id: &str,
    content: &str,
    expected_version: i64,
) -> Result<MemoryEntryView, NoteError> {
    check_content(content)?;
    let now = timestamp(Utc::now());
    let mut tx = begin_write(pool).await?;
    let note = live(&mut tx, id, expected_version).await?;
    if note.scope != "shared" {
        return Err(NoteError::Conflict(
            "a private note is its agent's own; it can be deleted, not edited",
        ));
    }
    person_version(&mut tx, id, note.version + 1, content, &note.trust, &now).await?;
    tx.commit().await?;
    view(pool, id).await
}

/// Hides the note from every reader; its versions stay for the record.
pub async fn delete(pool: &SqlitePool, id: &str, expected_version: i64) -> Result<(), NoteError> {
    let now = timestamp(Utc::now());
    let mut tx = begin_write(pool).await?;
    live(&mut tx, id, expected_version).await?;
    sqlx::query!(
        "UPDATE memory_entries SET deleted_at = ? WHERE id = ?",
        now,
        id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The person vouches for the text: a new version, same content, trusted. A task that already
/// read the note stays tainted; nothing clears taint.
pub async fn mark_trusted(
    pool: &SqlitePool,
    id: &str,
    expected_version: i64,
) -> Result<MemoryEntryView, NoteError> {
    let trusted =
        to_text(&Trust::Trusted).map_err(|e| NoteError::Read(ReadError::Corrupt(e.to_string())))?;
    let now = timestamp(Utc::now());
    let mut tx = begin_write(pool).await?;
    let note = live(&mut tx, id, expected_version).await?;
    if note.scope != "shared" {
        return Err(NoteError::Conflict(
            "a private note is its agent's own; it can be deleted, not marked",
        ));
    }
    if note.trust == trusted {
        return Err(NoteError::Conflict("the note is already trusted"));
    }
    person_version(&mut tx, id, note.version + 1, &note.content, &trusted, &now).await?;
    tx.commit().await?;
    view(pool, id).await
}
