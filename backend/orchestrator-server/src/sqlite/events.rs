//! Event log reads for SSE replay. Events are written only by `Store::emit`.

use orchestrator_core::ids::{EventId, RunId};
use orchestrator_core::runs::RunEvent;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::read::{ReadError, corrupt};
use super::text::{from_text, parse_timestamp, to_text};
use crate::events::EventEnvelope;

/// Stored `type` values the reads filter on; a test checks serde writes the same strings.
pub const LLM_RESPONDED: &str = "llm_responded";
pub const PLAN_RECORDED: &str = "plan_recorded";

/// The highest committed event id, or `None` when the log is empty. Takes a pool or an open
/// transaction, so a snapshot can read it first.
pub async fn last_id<'e>(
    executor: impl sqlx::SqliteExecutor<'e>,
) -> Result<Option<EventId>, ReadError> {
    let id = sqlx::query_scalar!(r#"SELECT MAX(id) AS "id?: i64" FROM run_events"#)
        .fetch_one(executor)
        .await?;
    Ok(id.map(EventId))
}

/// Every committed event with an id above `after`, in id order.
pub async fn since(pool: &SqlitePool, after: EventId) -> Result<Vec<EventEnvelope>, ReadError> {
    let rows = sqlx::query!(
        r#"SELECT id AS "id!: i64", root_run_id, run_id, ts, type AS kind, data
           FROM run_events WHERE id > ? ORDER BY id"#,
        after.0
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(EventEnvelope {
                id: EventId(row.id),
                root_run_id: from_text(&row.root_run_id).map_err(corrupt)?,
                run_id: from_text(&row.run_id).map_err(corrupt)?,
                ts: parse_timestamp(&row.ts).map_err(corrupt)?,
                event: event(&row.kind, &row.data)?,
            })
        })
        .collect()
}

/// Every event of the task whose root run is `root`, in id order, each with its run.
pub async fn of_task(pool: &SqlitePool, root: RunId) -> Result<Vec<(RunId, RunEvent)>, ReadError> {
    let root = to_text(&root).map_err(corrupt)?;
    let rows = sqlx::query!(
        r#"SELECT run_id, type AS kind, data FROM run_events WHERE root_run_id = ? ORDER BY id"#,
        root
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let run = from_text(&row.run_id).map_err(corrupt)?;
            Ok((run, event(&row.kind, &row.data)?))
        })
        .collect()
}

/// Rebuilt in the shape `insert_event` split it from.
fn event(kind: &str, data: &str) -> Result<RunEvent, ReadError> {
    let data: Value = serde_json::from_str(data).map_err(corrupt)?;
    serde_json::from_value(json!({ "type": kind, "data": data })).map_err(corrupt)
}

#[cfg(test)]
mod tests {
    use orchestrator_core::money::Micros;
    use orchestrator_core::runs::Usage;

    use super::*;

    #[test]
    fn the_type_constants_match_what_serde_stores() {
        let kind = |event: RunEvent| serde_json::to_value(event).expect("json")["type"].clone();
        let responded = RunEvent::LlmResponded {
            step: 0,
            text: None,
            usage: Usage::default(),
            cost: Micros(0),
        };
        assert_eq!(kind(responded), LLM_RESPONDED);
        assert_eq!(
            kind(RunEvent::PlanRecorded { steps: Vec::new() }),
            PLAN_RECORDED
        );
    }
}
