//! A task runs inside one team: its runs, agents and records carry that team, the API resolves
//! which team a start belongs to, and a deleted team takes no new run.

use orchestrator_core::engine::StartRequest;
use orchestrator_core::ids::{AgentId, TeamId};
use orchestrator_core::runs::{RunEvent, RunOutcome};
use orchestrator_server::sqlite::{agents, runs};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{App, app, events_until_finished, request, second_team, serve};

async fn team_agents(app: &App, team: TeamId) -> Vec<AgentId> {
    agents::list(&app.pool, Some(team))
        .await
        .expect("agents")
        .into_iter()
        .map(|record| record.spec.id())
        .collect()
}

async fn slug_in(app: &App, team: TeamId, slug: &str) -> AgentId {
    agents::list(&app.pool, Some(team))
        .await
        .expect("agents")
        .into_iter()
        .find(|record| record.spec.slug().as_str() == slug)
        .map(|record| record.spec.id())
        .expect("agent")
}

async fn post(base: &str, path: &str, body: Value) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .json(&body)
        .send()
        .await
        .expect("post");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

fn id(team: TeamId) -> String {
    json!(team).as_str().expect("id").to_string()
}

/// Both teams have a `helper`; a task on team B delegates to B's, never A's.
#[sqlx::test]
async fn a_task_on_team_b_runs_only_team_b_agents(pool: SqlitePool) {
    let app = app(pool).await;
    let b = second_team(&app).await;
    let mut rx = app.store.subscribe();
    let root = app
        .engine
        .start(StartRequest {
            team_id: Some(b),
            ..request("test:delegate")
        })
        .await
        .expect("start");
    let mut started = Vec::new();
    loop {
        let envelope = tokio::time::timeout(crate::DEADLINE, rx.recv())
            .await
            .expect("in time")
            .expect("open");
        if let RunEvent::RunStarted {
            agent_id, team_id, ..
        } = envelope.event
        {
            started.push((agent_id, team_id));
        }
        if envelope.run_id == root && matches!(envelope.event, RunEvent::RunFinished { .. }) {
            break;
        }
    }
    let b_agents = team_agents(&app, b).await;
    assert!(started.len() >= 2, "the Lead and its helper ran");
    assert!(
        started
            .iter()
            .all(|(agent, team)| *team == b && b_agents.contains(agent))
    );

    let listed = runs::list(&app.pool, 10, Some(b)).await.expect("list");
    assert_eq!(listed.runs.len(), 1);
    assert_eq!(listed.runs[0].team_id, b);
    let detail = runs::detail(&app.pool, root)
        .await
        .expect("read")
        .expect("task");
    assert_eq!(detail.team_id, b);
    let a_list = runs::list(&app.pool, 10, Some(app.team))
        .await
        .expect("list");
    assert!(a_list.runs.is_empty());
}

#[sqlx::test]
async fn post_runs_resolves_the_team_from_the_agent_or_the_request(pool: SqlitePool) {
    let app = app(pool).await;
    let b = second_team(&app).await;
    let base = serve(&app).await;
    let a_reader = slug_in(&app, app.team, "reader").await;

    let (status, body) = post(
        &base,
        "/api/runs",
        json!({"task": "t", "agent_id": a_reader, "team_id": id(b), "scenario": "test:hello"}),
    )
    .await;
    assert_eq!(
        (status, body["error"]["field"].clone()),
        (422, json!("agent_id"))
    );
    assert_eq!(body["error"]["message"], "not on this team");

    let (status, body) = post(
        &base,
        "/api/runs",
        json!({"task": "t", "team_id": id(TeamId::new()), "scenario": "test:hello"}),
    )
    .await;
    assert_eq!(
        (status, body["error"]["field"].clone()),
        (422, json!("team_id"))
    );

    let client = reqwest::Client::new();
    for (team, expected) in [(Some(b), b), (None, app.team)] {
        let mut body = json!({"task": "t", "scenario": "test:hello"});
        if let Some(team) = team {
            body["team_id"] = json!(id(team));
        }
        let (status, started) = post(&base, "/api/runs", body).await;
        assert_eq!(status, 202);
        let run = started["run_id"].as_str().expect("run").to_string();
        let detail: Value = client
            .get(format!("{base}/api/runs/{run}"))
            .send()
            .await
            .expect("get")
            .json()
            .await
            .expect("json");
        assert_eq!(detail["team_id"], json!(id(expected)));
        let lead = slug_in(&app, expected, "lead").await;
        assert_eq!(detail["runs"][0]["agent_id"], json!(lead));
    }

    let only_b: Value = client
        .get(format!("{base}/api/runs?team_id={}", id(b)))
        .send()
        .await
        .expect("get")
        .json()
        .await
        .expect("json");
    assert_eq!(only_b["runs"].as_array().map(Vec::len), Some(1));
    let bad = client
        .get(format!("{base}/api/runs?team_id=nope"))
        .send()
        .await
        .expect("get");
    assert_eq!(bad.status(), 422);
}

