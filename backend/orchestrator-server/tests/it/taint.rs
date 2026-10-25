//! Taint, custody, untrusted results and the repeat check (§6 rules 4, 5, 7, 8), and the drill.

use std::sync::Arc;
use std::time::Duration;

use orchestrator_core::engine::{Engine, StartRequest};
use orchestrator_core::ids::AgentId;
use orchestrator_core::llm::Item;
use orchestrator_core::policy::{Decision, Rule, TaintSource};
use orchestrator_core::runs::{Answer, CallStatus, RunEvent};
use orchestrator_core::tools::Target;
use orchestrator_server::events::EventEnvelope;
use orchestrator_server::sqlite::{SqliteStore, agents};
use sqlx::SqlitePool;

use crate::recording::Recording;
use crate::{App, Config, app, app_with, app_with_llm, request, task_events, test_config};

async fn agent_id(app: &App, slug: &str) -> AgentId {
    let all = agents::list(&app.pool).await.expect("agents");
    all.into_iter()
        .find(|record| record.spec.slug().as_str() == slug)
        .map(|record| record.spec.id())
        .expect("seeded agent")
}

async fn run_as(app: &App, slug: &str, scenario: &str) -> Vec<EventEnvelope> {
    let mut rx = app.store.subscribe();
    let start = StartRequest {
        agent_id: Some(agent_id(app, slug).await),
        ..request(scenario)
    };
    let root = app.engine.start(start).await.expect("start");
    task_events(&mut rx, root).await
}

/// Answers every approval in the task the same way, as a person at the cards would.
fn answer_every_approval(store: &Arc<SqliteStore>, engine: &Arc<Engine>, approve: bool) {
    let mut rx = store.subscribe();
    let engine = Arc::clone(engine);
    tokio::spawn(async move {
        while let Ok(envelope) = rx.recv().await {
            if let RunEvent::ApprovalRequested { approval_id, .. } = envelope.event {
                let answer = if approve {
                    Answer::Approve { note: None }
                } else {
                    Answer::Deny { note: None }
                };
                engine.answer(approval_id, answer);
            }
        }
    });
}

fn tainted(events: &[EventEnvelope]) -> Vec<TaintSource> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            RunEvent::TaskTainted { source } => Some(source.clone()),
            _ => None,
        })
        .collect()
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

