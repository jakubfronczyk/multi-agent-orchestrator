//! Read endpoints and cancel over HTTP: the task list, one task's detail and the fixed facts.

use orchestrator_core::engine::StartRequest;
use orchestrator_core::ids::RunId;
use orchestrator_core::runs::{RunEvent, RunOutcome};
use serde_json::Value;
use sqlx::SqlitePool;

use crate::{DEADLINE, app, events_until_finished, request, serve};

async fn get(client: &reqwest::Client, url: &str) -> (u16, Value) {
    let response = client.get(url).send().await.expect("get");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

fn id_text(run: RunId) -> String {
    run.to_string()
}

#[sqlx::test]
async fn a_waiting_task_is_listed_read_in_detail_and_cancelled(pool: SqlitePool) {
    let app = app(pool).await;
    let base = serve(&app).await;
    let client = reqwest::Client::new();
    let mut rx = app.store.subscribe();
    let root = app
        .engine
        .start(StartRequest {
            agent_id: Some(app.worker),
            ..request(r#"test:call:file_write:{"path":"a.md"}"#)
        })
        .await
        .expect("start");
    loop {
        let envelope = tokio::time::timeout(DEADLINE, rx.recv())
            .await
            .expect("event")
            .expect("open");
        if matches!(envelope.event, RunEvent::ApprovalRequested { .. }) {
            break;
        }
    }

    let (status, list) = get(&client, &format!("{base}/api/runs")).await;
    assert_eq!(status, 200);
    let task = &list["runs"][0];
    assert_eq!(task["id"], id_text(root));
    assert_eq!(task["status"], "running");
    assert_eq!(task["pending_approvals"], 1);
    assert_eq!(task["agents"].as_array().map(Vec::len), Some(1));
    assert!(list["last_event_id"].as_i64().is_some());

    let (status, detail) = get(&client, &format!("{base}/api/runs/{}", id_text(root))).await;
    assert_eq!(status, 200);
    assert_eq!(detail["runs"].as_array().map(Vec::len), Some(1));
    assert_eq!(detail["tool_calls"][0]["tool"], "file_write");
    assert_eq!(detail["tool_calls"][0]["phase"], "waiting");
    assert!(detail["tool_calls"][0]["action"].is_object());
    assert_eq!(
        detail["pending_approvals"][0]["tool_call_id"],
        detail["tool_calls"][0]["id"]
    );

    let cancel = format!("{base}/api/runs/{}/cancel", id_text(root));
    let response = client.post(&cancel).send().await.expect("cancel");
    assert_eq!(response.status(), 202);
    let events = events_until_finished(&mut rx, root).await;
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            outcome: RunOutcome::Cancelled
        })
    ));

    let again = client.post(&cancel).send().await.expect("again");
    assert_eq!(again.status(), 409, "a finished task cannot be cancelled");
    let (_, after) = get(&client, &format!("{base}/api/runs")).await;
    assert_eq!(after["runs"][0]["status"], "cancelled");
    assert_eq!(after["runs"][0]["pending_approvals"], 0);
}

#[sqlx::test]
async fn unknown_tasks_are_not_found_and_the_limit_is_checked(pool: SqlitePool) {
    let app = app(pool).await;
    let base = serve(&app).await;
    let client = reqwest::Client::new();
    let unknown = id_text(RunId::new());

    assert_eq!(
        get(&client, &format!("{base}/api/runs/{unknown}")).await.0,
        404
    );
    assert_eq!(
        get(&client, &format!("{base}/api/runs/not-an-id")).await.0,
        404
    );
    let cancel = client
        .post(format!("{base}/api/runs/{unknown}/cancel"))
        .send()
        .await
        .expect("cancel");
    assert_eq!(cancel.status(), 404);
    let (status, body) = get(&client, &format!("{base}/api/runs?limit=201")).await;
    assert_eq!(status, 422);
    assert_eq!(body["error"]["field"], "limit");
    let (status, empty) = get(&client, &format!("{base}/api/runs")).await;
    assert_eq!(status, 200);
    assert_eq!(empty["runs"].as_array().map(Vec::len), Some(0));
    assert!(empty["last_event_id"].is_null());
}

#[sqlx::test]
async fn health_models_and_tools_describe_the_fixed_setup(pool: SqlitePool) {
    let app = app(pool).await;
    let base = serve(&app).await;
    let client = reqwest::Client::new();

    let (_, health) = get(&client, &format!("{base}/api/health")).await;
    assert_eq!(health["provider"], "scripted");

    let (_, models) = get(&client, &format!("{base}/api/models")).await;
    assert_eq!(models["models"][0]["id"], "grok-4.3");
    assert_eq!(
        models["models"][0]["efforts"].as_array().map(Vec::len),
        Some(4)
    );
    assert_eq!(models["models"][1]["efforts"], serde_json::json!([]));

    let (_, tools) = get(&client, &format!("{base}/api/tools")).await;
    let tools = tools["tools"].as_array().expect("tools");
    let tool = |name: &str| {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name} listed"))
    };
    assert_eq!(tool("delegate")["lead"], "only");
    assert_eq!(tool("plan")["effect"], "control");
    assert_eq!(tool("send_message")["effect"], "external");
    assert_eq!(tool("inbox_read")["taints"], true);
    assert!(
        tool("file_read")["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty())
    );
}
