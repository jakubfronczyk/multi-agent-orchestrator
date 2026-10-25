//! Memory reads for the API. Notes are written only by the memory tools.

use chrono::{DateTime, Utc};
use orchestrator_core::ids::{AgentId, RunId};
use orchestrator_core::policy::Trust;
use orchestrator_core::tools::MemoryScope;
use serde::Serialize;
use sqlx::SqlitePool;
use ts_rs::TS;

use super::read::{ReadError, corrupt, text_opt};
use super::text::{from_text, parse_timestamp, to_text};
use crate::tools::escape_like;

const LIST_LIMIT: u32 = 200;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct MemoryEntryView {
    pub id: String,
    pub scope: MemoryScope,
    /// The agent a private note belongs to; `None` for shared notes.
    pub owner_agent_id: Option<AgentId>,
    pub key: String,
    pub content: String,
    pub author_agent_id: AgentId,
    pub author_slug: String,
    pub trust: Trust,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct MemoryList {
    pub entries: Vec<MemoryEntryView>,
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
    /// The owner of private notes; shared notes have none, so they never match.
    pub agent_id: Option<AgentId>,
    /// Matched literally against key and content.
    pub q: Option<String>,
}

/// Notes, most recently updated first, at most 200.
pub async fn list(pool: &SqlitePool, filter: MemoryFilter) -> Result<MemoryList, ReadError> {
    let scope = filter
        .scope
        .as_ref()
        .map(to_text)
        .transpose()
        .map_err(corrupt)?;
    let owner = filter
        .agent_id
        .as_ref()
        .map(to_text)
        .transpose()
        .map_err(corrupt)?;
    let pattern = filter.q.as_deref().map(|q| format!("%{}%", escape_like(q)));
    let rows = sqlx::query!(
        r#"SELECT e.id, e.scope, e.owner_agent_id, e.key, e.content, e.author_agent_id,
                  a.slug AS "author_slug!", e.trust, e.version, e.created_at, e.updated_at
           FROM memory_entries e JOIN agents a ON a.id = e.author_agent_id
           WHERE (?1 IS NULL OR e.scope = ?1) AND (?2 IS NULL OR e.owner_agent_id = ?2)
             AND (?3 IS NULL OR e.key LIKE ?3 ESCAPE '\' OR e.content LIKE ?3 ESCAPE '\')
           ORDER BY e.updated_at DESC, e.id
           LIMIT ?4"#,
        scope,
        owner,
        pattern,
        LIST_LIMIT
    )
    .fetch_all(pool)
    .await?;
    let entries = rows
        .into_iter()
        .map(|row| {
            Ok(MemoryEntryView {
                id: row.id,
                scope: from_text(&row.scope).map_err(corrupt)?,
                owner_agent_id: text_opt(row.owner_agent_id.as_deref())?,
                key: row.key,
                content: row.content,
                author_agent_id: from_text(&row.author_agent_id).map_err(corrupt)?,
                author_slug: row.author_slug,
                trust: from_text(&row.trust).map_err(corrupt)?,
                version: row.version,
                created_at: parse_timestamp(&row.created_at).map_err(corrupt)?,
                updated_at: parse_timestamp(&row.updated_at).map_err(corrupt)?,
            })
        })
        .collect::<Result<Vec<_>, ReadError>>()?;
    Ok(MemoryList { entries })
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
                author_agent_id: from_text(&row.author_agent_id).map_err(corrupt)?,
                author_slug: row.author_slug,
                run_id: from_text(&row.run_id).map_err(corrupt)?,
                root_run_id: from_text(&row.root_run_id).map_err(corrupt)?,
                trust: from_text(&row.trust).map_err(corrupt)?,
                created_at: parse_timestamp(&row.created_at).map_err(corrupt)?,
            })
        })
        .collect::<Result<Vec<_>, ReadError>>()?;
    Ok(Some(MemoryVersionList {
        entry_id: entry_id.to_string(),
        versions,
    }))
}