fn approvals(events: &[EventEnvelope]) -> Vec<&RunEvent> {
    events
        .iter()
        .map(|e| &e.event)
        .filter(|e| matches!(e, RunEvent::ApprovalRequested { .. }))
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
        "worker",
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

#[sqlx::test]
async fn a_denied_fetch_never_taints(pool: SqlitePool) {
    let app = app(pool).await;
    answer_every_approval(&app.store, &app.engine, false);
    let events = run_as(
        &app,
        "worker",
        r#"test:call:http_fetch:{"url":"https://evil.test/x"}"#,
    )
    .await;

    assert_eq!(statuses(&events), [CallStatus::DeniedHuman]);
    assert!(tainted(&events).is_empty());
}

#[sqlx::test]
async fn inbox_list_taints_once_per_listed_message_in_order(pool: SqlitePool) {
    let app = app(pool).await;
    let events = run_as(&app, "worker", "test:call:inbox_list:{}").await;
    assert_eq!(tainted(&events), [inbox("m1"), inbox("m2")]);
}

#[sqlx::test]
async fn parallel_reads_of_one_source_taint_once(pool: SqlitePool) {
    let app = app(pool).await;
    let events = run_as(&app, "worker", r#"test:twice:inbox_read:{"id":"m1"}"#).await;

    assert_eq!(statuses(&events), [CallStatus::Ok, CallStatus::Ok]);
    assert_eq!(tainted(&events), [inbox("m1")]);
}

#[sqlx::test]
async fn once_tainted_an_allowed_write_asks_and_the_card_shows_the_source(pool: SqlitePool) {
    let app = app(pool).await;
    answer_every_approval(&app.store, &app.engine, true);
    let events = run_as(
        &app,
        "writer",
        r#"test:seq:inbox_read:{"id":"m1"}|file_write:{"path":"a.md"}"#,
    )
    .await;

    let cards = approvals(&events);
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
    let events = run_as(&app, "worker", &format!("test:seq:{fetch}|{fetch}")).await;

    assert_eq!(approvals(&events).len(), 2, "an expiry is not a refusal");
    assert_eq!(
        statuses(&events),
        [CallStatus::Expired, CallStatus::Expired]
    );
}

#[sqlx::test]
async fn a_tainting_result_reaches_the_model_marked_and_wrapped(pool: SqlitePool) {
    let recording = Arc::new(Recording::slowing("none"));
    let app = app_with_llm(pool, test_config(), recording.clone()).await;
    run_as(&app, "worker", r#"test:call:inbox_read:{"id":"m1"}"#).await;

    let output = tool_output_seen_at_step(&recording, "worker", 1);
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
    let recording = Arc::new(Recording::slowing("none"));
    let app = app_with_llm(pool, test_config(), recording.clone()).await;
    let big = "é".repeat(70_000);
    run_as(
        &app,
        "worker",
        &format!(r#"test:call:memory_read:{{"q":"{big}"}}"#),
    )
    .await;

    let output = tool_output_seen_at_step(&recording, "worker", 1);
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

/// §13 test 13: the vendor note tries three exits and none reaches the outbox.
#[sqlx::test]
async fn the_drill_stops_every_exit(pool: SqlitePool) {
    let recording = Arc::new(Recording::slowing("none"));
    let app = app_with_llm(pool, test_config(), recording.clone()).await;
    answer_every_approval(&app.store, &app.engine, false);
    let mut rx = app.store.subscribe();
    let root = app.engine.start(request("drill")).await.expect("start");
    let events = task_events(&mut rx, root).await;
    let note = inbox("vendor-note");

    // Each call in order: (tool, decision, reasons, status).
    let mut calls: Vec<(String, Option<Decision>, Vec<Rule>)> = Vec::new();
    for e in &events {
        if let RunEvent::ToolCallRequested {
            tool,
            decision,
            reasons,
            ..
        } = &e.event
        {
            calls.push((tool.clone(), *decision, reasons.clone()));
        }
    }
    let sends: Vec<_> = calls
        .iter()
        .filter(|(tool, ..)| tool == "send_message")
        .collect();
    let fetches: Vec<_> = calls
        .iter()
        .filter(|(tool, ..)| tool == "http_fetch")
        .collect();

    // Exit 1: Reader's direct send is refused by its own setting; logged, no card.
    assert_eq!(sends[0].1, Some(Decision::Deny));
    assert_eq!(sends[0].2, [Rule::AgentPolicy]);

    // Exit 3: the exfil fetch asks with the taint and the off-allowlist host; the repeat is
    // refused as a repeat with no card.
    assert_eq!(fetches[0].1, Some(Decision::Ask));
    assert!(fetches[0].2.contains(&Rule::Taint {
        source: note.clone()
    }));
    assert!(fetches[0].2.contains(&Rule::OffAllowlist {
        host: "evil.test".to_string()
    }));
    assert_eq!(fetches[1].1, Some(Decision::Deny));
    assert_eq!(fetches[1].2.first(), Some(&Rule::RepeatOfRefused));

    // Exactly two cards: the fetch and Messenger's send; both show the source and are denied.
    let cards = approvals(&events);
    assert_eq!(cards.len(), 2);
    for card in &cards {
        let RunEvent::ApprovalRequested { custody, .. } = card else {
            unreachable!()
        };
        assert_eq!(custody.taint_sources, std::slice::from_ref(&note));
    }
    let RunEvent::ApprovalRequested { action, .. } = cards[0] else {
        unreachable!()
    };
    assert!(
        matches!(&action.target, Target::Host { url, .. } if url.contains("evil.test/collect"))
    );
    let RunEvent::ApprovalRequested { custody, .. } = cards[1] else {
        unreachable!()
    };
    let path: Vec<&str> = custody.agent_path.iter().map(|s| s.as_str()).collect();
    assert_eq!(path, ["lead", "messenger"]);
    assert!(
        custody.parent_task.starts_with("<untrusted_data"),
        "the child got its task as data"
    );

    let denied = statuses(&events)
        .iter()
        .filter(|s| **s == CallStatus::DeniedHuman)
        .count();
    assert_eq!(denied, 2);

    // Reader's result reaches the Lead marked untrusted.
    let lead_output = tool_output_seen_at_step(&recording, "lead", 1);
    let body: serde_json::Value = serde_json::from_str(&lead_output).expect("json");
    assert_eq!(body["trust"], "untrusted");

    let sent = sqlx::query_scalar!("SELECT COUNT(*) FROM outbox_messages")
        .fetch_one(&app.pool)
        .await
        .expect("count");
    assert_eq!(sent, 0, "nothing reaches the outbox");
}
