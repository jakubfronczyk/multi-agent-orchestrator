//! Every way a call can end, the approval round trip, and the step limit.

use std::sync::Arc;
use std::time::Duration;

use orchestrator_core::engine::{Engine, StartRequest};
use orchestrator_core::ids::ApprovalId;
use orchestrator_core::policy::{Decision, Rule, TaintSource};
use orchestrator_core::runs::{Answer, ApprovalOutcome, CallStatus, RunEvent, RunOutcome};
use orchestrator_core::tools::{MemoryScope, NoteKey};
use orchestrator_server::sqlite::SqliteStore;
use sqlx::SqlitePool;

use crate::recording::Recording;
use crate::tools::{LEAKED, PANIC_IN_CHECK, PANIC_IN_PREVIEW};
use crate::{
    App, Config, app, app_with, app_with_llm, events_until_finished, request, test_config, wait_for,
};

fn as_worker(app: &App, scenario: &str) -> StartRequest {
    StartRequest {
        agent_id: Some(app.worker),
        ..request(scenario)
    }
}

async fn run_to_end(app: &App, scenario: &str) -> Vec<RunEvent> {
    let mut rx = app.store.subscribe();
    let run = app
        .engine
        .start(as_worker(app, scenario))
        .await
        .expect("start");
    events_until_finished(&mut rx, run).await
}

/// Answers the first approval it sees, as a person clicking the card would.
fn answer_first_approval(store: &Arc<SqliteStore>, engine: &Arc<Engine>, answer: Answer) {
    let mut rx = store.subscribe();
    let engine = Arc::clone(engine);
    tokio::spawn(async move {
        while let Ok(envelope) = rx.recv().await {
            if let RunEvent::ApprovalRequested { approval_id, .. } = envelope.event {
                engine.answer(approval_id, answer);
                return;
            }
        }
    });
}

/// The event kinds that belong to calls, in order.
fn call_kinds(events: &[RunEvent]) -> Vec<&'static str> {
    events
        .iter()
        .filter_map(|event| match event {
            RunEvent::ToolCallRequested { .. } => Some("requested"),
            RunEvent::ApprovalRequested { .. } => Some("approval_requested"),
            RunEvent::ApprovalResolved { .. } => Some("approval_resolved"),
            RunEvent::ToolCallStarted { .. } => Some("started"),
            RunEvent::ToolCallFinished { .. } => Some("finished"),
            _ => None,
        })
        .collect()
}

fn finished_status(events: &[RunEvent]) -> CallStatus {
    events
        .iter()
        .find_map(|event| match event {
            RunEvent::ToolCallFinished { status, .. } => Some(*status),
            _ => None,
        })
        .expect("a finished call")
}

fn completed(events: &[RunEvent]) -> bool {
    matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            outcome: RunOutcome::Completed {
                truncated: false,
                ..
            }
        })
    )
}

#[sqlx::test]
async fn an_allowed_tool_runs_and_is_logged(pool: SqlitePool) {
    let app = app(pool).await;
    let events = run_to_end(&app, r#"test:call:memory_read:{"q":1}"#).await;

    assert_eq!(call_kinds(&events), ["requested", "started", "finished"]);
    assert!(events.iter().any(|e| matches!(
        e,
        RunEvent::ToolCallRequested {
            decision: Some(Decision::Allow),
            ..
        }
    )));
    assert_eq!(finished_status(&events), CallStatus::Ok);
    assert!(completed(&events));

    let row = sqlx::query!("SELECT phase, status FROM tool_calls")
        .fetch_one(&app.pool)
        .await
        .expect("row");
    assert_eq!(
        (row.phase.as_str(), row.status.as_deref()),
        ("finished", Some("ok"))
    );
}

#[sqlx::test]
async fn a_denied_tool_is_refused_and_the_run_continues(pool: SqlitePool) {
    let app = app(pool).await;
    let events = run_to_end(&app, r#"test:call:send_message:{"to":"a"}"#).await;

    assert_eq!(call_kinds(&events), ["requested", "finished"]);
    assert_eq!(finished_status(&events), CallStatus::Refused);
    assert!(completed(&events), "the run continues after a refusal");
}

#[sqlx::test]
async fn ask_waits_and_runs_after_approval(pool: SqlitePool) {
    let app = app(pool).await;
    answer_first_approval(&app.store, &app.engine, Answer::Approve { note: None });
    let events = run_to_end(&app, r#"test:call:file_write:{"path":"a.md"}"#).await;

    assert_eq!(
        call_kinds(&events),
        [
            "requested",
            "approval_requested",
            "approval_resolved",
            "started",
            "finished"
        ]
    );
    assert!(events.iter().any(|e| matches!(
        e,
        RunEvent::ApprovalResolved {
            outcome: ApprovalOutcome::Approved,
            ..
        }
    )));
    assert_eq!(finished_status(&events), CallStatus::Ok);
    assert!(completed(&events));
}

/// A call that reaches the gateway after the task is cancelled is logged and ends cancelled
/// without starting: the fifth call waits for a slot behind the card until the cancel.
#[sqlx::test]
async fn a_call_after_the_cancel_never_starts(pool: SqlitePool) {
    let app = app(pool).await;
    let (mut rx, mut asked) = (app.store.subscribe(), app.store.subscribe());
    let read = r#"memory_read:{"q":1}"#;
    let scenario =
        format!(r#"test:batch:file_write:{{"path":"a.md"}}|{read}|{read}|{read}|{read}"#);
    let run = app
        .engine
        .start(as_worker(&app, &scenario))
        .await
        .expect("start");
    wait_for(&mut asked, |e| {
        matches!(e.event, RunEvent::ApprovalRequested { .. }).then_some(())
    })
    .await;
    assert!(app.engine.cancel(run));
    let events = events_until_finished(&mut rx, run).await;

    let fifth = events
        .iter()
        .filter_map(|e| match e {
            RunEvent::ToolCallRequested { call_id, .. } => Some(*call_id),
            _ => None,
        })
        .nth(4)
        .expect("the fifth call is logged");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, RunEvent::ToolCallStarted { call_id } if *call_id == fifth))
    );
    assert!(events.iter().any(|e| matches!(
        e,
        RunEvent::ToolCallFinished { call_id, status: CallStatus::Cancelled, .. } if *call_id == fifth
    )));
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            outcome: RunOutcome::Cancelled
        })
    ));
}