/// A scripted demo needs its agents on the team's Lead; a Lead-only team gets a 409 naming them.
#[sqlx::test]
async fn demos_need_their_agents_on_the_team(pool: SqlitePool) {
    let app = app(pool).await;
    let b = second_team(&app).await;
    let solo = orchestrator_server::sqlite::teams::create(
        &app.pool,
        orchestrator_server::sqlite::teams::TeamInput {
            name: "Solo".to_string(),
        },
    )
    .await
    .expect("team")
    .id;
    let base = serve(&app).await;

    let (status, body) = post(
        &base,
        "/api/runs",
        json!({"task": "t", "team_id": id(solo), "scenario": "s1"}),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(
        body["error"]["message"],
        "this team has no reader, writer, messenger for the s1 demo"
    );
    let (status, body) = post(
        &base,
        &format!("/api/drill?team_id={}", id(solo)),
        json!(null),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(
        body["error"]["message"],
        "this team's Lead cannot hand work to reader, messenger; the drill needs them"
    );

    let (status, _) = post(&base, &format!("/api/drill?team_id={}", id(b)), json!(null)).await;
    assert_eq!(status, 202);
    let (status, _) = post(
        &base,
        "/api/runs",
        json!({"task": "t", "team_id": id(b), "scenario": "clean"}),
    )
    .await;
    assert_eq!(status, 202);
}

/// No run is recorded on a deleted team, and a finished task of one takes no follow-up.
#[sqlx::test]
async fn a_deleted_team_takes_no_new_run(pool: SqlitePool) {
    let app = app(pool).await;
    let b = second_team(&app).await;
    let mut rx = app.store.subscribe();
    let root = app
        .engine
        .start(StartRequest {
            team_id: Some(b),
            ..request("test:hello")
        })
        .await
        .expect("start");
    let events = events_until_finished(&mut rx, root).await;
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            outcome: RunOutcome::Completed { .. }
        })
    ));
    let b_text = id(b);
    sqlx::query!(
        "UPDATE teams SET deleted_at = updated_at WHERE id = ?",
        b_text
    )
    .execute(&app.pool)
    .await
    .expect("delete by SQL");

    let base = serve(&app).await;
    let (status, body) = post(
        &base,
        &format!("/api/runs/{}/messages", json!(root).as_str().expect("id")),
        json!({"text": "again"}),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(body["error"]["message"], "this task's team was deleted");

    let runs_before = sqlx::query_scalar!("SELECT COUNT(*) FROM runs")
        .fetch_one(&app.pool)
        .await
        .expect("count");
    let events_before = sqlx::query_scalar!("SELECT COUNT(*) FROM run_events")
        .fetch_one(&app.pool)
        .await
        .expect("count");
    let result = app
        .engine
        .start(StartRequest {
            team_id: Some(b),
            ..request("test:hello")
        })
        .await;
    assert!(result.is_err(), "the engine finds no live team");
    // The projection guard holds even for a start that raced the delete.
    use orchestrator_core::store::{RunRef, Store};
    let run = orchestrator_core::ids::RunId::new();
    let lead = slug_in(&app, app.team, "lead").await;
    let raced = app
        .store
        .emit(
            RunRef {
                root_run_id: run,
                run_id: run,
            },
            RunEvent::RunStarted {
                agent_id: lead,
                team_id: b,
                parent_run_id: None,
                parent_call_id: None,
                task: "t".to_string(),
                task_sources: Vec::new(),
                title: None,
                budget: None,
                scenario: None,
                agent_version: None,
                draft_only: false,
            },
        )
        .await;
    assert!(raced.is_err());
    let runs_after = sqlx::query_scalar!("SELECT COUNT(*) FROM runs")
        .fetch_one(&app.pool)
        .await
        .expect("count");
    let events_after = sqlx::query_scalar!("SELECT COUNT(*) FROM run_events")
        .fetch_one(&app.pool)
        .await
        .expect("count");
    assert_eq!((runs_after, events_after), (runs_before, events_before));
}

