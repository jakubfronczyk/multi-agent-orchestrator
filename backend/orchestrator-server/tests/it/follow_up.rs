//! Follow-ups: a finished task takes the Lead's next turn, rebuilt from the event log, so taint
//! and refused calls carry over, even into an engine that never ran the task.

use std::sync::Arc;

use orchestrator_core::engine::{Engine, FollowUpRequest, StartRequest};
use orchestrator_core::ids::RunId;
use orchestrator_core::llm::{Item, LlmProvider};
use orchestrator_core::policy::{Decision, Rule, TaintSource};
use orchestrator_core::runs::{Answer, RunEvent, RunOutcome};
use orchestrator_core::tools::Target;
use orchestrator_server::events::EventEnvelope;
use orchestrator_server::sqlite::{SqliteStore, runs};
use orchestrator_server::tools::files::Workspace;
use orchestrator_server::{seed, tools};
use serde_json::json;
use sqlx::SqlitePool;
use tokio::sync::broadcast;

use crate::recording::Recording;
use crate::{
    DEADLINE, TempDir, app_with_llm, events_until_finished, request, serve, task_events,
    test_config,
};

const S1_TASK: &str =
    "Check access request AR-1042 against our policy and email the decision to the owner";
const FOLLOW_UP: &str = "Email Rolf the decision again, then Anna instead";

/// The seeded team on the real tools, with a provider that records every request.
struct Seeded {
    pool: SqlitePool,
    store: Arc<SqliteStore>,
    recording: Arc<Recording>,
    temp: TempDir,
}

impl Seeded {
    async fn new(pool: SqlitePool) -> Self {
        let temp = TempDir::new();
        assert!(seed::run(&pool, &temp.workspace()).await.expect("seed"));
        Self {
            store: Arc::new(SqliteStore::new(pool.clone())),
            recording: Arc::new(Recording::slowing("none")),
            pool,
            temp,
        }
    }

    /// Engines share the store and workspace; a second one knows the task only from the log.
    fn engine(&self) -> Arc<Engine> {
        let tools = tools::all(
            self.pool.clone(),
            Workspace::new(self.temp.workspace(), self.pool.clone()),
        );
        let llm: Arc<dyn LlmProvider> = self.recording.clone();
        Arc::new(Engine::new(self.store.clone(), llm, tools, test_config()))
    }
}

/// Denies the send to Rolf and approves everything else, as in the S1 demo.
fn answer_as_in_s1(store: &SqliteStore, engine: &Arc<Engine>) {
    let mut rx = store.subscribe();
    let engine = Arc::clone(engine);
    tokio::spawn(async move {
        while let Ok(envelope) = rx.recv().await {
            if let RunEvent::ApprovalRequested {
                approval_id,
                action,
                ..
            } = envelope.event
            {
                let answer = match action.target {
                    Target::Recipient { address, .. } if address.starts_with("rolf") => {
                        Answer::Deny {
                            note: Some("wrong person, owner is Anna Weber".to_string()),
                        }
                    }
                    _ => Answer::Approve { note: None },
                };
                engine.answer(approval_id, answer);
            }
        }
    });
}

async fn run_s1(seeded: &Seeded, engine: &Engine) -> RunId {
    let mut rx = seeded.store.subscribe();
    let request = StartRequest {
        agent_id: None,
        task: S1_TASK.to_string(),
        title: None,
        budget: None,
        scenario: Some("s1".to_string()),
    };
    let root = engine.start(request).await.expect("start");
    task_events(&mut rx, root).await;
    root
}

/// Every event of the task from now until the turn `run` finishes.
async fn turn_events(
    rx: &mut broadcast::Receiver<EventEnvelope>,
    root: RunId,
    run: RunId,
) -> Vec<RunEvent> {
    let mut events = Vec::new();
    loop {
        let envelope = tokio::time::timeout(DEADLINE, rx.recv())
            .await
            .expect("turn finished before the deadline")
            .expect("broadcast open");
        if envelope.root_run_id != root {
            continue;
        }
        let done = envelope.run_id == run && matches!(envelope.event, RunEvent::RunFinished { .. });
        events.push(envelope.event);
        if done {
            return events;
        }
    }
}

async fn follow_up_s1(seeded: &Seeded, engine: &Engine, root: RunId) -> (RunId, Vec<RunEvent>) {
    let mut rx = seeded.store.subscribe();
    let request = FollowUpRequest {
        text: FOLLOW_UP.to_string(),
        scenario: Some("s1-followup".to_string()),
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
    let seeded = Seeded::new(pool).await;
    let engine = seeded.engine();
    answer_as_in_s1(&seeded.store, &engine);
    let root = run_s1(&seeded, &engine).await;

    let (run, events) = follow_up_s1(&seeded, &engine, root).await;
    assert_s1_carried_over(&events);

    // The Lead's input: S1's text, its summary wrapped as untrusted, then the follow-up.
    let first_request = seeded
        .recording
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
    let list = runs::list(&seeded.pool, 10).await.expect("list");
    assert_eq!(list.runs.len(), 1, "a follow-up is not a new task");
}

/// A fresh engine has no memory of the task; the log alone restores taint and refusals.
#[sqlx::test]
async fn a_fresh_engine_rebuilds_the_task_from_the_log(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let first = seeded.engine();
    answer_as_in_s1(&seeded.store, &first);
    let root = run_s1(&seeded, &first).await;
    drop(first);

    let fresh = seeded.engine();
    answer_as_in_s1(&seeded.store, &fresh);
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
    let recording = Arc::new(Recording::slowing("none"));
    let app = app_with_llm(pool, test_config(), recording.clone()).await;
    let base = serve(&app).await;
    let client = reqwest::Client::new();
    let mut rx = app.store.subscribe();
    let root = app.engine.start(request("test:hang")).await.expect("start");
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
    assert_eq!(detail.turns[1].summary.as_deref(), Some("hello"));
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
        .start(request("test:hello"))
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
    let by_worker = app
        .engine
        .start(StartRequest {
            agent_id: Some(app.worker),
            ..request("test:hello")
        })
        .await
        .expect("start");
    events_until_finished(&mut rx, by_worker).await;

    let body = json!({"text": "again"});
    let url = |root: RunId| format!("{base}/api/runs/{root}/messages");
    assert_eq!(post(&client, &url(by_worker), body.clone()).await, 409);
    assert_eq!(post(&client, &url(RunId::new()), body).await, 404);
    assert_eq!(
        post(&client, &url(by_worker), json!({"text": "  "})).await,
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
        .start(request("test:hello"))
        .await
        .expect("start");
    events_until_finished(&mut rx, root).await;
    let follow_up = FollowUpRequest {
        text: "again".to_string(),
        scenario: Some("test:hello".to_string()),
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
