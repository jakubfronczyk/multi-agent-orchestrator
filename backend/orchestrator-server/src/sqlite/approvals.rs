//! Approval reads for the API. Approvals are written only by `Store::emit`.

use chrono::{DateTime, Utc};
use orchestrator_core::ids::{ApprovalId, EventId, RunId, ToolCallId};
use orchestrator_core::runs::{ApprovalStatus, Custody};
use orchestrator_core::tools::Action;
use serde::Serialize;
use sqlx::{SqliteConnection, SqlitePool};
use ts_rs::TS;

use super::events;
use super::read::{ReadError, corrupt, json_opt, time_opt};
use super::text::{from_text, parse_timestamp, to_text};

/// One approval card with what it is about.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ApprovalView {
    pub id: ApprovalId,
    pub tool_call_id: ToolCallId,
    pub run_id: RunId,
    pub tool: String,
    pub action: Option<Action>,
    pub custody: Custody,
    pub preview: String,
    pub status: ApprovalStatus,
    pub note: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// `last_event_id` is read before the list, so applying events with a larger id never skips one.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ApprovalList {
    pub approvals: Vec<ApprovalView>,
    pub last_event_id: Option<EventId>,
}

pub async fn list(
    pool: &SqlitePool,
    status: Option<ApprovalStatus>,
) -> Result<ApprovalList, ReadError> {
    let mut tx = pool.begin().await?;
    let last_event_id = events::last_id(&mut *tx).await?;
    let approvals = views(&mut tx, status, None).await?;
    tx.commit().await?;
    Ok(ApprovalList {
        approvals,
        last_event_id,
    })
}

/// Cards with `status`, or every card when `None`; only one task's when `root` is set.
pub(crate) async fn views(
    conn: &mut SqliteConnection,
    status: Option<ApprovalStatus>,
    root: Option<RunId>,
) -> Result<Vec<ApprovalView>, ReadError> {
    let status = status.as_ref().map(to_text).transpose().map_err(corrupt)?;
    let root = root.as_ref().map(to_text).transpose().map_err(corrupt)?;
    let rows = sqlx::query!(
        r#"SELECT a.id, a.tool_call_id, a.custody, a.preview, a.status, a.note, a.expires_at, a.created_at,
                  a.resolved_at, t.run_id AS "run_id!", t.tool AS "tool!", t.action
           FROM approvals a
           JOIN tool_calls t ON t.id = a.tool_call_id
           JOIN runs r ON r.id = t.run_id
           WHERE (?1 IS NULL OR a.status = ?1) AND (?2 IS NULL OR r.root_run_id = ?2)
           ORDER BY a.created_at, a.id"#,
        status,
        root
    )
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ApprovalView {
                id: from_text(&row.id).map_err(corrupt)?,
                tool_call_id: from_text(&row.tool_call_id).map_err(corrupt)?,
                run_id: from_text(&row.run_id).map_err(corrupt)?,
                tool: row.tool,
                action: json_opt(row.action.as_deref())?,
                custody: serde_json::from_str(&row.custody).map_err(corrupt)?,
                preview: row.preview,
                status: from_text(&row.status).map_err(corrupt)?,
                note: row.note,
                expires_at: parse_timestamp(&row.expires_at).map_err(corrupt)?,
                created_at: parse_timestamp(&row.created_at).map_err(corrupt)?,
                resolved_at: time_opt(row.resolved_at.as_deref())?,
            })
        })
        .collect()
}

/// The stored status, or `None` when no approval has this id.
pub async fn status(
    pool: &SqlitePool,
    id: ApprovalId,
) -> Result<Option<ApprovalStatus>, ReadError> {
    let id = to_text(&id).map_err(corrupt)?;
    let status = sqlx::query_scalar!("SELECT status FROM approvals WHERE id = ?", id)
        .fetch_optional(pool)
        .await?;
    status
        .as_deref()
        .map(from_text)
        .transpose()
        .map_err(corrupt)
}