/// Every envelope, live and replayed, carries its task's team; lists filter by team and keep
/// each row's label.
#[sqlx::test]
async fn records_and_events_carry_their_team(pool: SqlitePool) {
    use orchestrator_core::ids::EventId;
    use orchestrator_server::sqlite::{approvals, events, tool_calls};

    let app = app(pool).await;
    let b = second_team(&app).await;
    let mut rx = app.store.subscribe();
    let mut asked = app.store.subscribe();
    let b_worker = agents::create(
        &app.pool,
        serde_json::from_value(json!({
            "team_id": b, "role": "specialist", "slug": "worker", "name": "w", "description": "d",
            "system_prompt": "p", "model": "grok-4.3", "icon": "i", "colour": "c",
            "tools": {"file_write": "ask", "memory_read": "allow"}, "delegate_targets": []
        }))
        .expect("input"),
    )
    .await
    .expect("worker")
    .spec
    .id();
    let a_root = app
        .engine
        .start(request(r#"test:call:memory_read:{"q":1}"#))
        .await
        .expect("a's task");
    let b_root = app
        .engine
        .start(StartRequest {
            team_id: Some(b),
            agent_id: Some(b_worker),
            ..request(r#"test:call:file_write:{"path":"b.md"}"#)
        })
        .await
        .expect("b's task");
    crate::wait_for(&mut asked, |e| {
        matches!(e.event, RunEvent::ApprovalRequested { .. }).then_some(())
    })
    .await;

    let pending_b = approvals::list(
        &app.pool,
        Some(orchestrator_core::runs::ApprovalStatus::Pending),
        Some(b),
    )
    .await
    .expect("list");
    assert_eq!(pending_b.approvals.len(), 1);
    assert_eq!(pending_b.approvals[0].team_id, b);
    let pending_a = approvals::list(
        &app.pool,
        Some(orchestrator_core::runs::ApprovalStatus::Pending),
        Some(app.team),
    )
    .await
    .expect("list");
    assert!(pending_a.approvals.is_empty());

    app.engine.cancel(b_root);
    let mut seen = Vec::new();
    let mut finished = 0;
    while finished < 2 {
        let envelope = tokio::time::timeout(crate::DEADLINE, rx.recv())
            .await
            .expect("in time")
            .expect("open");
        if matches!(envelope.event, RunEvent::RunFinished { .. })
            && (envelope.run_id == a_root || envelope.run_id == b_root)
        {
            finished += 1;
        }
        seen.push((envelope.id, envelope.root_run_id, envelope.team_id));
    }
    let team_of = |root| if root == b_root { b } else { app.team };
    assert!(seen.iter().all(|(_, root, team)| *team == team_of(*root)));
    assert!(seen.windows(2).all(|w| w[0].0 < w[1].0), "ids increase");
    let replayed = events::since(&app.pool, EventId(0)).await.expect("replay");
    assert!(replayed.iter().all(|e| e.team_id == team_of(e.root_run_id)));

    let calls = tool_calls::tool_calls(
        &app.pool,
        tool_calls::ToolCallFilter {
            team_id: Some(app.team),
            ..Default::default()
        },
        100,
    )
    .await
    .expect("calls");
    assert!(!calls.tool_calls.is_empty());
    assert!(calls.tool_calls.iter().all(|c| c.team_id == app.team));

    let base = serve(&app).await;
    for path in [
        "approvals",
        "tool-calls",
        "outbox",
        "memory",
        "runs",
        "agents",
    ] {
        let status = reqwest::Client::new()
            .get(format!("{base}/api/{path}?team_id=nope"))
            .send()
            .await
            .expect("get")
            .status();
        assert_eq!(status, 422, "{path}");
    }
}
