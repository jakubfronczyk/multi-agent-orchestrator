//! Taint, custody, untrusted results and the repeat check (§6 rules 4, 5, 7, 8).

use std::sync::Arc;
use std::time::Duration;

use orchestrator_core::engine::StartRequest;
use orchestrator_core::llm::Item;
use orchestrator_core::policy::{Rule, TaintSource};
use orchestrator_core::runs::{Answer, CallStatus, RunEvent};
use orchestrator_server::events::EventEnvelope;
use sqlx::SqlitePool;

use crate::recording::Recording;
use crate::tools::UNREPORTED_PATH;
use crate::{
    App, Config, agent_id, answer_approvals, app, app_with, app_with_llm, cards, request, tainted,
    task_events, test_config,
};

async fn run_as(app: &App, slug: &str, scenario: &str) -> Vec<EventEnvelope> {
    let mut rx = app.store.subscribe();
    let start = StartRequest {
        agent_id: Some(agent_id(&app.pool, slug).await),
        ..request(app.team, scenario)
    };
    let root = app.engine.start(start).await.expect("start");
    task_events(&mut rx, root).await
}

fn statuses(events: &[EventEnvelope]) -> Vec<CallStatus> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            RunEvent::ToolCallFinished { status, .. } => Some(*status),
            _ => None,
        })
        .collect()
}

fn inbox(id: &str) -> TaintSource {
    TaintSource::Inbox {
        message_id: id.to_string(),
    }
}

