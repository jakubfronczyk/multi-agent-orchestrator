//! Event log reads for SSE replay. Events are written only by `Store::emit`.

use orchestrator_core::ids::EventId;
use orchestrator_core::runs::RunEvent;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::text::{from_text, parse_timestamp};
use crate::events::EventEnvelope;

#[derive(Debug, thiserror::Error)]
pub enum EventReadError {
    #[error("storage failed: {0}")]
    Storage(#[from] sqlx::Error),
    #[error("stored event is unreadable: {0}")]
    Corrupt(String),
}

/// The highest committed event id, or `None` when the log is empty.
pub async fn last_id(pool: &SqlitePool) -> Result<Option<EventId>, EventReadError> {
    let id = sqlx::query_scalar!(r#"SELECT MAX(id) AS "id?: i64" FROM run_events"#)
        .fetch_one(pool)
        .await?;
    Ok(id.map(EventId))
}

/// Every committed event with an id above `after`, in id order.
pub async fn since(
    pool: &SqlitePool,
    after: EventId,
) -> Result<Vec<EventEnvelope>, EventReadError> {
    let rows = sqlx::query!(
        r#"SELECT id AS "id!: i64", root_run_id, run_id, ts, type AS kind, data
           FROM run_events WHERE id > ? ORDER BY id"#,
        after.0
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let data: Value = serde_json::from_str(&row.data).map_err(corrupt)?;
            // Rebuilt in the shape `insert_event` split it from.
            let event: RunEvent = serde_json::from_value(json!({ "type": row.kind, "data": data }))
                .map_err(corrupt)?;
            Ok(EventEnvelope {
                id: EventId(row.id),
                root_run_id: from_text(&row.root_run_id).map_err(corrupt)?,
                run_id: from_text(&row.run_id).map_err(corrupt)?,
                ts: parse_timestamp(&row.ts).map_err(corrupt)?,
                event,
            })
        })
        .collect()
}

fn corrupt(error: impl std::fmt::Display) -> EventReadError {
    EventReadError::Corrupt(error.to_string())
}
