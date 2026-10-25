//! Memory reads for the API. Notes are written only by the memory tools.

use chrono::{DateTime, Utc};
use orchestrator_core::ids::{AgentId, EventId, RunId, TeamId};
use orchestrator_core::policy::Trust;
use orchestrator_core::tools::MemoryScope;
use serde::Serialize;
use sqlx::SqlitePool;
use ts_rs::TS;

use super::events;
use super::read::{ReadError, decode, decode_time, encode_opt, text_opt};
use super::text::escape_like;

const LIST_LIMIT: u32 = 200;

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
    pub author_agent_id: AgentId,
    pub author_slug: String,
    pub trust: Trust,
    pub version: i64,
    /// The task that wrote the current version.
    pub root_run_id: RunId,
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
    pub author_agent_id: AgentId,
    pub author_slug: String,
    pub run_id: RunId,
    /// The task's root run, so a row can open its task.
    pub root_run_id: RunId,
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
                  a.slug AS "author_slug!", e.trust, e.version, e.created_at, e.updated_at,
                  r.root_run_id AS "root_run_id!"
           FROM memory_entries e
           JOIN agents a ON a.id = e.author_agent_id
           JOIN memory_versions v ON v.entry_id = e.id AND v.version = e.version
           JOIN runs r ON r.id = v.run_id
           WHERE (?1 IS NULL OR e.scope = ?1) AND (?2 IS NULL OR e.owner_agent_id = ?2)
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
                author_agent_id: decode(&row.author_agent_id)?,
                author_slug: row.author_slug,
                trust: decode(&row.trust)?,
                version: row.version,
                root_run_id: decode(&row.root_run_id)?,
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
        r#"SELECT v.version, v.content, v.author_agent_id, a.slug AS "author_slug!", v.run_id,
                  r.root_run_id, v.trust, v.created_at
           FROM memory_versions v
           JOIN agents a ON a.id = v.author_agent_id
           JOIN runs r ON r.id = v.run_id
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
                author_agent_id: decode(&row.author_agent_id)?,
                author_slug: row.author_slug,
                run_id: decode(&row.run_id)?,
                root_run_id: decode(&row.root_run_id)?,
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
