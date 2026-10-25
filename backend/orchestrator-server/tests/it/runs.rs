//! Checkpoint A: a run starts, calls the model and finishes, and every exit of §7.1 is observable.

use futures_util::StreamExt;
use orchestrator_core::runs::{FailReason, RunEvent, RunOutcome};
use serde_json::Value;
use sqlx::SqlitePool;

use std::time::Duration;

use crate::{DEADLINE, app, app_with, events_until_finished, request, test_config};

#[sqlx::test]
async fn post_runs_streams_the_run_over_sse(pool: SqlitePool) {
    let app = app(pool).await;
    let base = crate::serve(&app).await;

    let client = reqwest::Client::new();
    let mut sse = client
        .get(format!("{base}/api/events"))
        .send()
        .await
        .expect("sse")
        .bytes_stream();
    let started = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({"task": "say hello", "scenario": "test:hello"}))
        .send()
        .await
        .expect("post");
    assert_eq!(started.status(), 202);

    let mut buffer = String::new();
    let mut seen: Vec<(i64, String)> = Vec::new();
    while !seen.iter().any(|(_, kind)| kind == "run_finished") {
        let chunk = tokio::time::timeout(DEADLINE, sse.next())
            .await
            .expect("event before the deadline")
            .expect("stream open")
            .expect("chunk");
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buffer.find("\n\n") {
            let frame: String = buffer.drain(..end + 2).collect();
            for data in frame.lines().filter_map(|line| line.strip_prefix("data: ")) {
                let envelope: Value = serde_json::from_str(data).expect("json");
                let id = envelope["id"].as_i64().expect("id");
                seen.push((id, envelope["type"].as_str().expect("type").to_string()));
            }
        }
    }

    let kinds: Vec<&str> = seen.iter().map(|(_, kind)| kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "run_started",
            "llm_requested",
            "llm_responded",
            "run_finished"
        ]
    );
    assert!(
        seen.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "ids increase"
    );
}

#[sqlx::test]
async fn a_retryable_error_is_retried_and_logged(pool: SqlitePool) {
    let app = app(pool).await;
    let mut rx = app.store.subscribe();
    let run = app
        .engine
        .start(request("test:retry"))
        .await
        .expect("start");
    let events = events_until_finished(&mut rx, run).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, RunEvent::LlmRetrying { attempt: 1, .. }))
    );
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            outcome: RunOutcome::Completed {
                truncated: false,
                ..
            }
        })
    ));
}

#[sqlx::test]
async fn a_fatal_error_fails_the_run_with_llm_error(pool: SqlitePool) {
    let app = app(pool).await;
    let mut rx = app.store.subscribe();
    let run = app
        .engine
        .start(request("test:fatal"))
        .await
        .expect("start");
    let events = events_until_finished(&mut rx, run).await;

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, RunEvent::LlmRetrying { .. }))
    );
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            outcome: RunOutcome::Failed {
                reason: FailReason::LlmError,
                ..
            }
        })
    ));
}

#[sqlx::test]
async fn cancel_during_a_model_call_ends_the_run_cancelled(pool: SqlitePool) {
    let app = app(pool).await;
    let mut rx = app.store.subscribe();
    let run = app.engine.start(request("test:hang")).await.expect("start");

    loop {
        let envelope = tokio::time::timeout(DEADLINE, rx.recv())
            .await
            .expect("event")
            .expect("open");
        if matches!(envelope.event, RunEvent::LlmRequested { .. }) {
            break;
        }
    }
    assert!(app.engine.cancel(run));
    let events = events_until_finished(&mut rx, run).await;
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            outcome: RunOutcome::Cancelled
        })
    ));
}

#[sqlx::test]
async fn a_stalled_model_call_times_out_and_fails_after_the_retries(pool: SqlitePool) {
    let config = test_config();
    let retries = config.retry_backoff.len();
    let app = app_with(
        pool,
        crate::Config {
            llm_timeout: Duration::from_millis(20),
            ..config
        },
    )
    .await;
    let mut rx = app.store.subscribe();
    let run = app.engine.start(request("test:hang")).await.expect("start");
    let events = events_until_finished(&mut rx, run).await;

    let retried = events
        .iter()
        .filter(|e| matches!(e, RunEvent::LlmRetrying { .. }))
        .count();
    assert_eq!(retried, retries);
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            outcome: RunOutcome::Failed {
                reason: FailReason::LlmError,
                ..
            }
        })
    ));
}

#[sqlx::test]
async fn each_task_replays_its_script_from_the_first_attempt(pool: SqlitePool) {
    let app = app(pool).await;
    let mut rx = app.store.subscribe();
    for _ in 0..2 {
        let run = app
            .engine
            .start(request("test:retry"))
            .await
            .expect("start");
        let events = events_until_finished(&mut rx, run).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RunEvent::LlmRetrying { attempt: 1, .. }))
        );
    }
}
