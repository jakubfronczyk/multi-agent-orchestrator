//! Follow-ups: a finished task takes the Lead's next turn, rebuilt from the event log, so taint
//! and refused calls carry over, even into an engine that never ran the task.

use std::sync::Arc;

use orchestrator_core::engine::{Engine, FollowUpRequest, StartRequest};
use orchestrator_core::ids::RunId;
use orchestrator_core::llm::Item;
use orchestrator_core::policy::{Decision, Rule, TaintSource};
use orchestrator_core::runs::{RunEvent, RunOutcome};
use orchestrator_core::tools::Target;
use orchestrator_server::sqlite::runs;
use serde_json::json;
use sqlx::SqlitePool;

use crate::recording::Recording;
use crate::{
    Seeded, answer_approvals, app_with_llm, deny_rolf_naming_anna, events_until_finished, request,
    serve, task_events, test_config, turn_events,
};

const S1_TASK: &str =
    "Check access request AR-1042 against our policy and email the decision to the owner";
const FOLLOW_UP: &str = "Email Rolf the decision again, then Anna instead";

async fn run_s1(seeded: &Seeded, engine: &Engine) -> RunId {
    let mut rx = seeded.store.subscribe();
    let request = StartRequest {
        task: S1_TASK.to_string(),
        ..request(seeded.team, "s1")
    };
    let root = engine.start(request).await.expect("start");
    task_events(&mut rx, root).await;
    root
}

async fn follow_up_s1(seeded: &Seeded, engine: &Engine, root: RunId) -> (RunId, Vec<RunEvent>) {
    let mut rx = seeded.store.subscribe();
    let request = FollowUpRequest {
        text: FOLLOW_UP.to_string(),
        scenario: Some("s1-followup".to_string()),
        draft_only: false,
    };
    let run = engine.follow_up(root, request).await.expect("follow-up");
    (run, turn_events(&mut rx, root, run).await)
}

/// The repeated send to Rolf is refused without a card; the send to Anna asks, and its card
/// names the inbox source S1 read, so the taint carried over.
fn assert_s1_carried_over(events: &[RunEvent]) {
    let request = TaintSource::Inbox {
        message_id: "access-request-1042".to_string(),
    };
    let sends: Vec<(Option<Decision>, Vec<Rule>)> = events
        .iter()
        .filter_map(|e| match e {
            RunEvent::ToolCallRequested {
                tool,
                decision,
                reasons,
                ..
            } if tool == "send_message" => Some((*decision, reasons.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(sends.len(), 2);
    assert_eq!(
        (sends[0].0, sends[0].1.first()),
        (Some(Decision::Deny), Some(&Rule::RepeatOfRefused))
    );
    assert_eq!(sends[1].0, Some(Decision::Ask));

    let cards: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            RunEvent::ApprovalRequested {
                action, custody, ..
            } => Some((action, custody)),
            _ => None,
        })
        .collect();
    assert_eq!(cards.len(), 1, "only the send to Anna shows a card");
    let (action, custody) = cards[0];
    assert!(matches!(
        &action.target,
        Target::Recipient { address, .. } if address == "anna.weber@example.com"
    ));
    assert!(custody.taint_sources.contains(&request));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, RunEvent::TaskTainted { .. })),
        "the follow-up starts tainted; nothing new taints it"
    );
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            outcome: RunOutcome::Completed { .. }
        })
    ));
}

#[sqlx::test]
async fn a_follow_up_keeps_the_taint_and_refuses_the_repeated_send(pool: SqlitePool) {
    let recording = Arc::new(Recording::new());
    let seeded = Seeded::with_llm(pool, recording.clone()).await;
    let engine = seeded.engine.clone();
    answer_approvals(&seeded.store, &engine, deny_rolf_naming_anna);
    let root = run_s1(&seeded, &engine).await;

    let (run, events) = follow_up_s1(&seeded, &engine, root).await;
    assert_s1_carried_over(&events);

    // The Lead's input: S1's text, its summary wrapped as untrusted, then the follow-up.
    let first_request = recording
        .requests()
        .into_iter()
        .find(|r| r.script.run_id == run && r.script.step == 0)
        .expect("the follow-up's first request");
    let items = &first_request.transcript;
    assert_eq!(items.len(), 3);
    assert_eq!(
        items[0],
        Item::User {
            text: S1_TASK.to_string()
        }
    );
    let Item::Assistant { text } = &items[1] else {
        panic!("the earlier answer is {:?}", items[1]);
    };
    assert!(text.contains("untrusted") && text.contains("AR-1042 is approved"));
    assert_eq!(
        items[2],
        Item::User {
            text: FOLLOW_UP.to_string()
        }
    );

    let detail = runs::detail(&seeded.pool, root)
        .await
        .expect("read")
        .expect("task");
    let texts: Vec<&str> = detail.turns.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, [S1_TASK, FOLLOW_UP]);
    assert_eq!(detail.turns[1].run_id, run);
    assert!(
        detail.turns[1]
            .summary
            .as_deref()
            .is_some_and(|s| s.contains("Anna Weber"))
    );
    let list = runs::list(&seeded.pool, 10, None).await.expect("list");
    assert_eq!(list.runs.len(), 1, "a follow-up is not a new task");
}

