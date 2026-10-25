//! Checkpoint A: a run starts, calls the model and finishes, and every exit of §7.1 is observable.

use orchestrator_core::runs::{FailReason, RunEvent, RunOutcome};
use serde_json::Value;
use sqlx::SqlitePool;

use std::time::Duration;

use crate::{DEADLINE, app, app_with, events_until_finished, request, test_config};

#[sqlx::test]
async fn post_runs_streams_the_run_over_sse(pool: SqlitePool) {
    let app = app(pool).await;
    let base = crate::serve(&app).await;

    let mut sse = SseReader::open(&base, None).await;
    let started = reqwest::Client::new()
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({"task": "say hello", "scenario": "test:hello"}))
        .send()
        .await
        .expect("post");
    assert_eq!(started.status(), 202);
    let seen = sse.until("run_finished").await;

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

/// Reads `(id, type)` pairs from an SSE response body, one frame at a time.
struct SseReader {
    response: reqwest::Response,
    buffer: String,
}

impl SseReader {
    async fn open(base: &str, last_event_id: Option<i64>) -> Self {
        let mut request = reqwest::Client::new().get(format!("{base}/api/events"));
        if let Some(id) = last_event_id {
            request = request.header("Last-Event-ID", id.to_string());
        }
        let response = request.send().await.expect("sse");
        assert_eq!(response.status(), 200);
        Self {
            response,
            buffer: String::new(),
        }
    }

    /// Events until one of type `until` arrives, that one included.
    async fn until(&mut self, until: &str) -> Vec<(i64, String)> {
        let mut seen = Vec::new();
        loop {
            while let Some(end) = self.buffer.find("\n\n") {
                let frame: String = self.buffer.drain(..end + 2).collect();
                for data in frame.lines().filter_map(|line| line.strip_prefix("data: ")) {
                    let envelope: Value = serde_json::from_str(data).expect("json");
                    let kind = envelope["type"].as_str().expect("type").to_string();
                    seen.push((envelope["id"].as_i64().expect("id"), kind.clone()));
                    if kind == until {
                        return seen;
                    }
                }
            }
            let chunk = tokio::time::timeout(DEADLINE, self.response.chunk())
                .await
                .expect("event before the deadline")
                .expect("chunk")
                .expect("stream open");
            self.buffer.push_str(&String::from_utf8_lossy(&chunk));
        }
    }
}

#[sqlx::test]
async fn last_event_id_replays_what_was_missed_then_continues_live(pool: SqlitePool) {
    let app = app(pool).await;
    let base = crate::serve(&app).await;
    let mut rx = app.store.subscribe();
    let first = app
        .engine
        .start(request("test:hello"))
        .await
        .expect("start");
    let mut missed = Vec::new();
    loop {
        let envelope = tokio::time::timeout(DEADLINE, rx.recv())
            .await
            .expect("event")
            .expect("open");
        missed.push(envelope.id.0);
        if envelope.run_id == first && matches!(envelope.event, RunEvent::RunFinished { .. }) {
            break;
        }
    }

    let mut sse = SseReader::open(&base, Some(missed[0])).await;
    let replayed = sse.until("run_finished").await;
    let ids: Vec<i64> = replayed.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, missed[1..], "everything after the given id, in order");

    app.engine
        .start(request("test:hello"))
        .await
        .expect("second");
    let live = sse.until("run_finished").await;
    assert_eq!(live[0].1, "run_started");
    assert!(
        live.iter()
            .all(|(id, _)| id > missed.last().expect("missed")),
        "the live stream repeats nothing the replay sent"
    );
    assert!(live.windows(2).all(|pair| pair[0].0 < pair[1].0));
}

#[sqlx::test]
async fn without_last_event_id_the_stream_is_live_only(pool: SqlitePool) {
    let app = app(pool).await;
    let base = crate::serve(&app).await;
    let mut rx = app.store.subscribe();
    let earlier = app
        .engine
        .start(request("test:hello"))
        .await
        .expect("start");
    events_until_finished(&mut rx, earlier).await;

    let mut sse = SseReader::open(&base, None).await;
    app.engine
        .start(request("test:hello"))
        .await
        .expect("second");
    let live = sse.until("run_finished").await;
    assert_eq!(
        live.iter()
            .map(|(_, kind)| kind.as_str())
            .collect::<Vec<_>>(),
        [
            "run_started",
            "llm_requested",
            "llm_responded",
            "run_finished"
        ],
        "nothing from the earlier task is replayed"
    );
}

#[sqlx::test]
async fn an_id_from_before_a_database_reset_is_treated_as_live_only(pool: SqlitePool) {
    let app = app(pool).await;
    let base = crate::serve(&app).await;
    let mut sse = SseReader::open(&base, Some(500)).await;
    app.engine
        .start(request("test:hello"))
        .await
        .expect("start");
    let live = sse.until("run_finished").await;
    assert_eq!(
        live[0].1, "run_started",
        "new events are not hidden behind id 500"
    );
}
