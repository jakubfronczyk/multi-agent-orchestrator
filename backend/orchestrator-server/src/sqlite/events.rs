//! Event log reads for SSE replay. Events are written only by `Store::emit`.

use orchestrator_core::ids::{EventId, RunId};
use orchestrator_core::runs::RunEvent;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::read::{ReadError, corrupt, decode, decode_json, decode_time, encode};
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
                root_run_id: decode(&row.root_run_id)?,
                run_id: decode(&row.run_id)?,
                ts: decode_time(&row.ts)?,
                event: event(&row.kind, &row.data)?,
            })
        })
        .collect()
}

/// Every event of the task whose root run is `root`, in id order, each with its run.
pub async fn of_task(pool: &SqlitePool, root: RunId) -> Result<Vec<(RunId, RunEvent)>, ReadError> {
    let root = encode(&root)?;
    let rows = sqlx::query!(
        r#"SELECT run_id, type AS kind, data FROM run_events WHERE root_run_id = ? ORDER BY id"#,
        root
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let run = decode(&row.run_id)?;
            Ok((run, event(&row.kind, &row.data)?))
        })
        .collect()
}

/// Rebuilt in the shape `insert_event` split it from.
fn event(kind: &str, data: &str) -> Result<RunEvent, ReadError> {
    let data: Value = decode_json(data)?;
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