#[sqlx::test]
async fn a_failed_fetch_taints_before_its_result_is_recorded(pool: SqlitePool) {
    let app = app(pool).await;
    let url = "https://example.com/fail";
    let events = run_as(
        &app,
        "specialist",
        &format!(r#"test:call:http_fetch:{{"url":"{url}"}}"#),
    )
    .await;

    assert_eq!(
        tainted(&events),
        [TaintSource::Web {
            url: url.to_string()
        }]
    );
    assert_eq!(statuses(&events), [CallStatus::Failed]);
    let tainted_at = events
        .iter()
        .position(|e| matches!(e.event, RunEvent::TaskTainted { .. }));
    let finished_at = events
        .iter()
        .position(|e| matches!(e.event, RunEvent::ToolCallFinished { .. }));
    assert!(
        tainted_at < finished_at,
        "taint is committed before the result"
    );
}

/// An outside-reading tool taints from its target, so a tool that forgets to report its
/// source still taints the task.
#[sqlx::test]
async fn a_fetch_taints_even_when_the_tool_reports_no_source(pool: SqlitePool) {
    let app = app(pool).await;
    let url = format!("https://example.com{UNREPORTED_PATH}");
    let events = run_as(
        &app,
        "specialist",
        &format!(r#"test:call:http_fetch:{{"url":"{url}"}}"#),
    )
    .await;

    assert_eq!(statuses(&events), [CallStatus::Ok]);
    assert_eq!(tainted(&events), [TaintSource::Web { url }]);
}

#[sqlx::test]
async fn a_denied_fetch_never_taints(pool: SqlitePool) {
    let app = app(pool).await;
    answer_approvals(&app.store, &app.engine, |_| Answer::Deny { note: None });
    let events = run_as(
        &app,
        "specialist",
        r#"test:call:http_fetch:{"url":"https://evil.test/x"}"#,
    )
    .await;

    assert_eq!(statuses(&events), [CallStatus::DeniedHuman]);
    assert!(tainted(&events).is_empty());
}

#[sqlx::test]
async fn inbox_list_taints_once_per_listed_message_in_order(pool: SqlitePool) {
    let app = app(pool).await;
    let events = run_as(&app, "specialist", "test:call:inbox_list:{}").await;
    assert_eq!(tainted(&events), [inbox("m1"), inbox("m2")]);
}

#[sqlx::test]
async fn parallel_reads_of_one_source_taint_once(pool: SqlitePool) {
    let app = app(pool).await;
    let events = run_as(&app, "specialist", r#"test:twice:inbox_read:{"id":"m1"}"#).await;

    assert_eq!(statuses(&events), [CallStatus::Ok, CallStatus::Ok]);
    assert_eq!(tainted(&events), [inbox("m1")]);
}

#[sqlx::test]
async fn once_tainted_an_allowed_write_asks_and_the_card_shows_the_source(pool: SqlitePool) {
    let app = app(pool).await;
    answer_approvals(&app.store, &app.engine, |_| Answer::Approve { note: None });
    let events = run_as(
        &app,
        "writer",
        r#"test:seq:inbox_read:{"id":"m1"}|file_write:{"path":"a.md"}"#,
    )
    .await;

    let cards = cards(&events);
    assert_eq!(cards.len(), 1);
    let RunEvent::ApprovalRequested { custody, .. } = cards[0] else {
        unreachable!()
    };
    assert_eq!(custody.taint_sources, [inbox("m1")]);
    assert_eq!(
        custody.rules,
        [Rule::Taint {
            source: inbox("m1")
        }]
    );
    assert_eq!(statuses(&events), [CallStatus::Ok, CallStatus::Ok]);
}

#[sqlx::test]
async fn notes_written_after_taint_are_stored_untrusted(pool: SqlitePool) {
    let app = app(pool).await;
    let events = run_as(
        &app,
        "writer",
        r#"test:seq:memory_write:{}|inbox_read:{"id":"m1"}|memory_write:{}"#,
    )
    .await;

    let previews: Vec<String> = events
        .iter()
        .filter_map(|e| match &e.event {
            RunEvent::ToolCallFinished { result_preview, .. } => result_preview.clone(),
            _ => None,
        })
        .collect();
    assert!(previews[0].contains("\"trusted\""), "{}", previews[0]);
    assert!(previews[2].contains("\"untrusted\""), "{}", previews[2]);
}

#[sqlx::test]
async fn an_expired_card_asks_again_on_retry(pool: SqlitePool) {
    let app = app_with(
        pool,
        Config {
            approval_timeout: Duration::from_millis(5),
            ..test_config()
        },
    )
    .await;
    let fetch = r#"http_fetch:{"url":"https://evil.test/x"}"#;
    let events = run_as(&app, "specialist", &format!("test:seq:{fetch}|{fetch}")).await;

    assert_eq!(cards(&events).len(), 2, "an expiry is not a refusal");
    assert_eq!(
        statuses(&events),
        [CallStatus::Expired, CallStatus::Expired]
    );
}

#[sqlx::test]
async fn a_tainting_result_reaches_the_model_marked_and_wrapped(pool: SqlitePool) {
    let recording = Arc::new(Recording::new());
    let app = app_with_llm(pool, test_config(), recording.clone()).await;
    run_as(&app, "specialist", r#"test:call:inbox_read:{"id":"m1"}"#).await;

    let output = tool_output_seen_at_step(&recording, "specialist", 1);
    let body: serde_json::Value = serde_json::from_str(&output).expect("valid json");
    assert_eq!(body["trust"], "untrusted");
    assert_eq!(body["source"], "inbox:m1");
    let result = body["result"].as_str().expect("wrapped text");
    assert!(
        result.starts_with("<untrusted_data source=\"inbox:m1\">"),
        "{result}"
    );
}

#[sqlx::test]
async fn an_oversized_result_is_cut_and_stays_valid_json(pool: SqlitePool) {
    let recording = Arc::new(Recording::new());
    let app = app_with_llm(pool, test_config(), recording.clone()).await;
    let big = "é".repeat(70_000);
    run_as(
        &app,
        "specialist",
        &format!(r#"test:call:memory_read:{{"q":"{big}"}}"#),
    )
    .await;

    let output = tool_output_seen_at_step(&recording, "specialist", 1);
    let body: serde_json::Value = serde_json::from_str(&output).expect("valid json");
    assert_eq!(body["truncated"], true);
    assert!(output.chars().count() < 70_000);
}

fn tool_output_seen_at_step(recording: &Recording, agent: &str, step: u32) -> String {
    recording
        .requests()
        .into_iter()
        .find(|r| r.script.agent.as_str() == agent && r.script.step == step)
        .and_then(|r| {
            r.transcript.into_iter().find_map(|item| match item {
                Item::ToolOutput { output, .. } => Some(output),
                _ => None,
            })
        })
        .expect("a tool output in the transcript")
}

/// Each call says whether its own result brought outside content in, a repeat read included;
/// a call that did not read outside content, or failed to, is not marked.
#[sqlx::test]
async fn each_call_records_whether_it_brought_outside_content(pool: SqlitePool) {
    let app = app(pool).await;
    let scenario =
        r#"test:seq:inbox_read:{"id":"m1"}|file_read:{"path":"a.md"}|inbox_read:{"id":"m1"}"#;
    let events = run_as(&app, "specialist", scenario).await;
    let flags: Vec<bool> = events
        .iter()
        .filter_map(|e| match &e.event {
            RunEvent::ToolCallFinished { tainted, .. } => Some(*tainted),
            _ => None,
        })
        .collect();
    assert_eq!(flags, [true, false, true]);
    let stored = sqlx::query_scalar!("SELECT tainted FROM tool_calls ORDER BY requested_at, id")
        .fetch_all(&app.pool)
        .await
        .expect("calls");
    assert_eq!(stored, [1, 0, 1]);
}

/// A run that sends twice to one person leaves two outbox rows, each naming its own call.
#[sqlx::test]
async fn each_sent_message_names_its_call(pool: SqlitePool) {
    use orchestrator_core::tools::Tool;
    use orchestrator_server::tools::messages::SendMessage;

    let app = app(pool.clone()).await;
    let send = SendMessage::new(pool);
    let args = serde_json::json!({"channel": "email", "recipient": "a@example.com", "subject": "s", "body": "b"});
    let mut calls = Vec::new();
    for _ in 0..2 {
        let ctx = crate::tool_ctx(&app, "messenger").await;
        send.run(&ctx, args.clone()).await.expect("sent");
        calls.push(ctx.call_id);
    }
    let sent = orchestrator_server::sqlite::outbox::list(&app.pool, Default::default())
        .await
        .expect("outbox")
        .messages;
    let mut named: Vec<_> = sent.iter().map(|m| m.tool_call_id).collect();
    named.sort_by_key(|id| serde_json::to_string(id).expect("id"));
    calls.sort_by_key(|id| serde_json::to_string(id).expect("id"));
    assert_eq!(named, calls);
}
