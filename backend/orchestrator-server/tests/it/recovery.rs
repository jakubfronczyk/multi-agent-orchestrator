//! §7.7 restart recovery: what a previous process left open is closed through terminal events.

use chrono::Utc;
use orchestrator_core::agents::Slug;
use orchestrator_core::ids::{ApprovalId, RunId, ToolCallId};
use orchestrator_core::policy::Decision;
use orchestrator_core::runs::{ApprovalOutcome, CallStatus, Custody, RunEvent, RunOutcome};
use orchestrator_core::store::{RunRef, Store};
use orchestrator_core::tools::{Action, Target};
use orchestrator_server::recovery::interrupt_unfinished;
use orchestrator_server::sqlite::SqliteStore;
use serde_json::json;
use sqlx::SqlitePool;

use crate::app;

fn requested(call_id: ToolCallId, tool: &str) -> RunEvent {
    RunEvent::ToolCallRequested {
        call_id,
        tool: tool.to_string(),
        args: json!({}),
        action: Some(action()),
        decision: Some(Decision::Ask),
        reasons: Vec::new(),
    }
}

fn action() -> Action {
    Action {
        verb: "write".to_string(),
        target: Target::File {
            path: "a.md".to_string(),
        },
    }
}

/// The log a process leaves when it dies mid-task: the root is running a delegate call, and
/// the child it started is waiting on an approval card.
#[sqlx::test]
async fn a_crashed_task_is_interrupted_child_first_and_recovery_runs_once(pool: SqlitePool) {
    let app = app(pool).await;
    let store = SqliteStore::new(app.pool.clone());
    let root = RunId::new();
    let child = RunId::new();
    let at_root = RunRef {
        root_run_id: root,
        run_id: root,
    };
    let at_child = RunRef {
        root_run_id: root,
        run_id: child,
    };
    let (delegate, write) = (ToolCallId::new(), ToolCallId::new());
    let approval = ApprovalId::new();
    for (at, event) in [
        (
            at_root,
            RunEvent::RunStarted {
                agent_id: app.worker,
                parent_run_id: None,
                parent_call_id: None,
                task: "t".to_string(),
                task_sources: Vec::new(),
                title: None,
                budget: None,
            },
        ),
        (at_root, requested(delegate, "delegate")),
        (at_root, RunEvent::ToolCallStarted { call_id: delegate }),
        (
            at_child,
            RunEvent::RunStarted {
                agent_id: app.worker,
                parent_run_id: Some(root),
                parent_call_id: Some(delegate),
                task: "step".to_string(),
                task_sources: Vec::new(),
                title: None,
                budget: None,
            },
        ),
        (at_child, requested(write, "file_write")),
        (
            at_child,
            RunEvent::ApprovalRequested {
                approval_id: approval,
                call_id: write,
                action: action(),
                custody: Custody {
                    agent_path: vec![Slug::new("worker").expect("slug")],
                    parent_task: "t".to_string(),
                    parent_task_sources: Vec::new(),
                    rules: Vec::new(),
                    taint_sources: Vec::new(),
                },
                preview: "a.md".to_string(),
                expires_at: Utc::now(),
            },
        ),
    ] {
        store.emit(at, event).await.expect("emit");
    }

    let mut rx = store.subscribe();
    assert_eq!(
        interrupt_unfinished(&app.pool, &store)
            .await
            .expect("recover"),
        2
    );
    let mut closed = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        closed.push((envelope.run_id, envelope.event));
    }
    let interrupted_call = |call_id| RunEvent::ToolCallFinished {
        call_id,
        status: CallStatus::Interrupted,
        result_preview: None,
        duration_ms: 0,
        child_run_id: None,
        tool: None,
        args: None,
    };
    let finished = RunEvent::RunFinished {
        outcome: RunOutcome::Interrupted,
    };
    assert_eq!(
        closed,
        [
            (
                child,
                RunEvent::ApprovalResolved {
                    approval_id: approval,
                    outcome: ApprovalOutcome::Expired,
                    note: None,
                }
            ),
            (child, interrupted_call(write)),
            (child, finished.clone()),
            (root, interrupted_call(delegate)),
            (root, finished),
        ]
    );

    let statuses =
        sqlx::query_scalar!(r#"SELECT status AS "status!" FROM runs ORDER BY started_at"#)
            .fetch_all(&app.pool)
            .await
            .expect("runs");
    assert_eq!(statuses, ["interrupted", "interrupted"]);
    let card = sqlx::query_scalar!("SELECT status FROM approvals")
        .fetch_one(&app.pool)
        .await
        .expect("approval");
    assert_eq!(card, "expired");
    assert_eq!(
        interrupt_unfinished(&app.pool, &store)
            .await
            .expect("again"),
        0,
        "a second boot finds nothing open"
    );
}
