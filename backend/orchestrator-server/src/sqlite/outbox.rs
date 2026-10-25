//! Outbox reads for the API. Messages are written only by `send_message`.

use chrono::{DateTime, Utc};
use orchestrator_core::ids::{RunId, TeamId, ToolCallId};
use serde::Serialize;
use sqlx::SqlitePool;
use ts_rs::TS;

use super::read::{ReadError, decode, decode_time, encode_opt};

const LIST_LIMIT: u32 = 200;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct OutboxMessage {
    pub id: String,
    pub run_id: RunId,
    /// The task's root run, so a row can open its task.
    pub root_run_id: RunId,
    /// The task's team.
    pub team_id: TeamId,
    /// The `send_message` call that sent it; one call sends at most one message.
    pub tool_call_id: ToolCallId,
    /// `email` or `chat`.
    pub channel: String,
    pub recipient: String,
    pub subject: Option<String>,
    pub body: String,
    pub sent_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct OutboxList {
    pub messages: Vec<OutboxMessage>,
}

/// List filters; each one that is set must match.
#[derive(Debug, Clone, Default)]
pub struct OutboxFilter {
    pub team_id: Option<TeamId>,
}

/// Sent messages, newest first, at most 200.
pub async fn list(pool: &SqlitePool, filter: OutboxFilter) -> Result<OutboxList, ReadError> {
    let team = encode_opt(filter.team_id.as_ref())?;
    let rows = sqlx::query!(
        "SELECT o.id, o.run_id, r.root_run_id, r.team_id, o.tool_call_id, o.channel, o.recipient, o.subject,
                o.body, o.sent_at
         FROM outbox_messages o JOIN runs r ON r.id = o.run_id
         WHERE (?1 IS NULL OR r.team_id = ?1)
         ORDER BY o.sent_at DESC, o.id DESC LIMIT ?2",
        team,
        LIST_LIMIT
    )
    .fetch_all(pool)
    .await?;
    let messages = rows
        .into_iter()
        .map(|row| {
            Ok(OutboxMessage {
                id: row.id,
                run_id: decode(&row.run_id)?,
                root_run_id: decode(&row.root_run_id)?,
                team_id: decode(&row.team_id)?,
                tool_call_id: decode(&row.tool_call_id)?,
                channel: row.channel,
                recipient: row.recipient,
                subject: row.subject,
                body: row.body,
                sent_at: decode_time(&row.sent_at)?,
            })
        })
        .collect::<Result<Vec<_>, ReadError>>()?;
    Ok(OutboxList { messages })
}
