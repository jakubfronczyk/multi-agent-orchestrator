//! Approval reads for the API. Approvals are written only by `Store::emit`.

use chrono::{DateTime, Utc};
use orchestrator_core::ids::{ApprovalId, EventId, RunId, ToolCallId};
use orchestrator_core::runs::{ApprovalStatus, Custody};
use orchestrator_core::tools::Action;
use serde::Serialize;
use sqlx::{SqliteConnection, SqlitePool};
use ts_rs::TS;

use super::events;
use super::read::{
    ReadError, decode, decode_json, decode_time, encode, encode_opt, json_opt, text_opt, time_opt,
};

/// One approval card with what it is about.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ApprovalView {
    pub id: ApprovalId,
    pub tool_call_id: ToolCallId,
    pub run_id: RunId,
    /// The task the card belongs to, so a global list can link to its run view.
    pub root_run_id: RunId,
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
    let status = encode_opt(status.as_ref())?;
    let root = encode_opt(root.as_ref())?;
    let rows = sqlx::query!(
        r#"SELECT a.id, a.tool_call_id, a.custody, a.preview, a.status, a.note, a.expires_at, a.created_at,
                  a.resolved_at, t.run_id AS "run_id!", r.root_run_id AS "root_run_id!", t.tool AS "tool!", t.action
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
                id: decode(&row.id)?,
                tool_call_id: decode(&row.tool_call_id)?,
                run_id: decode(&row.run_id)?,
                root_run_id: decode(&row.root_run_id)?,
                tool: row.tool,
                action: json_opt(row.action.as_deref())?,
                custody: decode_json(&row.custody)?,
                preview: row.preview,
                status: decode(&row.status)?,
                note: row.note,
                expires_at: decode_time(&row.expires_at)?,
                created_at: decode_time(&row.created_at)?,
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
    let id = encode(&id)?;
    let status = sqlx::query_scalar!("SELECT status FROM approvals WHERE id = ?", id)
        .fetch_optional(pool)
        .await?;
    text_opt(status.as_deref())
}
