//! Restart recovery (§7.7): work a previous process left unfinished cannot resume, so at boot
//! every pending approval expires, every unfinished call and run is interrupted, each through
//! its terminal event so the log, the tables and any replay agree.

use orchestrator_core::ids::{ApprovalId, RunId, ToolCallId};
use orchestrator_core::runs::{
    ApprovalOutcome, ApprovalStatus, CallPhase, CallStatus, RunEvent, RunOutcome, RunStatus,
};
use orchestrator_core::store::{RunRef, Store, StoreError};
use sqlx::SqlitePool;

use crate::sqlite::text::{from_text, to_text};

#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    #[error("storage failed: {0}")]
    Storage(#[from] sqlx::Error),
    #[error("stored run is unreadable: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Emit(#[from] StoreError),
}

/// Runs before the engine starts, so nothing else writes meanwhile; returns how many runs it
/// interrupted. Children are closed before their parents (newest first), as a live task ends.
pub async fn interrupt_unfinished(
    pool: &SqlitePool,
    store: &dyn Store,
) -> Result<usize, RecoveryError> {
    let running = to_text(&RunStatus::Running).map_err(corrupt)?;
    let finished = to_text(&CallPhase::Finished).map_err(corrupt)?;
    let pending = to_text(&ApprovalStatus::Pending).map_err(corrupt)?;
    let runs = sqlx::query!(
        "SELECT id, root_run_id FROM runs WHERE status = ? ORDER BY started_at DESC, id DESC",
        running
    )
    .fetch_all(pool)
    .await?;

    for run in &runs {
        let at = RunRef {
            root_run_id: from_text::<RunId>(&run.root_run_id).map_err(corrupt)?,
            run_id: from_text::<RunId>(&run.id).map_err(corrupt)?,
        };
        let calls = sqlx::query!(
            r#"SELECT t.id, a.id AS "approval_id?"
               FROM tool_calls t
               LEFT JOIN approvals a ON a.tool_call_id = t.id AND a.status = ?1
               WHERE t.run_id = ?2 AND t.phase != ?3
               ORDER BY t.requested_at, t.id"#,
            pending,
            run.id,
            finished
        )
        .fetch_all(pool)
        .await?;
        for call in calls {
            if let Some(approval) = call.approval_id {
                let approval_id: ApprovalId = from_text(&approval).map_err(corrupt)?;
                let resolved = RunEvent::ApprovalResolved {
                    approval_id,
                    outcome: ApprovalOutcome::Expired,
                    note: None,
                };
                store.emit(at, resolved).await?;
            }
            let call_id: ToolCallId = from_text(&call.id).map_err(corrupt)?;
            let interrupted = RunEvent::ToolCallFinished {
                tainted: false,
                call_id,
                status: CallStatus::Interrupted,
                result_preview: None,
                duration_ms: 0,
                child_run_id: None,
                tool: None,
                args: None,
            };
            store.emit(at, interrupted).await?;
        }
        let finished = RunEvent::RunFinished {
            outcome: RunOutcome::Interrupted,
        };
        store.emit(at, finished).await?;
    }
    Ok(runs.len())
}

fn corrupt(error: impl std::fmt::Display) -> RecoveryError {
    RecoveryError::Corrupt(error.to_string())
}
