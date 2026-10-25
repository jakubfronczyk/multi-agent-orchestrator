//! Read endpoints, cancel and agent rules over HTTP: the task list, one task's detail and the
//! fixed facts.

use orchestrator_core::engine::StartRequest;
use orchestrator_core::ids::RunId;
use orchestrator_core::runs::{RunEvent, RunOutcome};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{app, events_until_finished, request, serve, wait_for};

async fn get(client: &reqwest::Client, url: &str) -> (u16, Value) {
    let response = client.get(url).send().await.expect("get");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
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
    wait_for(&mut rx, |envelope| {
        matches!(envelope.event, RunEvent::ApprovalRequested { .. }).then_some(())
    })
    .await;

    let (status, list) = get(&client, &format!("{base}/api/runs")).await;
    assert_eq!(status, 200);
    let task = &list["runs"][0];
    assert_eq!(task["id"], root.to_string());
    assert_eq!(task["status"], "running");
    assert_eq!(task["pending_approvals"], 1);
    assert_eq!(task["agents"].as_array().map(Vec::len), Some(1));
    assert!(list["last_event_id"].as_i64().is_some());

    let (status, detail) = get(&client, &format!("{base}/api/runs/{root}")).await;
    assert_eq!(status, 200);
    assert_eq!(detail["runs"].as_array().map(Vec::len), Some(1));
    assert_eq!(detail["tool_calls"][0]["tool"], "file_write");
    assert_eq!(detail["tool_calls"][0]["phase"], "waiting");
    assert!(detail["tool_calls"][0]["action"].is_object());
    assert_eq!(
        detail["pending_approvals"][0]["tool_call_id"],
        detail["tool_calls"][0]["id"]
    );
    let (_, cards) = get(&client, &format!("{base}/api/approvals?status=pending")).await;
    assert_eq!(cards["approvals"][0]["root_run_id"], root.to_string());

    let cancel = format!("{base}/api/runs/{root}/cancel");
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
    let unknown = RunId::new().to_string();

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
    assert_eq!(tools["allowlist"], serde_json::json!(["example.com"]));
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

/// Configuration only tightens: an agent that would send without asking is refused over HTTP.
#[sqlx::test]
async fn an_agent_that_sends_without_asking_is_refused(pool: SqlitePool) {
    let app = app(pool).await;
    let base = serve(&app).await;
    let client = reqwest::Client::new();
    let agent = json!({
        "role": "specialist",
        "slug": "loud",
        "name": "Loud",
        "description": "sends freely",
        "system_prompt": "You send.",
        "model": "grok-4.3",
        "reasoning_effort": null,
        "icon": "i",
        "colour": "c",
        "tools": {"send_message": "allow"},
        "delegate_targets": []
    });
    let response = client
        .post(format!("{base}/api/agents"))
        .json(&agent)
        .send()
        .await
        .expect("response");
    assert_eq!(response.status().as_u16(), 422);
    let body: Value = response.json().await.expect("json");
    assert_eq!(body["error"]["code"], "invalid_input", "{body}");
    assert_eq!(body["error"]["field"], "tools", "{body}");

    let (_, listed) = get(&client, &format!("{base}/api/agents")).await;
    let slugs: Vec<&str> = listed
        .as_array()
        .expect("agents")
        .iter()
        .filter_map(|a| a["slug"].as_str())
        .collect();
    assert!(
        slugs.contains(&"worker") && !slugs.contains(&"loud"),
        "nothing stored: {slugs:?}"
    );
}

/// DNS rebinding and cross-site posts cannot answer a card: a non-loopback Host is refused on
/// every request, and a non-loopback Origin on every change. The app's own origin still works.
#[sqlx::test]
async fn only_this_app_on_loopback_can_answer_a_card(pool: SqlitePool) {
    let app = app(pool).await;
    let base = serve(&app).await;
    let client = reqwest::Client::new();
    let mut rx = app.store.subscribe();
    app.engine
        .start(StartRequest {
            agent_id: Some(app.worker),
            ..request(r#"test:call:file_write:{"path":"a.md"}"#)
        })
        .await
        .expect("start");
    let approval = wait_for(&mut rx, |envelope| match envelope.event {
        RunEvent::ApprovalRequested { approval_id, .. } => Some(approval_id),
        _ => None,
    })
    .await;
    let approval = serde_json::to_value(approval).expect("id");
    let answer = format!("{base}/api/approvals/{}", approval.as_str().expect("str"));
    let approve = json!({"decision": "approve"});

    let rebound = client
        .post(&answer)
        .header("host", "evil.test:8787")
        .json(&approve)
        .send()
        .await
        .expect("post");
    assert_eq!(rebound.status(), 403);
    let body: Value = rebound.json().await.expect("json");
    assert_eq!(body["error"]["code"], "forbidden");
    let read = client
        .get(format!("{base}/api/approvals?status=pending"))
        .header("host", "evil.test")
        .send()
        .await
        .expect("get");
    assert_eq!(read.status(), 403);

    let cross_site = client
        .post(&answer)
        .header("origin", "https://evil.test")
        .json(&approve)
        .send()
        .await
        .expect("post");
    assert_eq!(cross_site.status(), 403);
    // Another local app's page is another origin too.
    let other_port = client
        .post(&answer)
        .header("origin", "http://localhost:3000")
        .json(&approve)
        .send()
        .await
        .expect("post");
    assert_eq!(other_port.status(), 403);
    let (_, pending) = get(&client, &format!("{base}/api/approvals?status=pending")).await;
    assert_eq!(pending["approvals"][0]["status"], "pending");

    let own_origin = base.replace("127.0.0.1", "LOCALHOST");
    let own = client
        .post(&answer)
        .header(
            "host",
            own_origin.trim_start_matches("http://").to_lowercase(),
        )
        .header("origin", own_origin)
        .json(&approve)
        .send()
        .await
        .expect("post");
    assert_eq!(own.status(), 204);
}

/// `GET /agents` lists every team's agents, each with its `team_id`; `?team_id=` narrows it.
#[sqlx::test]
async fn agents_are_listed_per_team(pool: SqlitePool) {
    let app = app(pool).await;
    let other = crate::new_team(&app.pool, "Other").await;
    let input: orchestrator_core::agents::AgentInput = serde_json::from_value(json!({
        "team_id": other, "role": "specialist", "slug": "worker", "name": "w", "description": "d",
        "system_prompt": "p", "model": "grok-4.3", "icon": "i", "colour": "c", "tools": {},
        "delegate_targets": []
    }))
    .expect("input");
    orchestrator_server::sqlite::agents::create(&app.pool, input)
        .await
        .expect("same slug on another team");
    let base = serve(&app).await;
    let client = reqwest::Client::new();

    let (status, all) = get(&client, &format!("{base}/api/agents")).await;
    assert_eq!(status, 200);
    let all = all.as_array().expect("list").clone();
    assert!(all.iter().all(|agent| agent["team_id"].is_string()));
    let (_, only) = get(
        &client,
        &format!(
            "{base}/api/agents?team_id={}",
            json!(other).as_str().expect("id")
        ),
    )
    .await;
    let only = only.as_array().expect("list");
    assert_eq!(only.len(), 1);
    assert_eq!(only[0]["slug"], "worker");
    assert!(all.len() > only.len());
    let (status, body) = get(&client, &format!("{base}/api/agents?team_id=nope")).await;
    assert_eq!(status, 422);
    assert_eq!(body["error"]["field"], "team_id");
}
