//! Outbox reads for the API. Messages are written only by `send_message`.

use chrono::{DateTime, Utc};
use orchestrator_core::ids::RunId;
use serde::Serialize;
use sqlx::SqlitePool;
use ts_rs::TS;

use super::read::{ReadError, corrupt};
use super::text::{from_text, parse_timestamp};

const LIST_LIMIT: u32 = 200;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct OutboxMessage {
    pub id: String,
    pub run_id: RunId,
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

/// Sent messages, newest first, at most 200.
pub async fn list(pool: &SqlitePool) -> Result<OutboxList, ReadError> {
    let rows = sqlx::query!(
        "SELECT id, run_id, channel, recipient, subject, body, sent_at
         FROM outbox_messages ORDER BY sent_at DESC, id DESC LIMIT ?",
        LIST_LIMIT
    )
    .fetch_all(pool)
    .await?;
    let messages = rows
        .into_iter()
        .map(|row| {
            Ok(OutboxMessage {
                id: row.id,
                run_id: from_text(&row.run_id).map_err(corrupt)?,
                channel: row.channel,
                recipient: row.recipient,
                subject: row.subject,
                body: row.body,
                sent_at: parse_timestamp(&row.sent_at).map_err(corrupt)?,
            })
        })
        .collect::<Result<Vec<_>, ReadError>>()?;
    Ok(OutboxList { messages })
}