/// Tool code on the run's own task cannot end the run: a panicking preview falls back to the
/// arguments, and a panicking check refuses the call.
#[sqlx::test]
async fn a_panic_in_preview_or_check_ends_only_that_call(pool: SqlitePool) {
    let app = app(pool).await;
    answer_first_approval(&app.store, &app.engine, Answer::Approve { note: None });
    let scenario = format!(r#"test:call:file_write:{{"path":"{PANIC_IN_PREVIEW}"}}"#);
    let events = run_to_end(&app, &scenario).await;
    let preview = events.iter().find_map(|e| match e {
        RunEvent::ApprovalRequested { preview, .. } => Some(preview.clone()),
        _ => None,
    });
    assert!(preview.is_some_and(|p| p.contains(PANIC_IN_PREVIEW)));
    assert_eq!(finished_status(&events), CallStatus::Ok);
    assert!(completed(&events));

    let scenario = format!(r#"test:call:file_write:{{"path":"{PANIC_IN_CHECK}"}}"#);
    let events = run_to_end(&app, &scenario).await;
    assert_eq!(finished_status(&events), CallStatus::InvalidArgs);
    assert!(completed(&events));
}

#[sqlx::test]
async fn ask_denied_returns_denied_human_with_the_note(pool: SqlitePool) {
    let app = app(pool).await;
    let note = Some("not this file".to_string());
    answer_first_approval(&app.store, &app.engine, Answer::Deny { note: note.clone() });
    let events = run_to_end(&app, r#"test:call:file_write:{"path":"a.md"}"#).await;

    assert_eq!(
        call_kinds(&events),
        [
            "requested",
            "approval_requested",
            "approval_resolved",
            "finished"
        ]
    );
    assert!(events.iter().any(|e| matches!(
        e,
        RunEvent::ApprovalResolved { outcome: ApprovalOutcome::Denied, note: n, .. } if *n == note
    )));
    assert_eq!(finished_status(&events), CallStatus::DeniedHuman);
    assert!(completed(&events));
}

#[sqlx::test]
async fn ask_with_no_answer_expires(pool: SqlitePool) {
    let config = Config {
        approval_timeout: Duration::from_millis(10),
        ..test_config()
    };
    let app = app_with(pool, config).await;
    let events = run_to_end(&app, r#"test:call:file_write:{"path":"a.md"}"#).await;

    assert!(events.iter().any(|e| matches!(
        e,
        RunEvent::ApprovalResolved {
            outcome: ApprovalOutcome::Expired,
            ..
        }
    )));
    assert_eq!(finished_status(&events), CallStatus::Expired);
}

#[sqlx::test]
async fn a_tool_timeout_is_a_result_not_a_crash(pool: SqlitePool) {
    let app = app(pool).await;
    let events = run_to_end(&app, "test:call:csv_query:{}").await;

    assert_eq!(finished_status(&events), CallStatus::Timeout);
    assert!(completed(&events));
}

#[sqlx::test]
async fn invalid_args_are_refused_before_running(pool: SqlitePool) {
    let app = app(pool).await;
    let events = run_to_end(&app, "test:call:memory_read:[1]").await;

    assert_eq!(call_kinds(&events), ["requested", "finished"]);
    assert!(events.iter().any(|e| matches!(
        e,
        RunEvent::ToolCallRequested { action: None, reasons, .. } if *reasons == [Rule::InvalidArgs]
    )));
    assert_eq!(finished_status(&events), CallStatus::InvalidArgs);
}

#[sqlx::test]
async fn panics_failures_and_unoffered_tools_each_end_one_call(pool: SqlitePool) {
    let app = app(pool).await;
    for (scenario, status) in [
        ("test:call:file_list:{}", CallStatus::Internal),
        ("test:call:file_read:{}", CallStatus::Failed),
        ("test:call:memory_search:{}", CallStatus::NotOffered),
    ] {
        let events = run_to_end(&app, scenario).await;
        assert_eq!(finished_status(&events), status, "{scenario}");
        assert!(completed(&events), "{scenario}");
    }
}

#[sqlx::test]
async fn the_step_limit_ends_the_run_truncated(pool: SqlitePool) {
    let app = app(pool).await;
    let events = run_to_end(&app, "test:loop:memory_read").await;

    let model_calls = events
        .iter()
        .filter(|e| matches!(e, RunEvent::LlmRequested { .. }))
        .count();
    assert_eq!(model_calls, 9, "8 steps with tools, then one without");
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            outcome: RunOutcome::Completed {
                truncated: true,
                ..
            }
        })
    ));
}