/// A fresh engine has no memory of the task; the log alone restores taint and refusals.
#[sqlx::test]
async fn a_fresh_engine_rebuilds_the_task_from_the_log(pool: SqlitePool) {
    let recording = Arc::new(Recording::new());
    let seeded = Seeded::with_llm(pool, recording.clone()).await;
    let first = seeded.engine.clone();
    answer_approvals(&seeded.store, &first, deny_rolf_naming_anna);
    let root = run_s1(&seeded, &first).await;
    drop(first);

    let fresh = seeded.engine_with(recording.clone());
    answer_approvals(&seeded.store, &fresh, deny_rolf_naming_anna);
    let (_, events) = follow_up_s1(&seeded, &fresh, root).await;
    assert_s1_carried_over(&events);
}

async fn post(client: &reqwest::Client, url: &str, body: serde_json::Value) -> u16 {
    client
        .post(url)
        .json(&body)
        .send()
        .await
        .expect("response")
        .status()
        .as_u16()
}

/// 409 while the task runs; after a cancel the follow-up starts, and the Lead is told how the
/// earlier turn ended.
#[sqlx::test]
async fn a_running_task_is_a_conflict_and_a_cancelled_one_takes_a_follow_up(pool: SqlitePool) {
    let recording = Arc::new(Recording::new());
    let app = app_with_llm(pool, test_config(), recording.clone()).await;
    let base = serve(&app).await;
    let client = reqwest::Client::new();
    let mut rx = app.store.subscribe();
    let root = app
        .engine
        .start(request(app.team, "test:hang"))
        .await
        .expect("start");
    let messages = format!("{base}/api/runs/{root}/messages");

    let body = json!({"text": "and then?", "scenario": "test:hello"});
    assert_eq!(post(&client, &messages, body.clone()).await, 409);
    assert!(app.engine.cancel(root));
    events_until_finished(&mut rx, root).await;

    assert_eq!(post(&client, &messages, body).await, 202);
    let detail = loop {
        let detail = runs::detail(&app.pool, root)
            .await
            .expect("read")
            .expect("task");
        if detail.turns.len() == 2 && detail.turns[1].outcome.is_some() {
            break detail;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    };
    assert_eq!(detail.turns[0].outcome, Some(RunOutcome::Cancelled));
    assert_eq!(
        detail.turns[1].summary.as_deref(),
        Some("hello\n\nFrom the record: nothing was saved or sent.")
    );
    let follow_up = recording
        .requests()
        .into_iter()
        .find(|r| r.script.run_id == detail.turns[1].run_id)
        .expect("the follow-up's request");
    assert!(follow_up.transcript.contains(&Item::Assistant {
        text: "Earlier turn: cancelled by the user".to_string()
    }));
}

#[sqlx::test]
async fn concurrent_follow_ups_start_one_turn(pool: SqlitePool) {
    let app = crate::app(pool).await;
    let base = serve(&app).await;
    let client = reqwest::Client::new();
    let mut rx = app.store.subscribe();
    let root = app
        .engine
        .start(request(app.team, "test:hello"))
        .await
        .expect("start");
    events_until_finished(&mut rx, root).await;

    let messages = format!("{base}/api/runs/{root}/messages");
    let body = json!({"text": "again", "scenario": "test:hang"});
    let (a, b) = tokio::join!(
        post(&client, &messages, body.clone()),
        post(&client, &messages, body)
    );
    let mut statuses = [a, b];
    statuses.sort_unstable();
    assert_eq!(statuses, [202, 409]);
    app.engine.cancel(root);
}

#[sqlx::test]
async fn follow_ups_need_a_finished_lead_task(pool: SqlitePool) {
    let app = crate::app(pool).await;
    let base = serve(&app).await;
    let client = reqwest::Client::new();
    let mut rx = app.store.subscribe();
    let by_specialist = app
        .engine
        .start(StartRequest {
            agent_id: Some(app.specialist),
            ..request(app.team, "test:hello")
        })
        .await
        .expect("start");
    events_until_finished(&mut rx, by_specialist).await;

    let body = json!({"text": "again"});
    let url = |root: RunId| format!("{base}/api/runs/{root}/messages");
    assert_eq!(post(&client, &url(by_specialist), body.clone()).await, 409);
    assert_eq!(post(&client, &url(RunId::new()), body).await, 404);
    assert_eq!(
        post(&client, &url(by_specialist), json!({"text": "  "})).await,
        422
    );
}

/// A follow-up's run has no parent but is not a root: the drill report finds only the task's root.
#[sqlx::test]
async fn the_drill_report_is_found_by_root_only(pool: SqlitePool) {
    let app = crate::app(pool).await;
    let base = serve(&app).await;
    let client = reqwest::Client::new();
    let mut rx = app.store.subscribe();
    let root = app
        .engine
        .start(request(app.team, "test:hello"))
        .await
        .expect("start");
    events_until_finished(&mut rx, root).await;
    let follow_up = FollowUpRequest {
        text: "again".to_string(),
        scenario: Some("test:hello".to_string()),
        draft_only: false,
    };
    let run = app
        .engine
        .follow_up(root, follow_up)
        .await
        .expect("follow-up");
    events_until_finished(&mut rx, run).await;

    let status = |id: RunId| {
        let url = format!("{base}/api/drill/{id}/report");
        let client = client.clone();
        async move {
            client
                .get(url)
                .send()
                .await
                .expect("response")
                .status()
                .as_u16()
        }
    };
    assert_eq!(status(root).await, 200);
    assert_eq!(status(run).await, 404);
}

/// The calls each tool name made, with how they were decided, in request order.
fn decisions(events: &[RunEvent], tool: &str) -> Vec<(Option<Decision>, Option<Rule>)> {
    events
        .iter()
        .filter_map(|e| match e {
            RunEvent::ToolCallRequested {
                tool: name,
                decision,
                reasons,
                ..
            } if name == tool => Some((*decision, reasons.first().cloned())),
            _ => None,
        })
        .collect()
}

/// A draft-only S1 on the real tools: reads and delegations run; the save, the note and both
/// sends are refused by draft_only with no card, so nothing is written or sent. A later turn
/// that is not draft only makes the same send and it asks: a draft refusal is not a repeat.
#[sqlx::test]
async fn a_draft_only_turn_changes_nothing_and_a_later_turn_may_act(pool: SqlitePool) {
    use orchestrator_core::runs::CallStatus;
    use orchestrator_server::sqlite::outbox;

    let recording = Arc::new(Recording::new());
    let seeded = Seeded::with_llm(pool, recording.clone()).await;
    let engine = seeded.engine.clone();
    answer_approvals(&seeded.store, &engine, deny_rolf_naming_anna);
    let mut rx = seeded.store.subscribe();
    let request = StartRequest {
        task: S1_TASK.to_string(),
        draft_only: true,
        ..crate::request(seeded.team, "s1")
    };
    let root = engine.start(request).await.expect("start");
    let events: Vec<RunEvent> = task_events(&mut rx, root)
        .await
        .into_iter()
        .map(|e| e.event)
        .collect();

    let draft = (Some(Decision::Deny), Some(Rule::DraftOnly));
    assert_eq!(
        decisions(&events, "file_write"),
        std::slice::from_ref(&draft)
    );
    assert_eq!(
        decisions(&events, "memory_write"),
        std::slice::from_ref(&draft)
    );
    assert_eq!(decisions(&events, "send_message"), [draft.clone(), draft]);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, RunEvent::ApprovalRequested { .. })),
        "draft-only refusals show no card"
    );
    for tool in ["inbox_read", "file_read", "delegate", "memory_search"] {
        assert!(
            decisions(&events, tool)
                .iter()
                .all(|(d, _)| *d == Some(Decision::Allow)),
            "{tool} still runs"
        );
    }
    assert!(events.iter().any(|e| matches!(
        e,
        RunEvent::ToolCallFinished {
            status: CallStatus::Ok,
            ..
        }
    )));
    assert!(
        !seeded
            .temp
            .workspace()
            .join("decisions/AR-1042.md")
            .exists()
    );
    let notes = sqlx::query_scalar!("SELECT COUNT(*) FROM memory_entries")
        .fetch_one(&seeded.pool)
        .await
        .expect("count");
    assert_eq!(notes, 0);
    assert!(
        outbox::list(&seeded.pool, Default::default())
            .await
            .expect("outbox")
            .messages
            .is_empty()
    );

    let requests = recording.requests();
    let told = |slug: &str| {
        requests
            .iter()
            .filter(|r| r.script.agent.as_str() == slug)
            .all(|r| r.instructions.contains("This turn is draft only"))
    };
    assert!(told("lead"), "the Lead is told the turn is draft only");
    assert!(
        requests
            .iter()
            .filter(|r| r.script.agent.as_str() == "writer")
            .all(|r| !r.instructions.contains("This turn is draft only"))
    );
    let detail = runs::detail(&seeded.pool, root)
        .await
        .expect("read")
        .expect("task");
    assert!(
        detail.runs.iter().all(|run| run.draft_only),
        "children inherit it"
    );

    // The follow-up is not draft only: the send to Rolf asks instead of being a repeat.
    let (_, later) = follow_up_s1(&seeded, &engine, root).await;
    let rolf = decisions(&later, "send_message");
    assert_eq!(rolf.len(), 2);
    assert_eq!(rolf[0].0, Some(Decision::Ask), "not refused as a repeat");
    let cards = later
        .iter()
        .filter(|e| matches!(e, RunEvent::ApprovalRequested { .. }))
        .count();
    assert_eq!(cards, 2, "Rolf and Anna each show a card");
    let detail = runs::detail(&seeded.pool, root)
        .await
        .expect("read")
        .expect("task");
    let drafts: Vec<bool> = detail.turns.iter().map(|t| t.draft_only).collect();
    assert_eq!(drafts, [true, false]);
}