#[sqlx::test]
async fn an_answer_racing_expiry_resolves_the_approval_once(pool: SqlitePool) {
    let config = Config {
        approval_timeout: Duration::from_millis(1),
        ..test_config()
    };
    let app = app_with(pool, config).await;
    for _ in 0..10 {
        answer_first_approval(&app.store, &app.engine, Answer::Approve { note: None });
        let events = run_to_end(&app, r#"test:call:file_write:{"path":"a.md"}"#).await;

        let outcomes: Vec<ApprovalOutcome> = events
            .iter()
            .filter_map(|e| match e {
                RunEvent::ApprovalResolved { outcome, .. } => Some(*outcome),
                _ => None,
            })
            .collect();
        assert_eq!(outcomes.len(), 1);
        let expected = match outcomes[0] {
            ApprovalOutcome::Approved => CallStatus::Ok,
            _ => CallStatus::Expired,
        };
        assert_eq!(finished_status(&events), expected);
    }
}

#[sqlx::test]
async fn a_second_answer_over_http_is_a_conflict(pool: SqlitePool) {
    let app = app(pool).await;
    let mut rx = app.store.subscribe();
    let base = crate::serve(&app).await;

    app.engine
        .start(as_worker(&app, r#"test:call:file_write:{"path":"a.md"}"#))
        .await
        .expect("start");
    let approval: ApprovalId = wait_for(&mut rx, |envelope| match envelope.event {
        RunEvent::ApprovalRequested { approval_id, .. } => Some(approval_id),
        _ => None,
    })
    .await;

    let client = reqwest::Client::new();
    let listed: serde_json::Value = client
        .get(format!("{base}/api/approvals?status=pending"))
        .send()
        .await
        .expect("list")
        .json()
        .await
        .expect("json");
    assert_eq!(listed["approvals"].as_array().map(Vec::len), Some(1));

    let url = format!(
        "{base}/api/approvals/{}",
        serde_json::to_value(approval)
            .expect("id")
            .as_str()
            .expect("str")
    );
    let body = serde_json::json!({"decision": "approve"});
    assert_eq!(
        client
            .post(&url)
            .json(&body)
            .send()
            .await
            .expect("first")
            .status(),
        204
    );
    assert_eq!(
        client
            .post(&url)
            .json(&body)
            .send()
            .await
            .expect("second")
            .status(),
        409
    );
}

/// A tool that hands the Lead outside content fails as internal and taints the task; the content
/// reaches neither the Lead's model nor the log.
#[sqlx::test]
async fn a_tainted_result_never_reaches_the_lead(pool: SqlitePool) {
    let recording = Arc::new(Recording::new());
    let app = app_with_llm(pool, test_config(), recording.clone()).await;
    let mut rx = app.store.subscribe();
    let run = app
        .engine
        .start(request(r#"test:call:memory_search:{"query":"payroll"}"#))
        .await
        .expect("start");
    let events = events_until_finished(&mut rx, run).await;

    assert_eq!(finished_status(&events), CallStatus::Internal);
    let tainted: Vec<&TaintSource> = events
        .iter()
        .filter_map(|e| match e {
            RunEvent::TaskTainted { source } => Some(source),
            _ => None,
        })
        .collect();
    let vendor = TaintSource::Note {
        scope: MemoryScope::Shared,
        key: NoteKey::new("vendor").expect("key"),
    };
    assert_eq!(tainted, [&vendor], "the withheld result still taints");
    assert!(!format!("{events:?}").contains(LEAKED));
    let lead_requests: Vec<_> = recording
        .requests()
        .into_iter()
        .filter(|r| r.script.agent.as_str() == "lead")
        .collect();
    assert_eq!(lead_requests.len(), 2, "the call, then the answer after it");
    assert!(
        lead_requests
            .iter()
            .all(|r| !format!("{r:?}").contains(LEAKED))
    );
}
