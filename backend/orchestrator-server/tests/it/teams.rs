//! `/api/teams`: a team is born with its Lead, names are unique among live teams, and a
//! Lead-only team still runs.

use orchestrator_core::engine::StartRequest;
use orchestrator_core::ids::TeamId;
use orchestrator_core::runs::RunOutcome;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{app, events_until_finished, request, serve};

async fn send(client: &reqwest::RequestBuilder) -> (u16, Value) {
    let response = client
        .try_clone()
        .expect("clone")
        .send()
        .await
        .expect("send");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

#[sqlx::test]
async fn a_new_team_has_its_lead_and_a_unique_name(pool: SqlitePool) {
    let app = app(pool).await;
    let base = serve(&app).await;
    let client = reqwest::Client::new();
    let post = |name: Value| {
        client
            .post(format!("{base}/api/teams"))
            .json(&json!({"name": name}))
    };

    let (status, team) = send(&post(json!("Contracts"))).await;
    assert_eq!(status, 201);
    assert_eq!(team["agent_count"], 1);
    let (status, lead) = send(&client.get(format!(
        "{base}/api/agents/{}",
        team["lead_agent_id"].as_str().expect("lead")
    )))
    .await;
    assert_eq!(status, 200);
    assert_eq!(lead["team_id"], team["id"]);
    assert_eq!(lead["role"], "lead");
    assert_eq!(lead["slug"], "lead");
    assert_eq!(lead["delegate_targets"], json!([]));
    let tools: Vec<&String> = lead["tools"].as_object().expect("tools").keys().collect();
    assert_eq!(tools, ["delegate", "memory_read", "memory_search", "plan"]);

    for bad in [json!("  "), json!("x".repeat(61)), json!("contracts")] {
        let (status, body) = send(&post(bad.clone())).await;
        assert_eq!(
            (status, body["error"]["field"].clone()),
            (422, json!("name")),
            "{bad}"
        );
    }

    let (status, list) = send(&client.get(format!("{base}/api/teams"))).await;
    assert_eq!(status, 200);
    let names: Vec<&str> = list["teams"]
        .as_array()
        .expect("teams")
        .iter()
        .map(|t| t["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, ["Test team", "Contracts"], "oldest first");
    let (status, _) = send(&client.get(format!("{base}/api/teams?include_deleted=maybe"))).await;
    assert_eq!(status, 422);
}

#[sqlx::test]
async fn rename_and_reads_cover_live_and_deleted_teams(pool: SqlitePool) {
    let app = app(pool).await;
    let base = serve(&app).await;
    let client = reqwest::Client::new();
    let (_, team) = send(
        &client
            .post(format!("{base}/api/teams"))
            .json(&json!({"name": "A"})),
    )
    .await;
    let id = team["id"].as_str().expect("id").to_string();

    let (status, renamed) = send(
        &client
            .put(format!("{base}/api/teams/{id}"))
            .json(&json!({"name": "B"})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(renamed["name"], "B");
    assert!(renamed["updated_at"].as_str() > team["updated_at"].as_str());

    for path in [
        "nope".to_string(),
        json!(TeamId::new()).as_str().expect("id").to_string(),
    ] {
        let (status, _) = send(
            &client
                .put(format!("{base}/api/teams/{path}"))
                .json(&json!({"name": "C"})),
        )
        .await;
        assert_eq!(status, 404, "{path}");
    }

    sqlx::query!("UPDATE teams SET deleted_at = updated_at WHERE id = ?", id)
        .execute(&app.pool)
        .await
        .expect("delete by SQL");
    let (status, _) = send(
        &client
            .put(format!("{base}/api/teams/{id}"))
            .json(&json!({"name": "C"})),
    )
    .await;
    assert_eq!(status, 404, "a deleted team cannot be renamed");
    let (_, live) = send(&client.get(format!("{base}/api/teams"))).await;
    assert_eq!(live["teams"].as_array().map(Vec::len), Some(1));
    let (_, all) = send(&client.get(format!("{base}/api/teams?include_deleted=true"))).await;
    assert_eq!(all["teams"].as_array().map(Vec::len), Some(2));
}

#[sqlx::test]
async fn a_second_lead_on_a_team_is_refused(pool: SqlitePool) {
    let app = app(pool).await;
    let base = serve(&app).await;
    let client = reqwest::Client::new();
    let (_, team) = send(
        &client
            .post(format!("{base}/api/teams"))
            .json(&json!({"name": "Contracts"})),
    )
    .await;
    let (status, body) = send(&client.post(format!("{base}/api/agents")).json(&json!({
        "team_id": team["id"], "role": "lead", "slug": "boss", "name": "Boss", "description": "d",
        "system_prompt": "p", "model": "grok-4.3", "icon": "i", "colour": "c", "tools": {},
        "delegate_targets": []
    })))
    .await;
    assert_eq!(status, 422);
    assert_eq!(body["error"]["field"], "role");
    assert_eq!(body["error"]["message"], "the team already has a Lead");
}

/// A Lead with no teammates is offered `delegate` with an empty choice and still finishes.
#[sqlx::test]
async fn a_lead_only_team_runs_a_task(pool: SqlitePool) {
    let app = app(pool).await;
    let team = orchestrator_server::sqlite::teams::create(
        &app.pool,
        orchestrator_server::sqlite::teams::TeamInput {
            name: "Solo".to_string(),
        },
    )
    .await
    .expect("team")
    .id;
    let mut rx = app.store.subscribe();
    let run = app
        .engine
        .start(request(team, "test:delegate"))
        .await
        .expect("start");
    let events = events_until_finished(&mut rx, run).await;
    assert!(matches!(
        events.last(),
        Some(orchestrator_core::runs::RunEvent::RunFinished {
            outcome: RunOutcome::Completed { .. }
        })
    ));
}

/// A team with a live task is kept, the last team is kept, and a deleted team's history stays.
#[sqlx::test]
async fn delete_keeps_busy_and_last_teams_and_their_history(pool: SqlitePool) {
    use orchestrator_core::runs::RunEvent;

    let app = app(pool).await;
    let b = crate::second_team(&app).await;
    let b_text = json!(b).as_str().expect("id").to_string();
    let base = serve(&app).await;
    let client = reqwest::Client::new();
    let delete = |id: &str| client.delete(format!("{base}/api/teams/{id}"));

    // A specialist on team B whose file save waits on a card.
    let specialist = orchestrator_server::sqlite::agents::create(
        &app.pool,
        serde_json::from_value(json!({
            "team_id": b, "role": "specialist", "slug": "specialist", "name": "w", "description": "d",
            "system_prompt": "p", "model": "grok-4.3", "icon": "i", "colour": "c",
            "tools": {"file_write": "ask"}, "delegate_targets": []
        }))
        .expect("input"),
    )
    .await
    .expect("specialist")
    .spec
    .id();
    let mut rx = app.store.subscribe();
    let root = app
        .engine
        .start(StartRequest {
            agent_id: Some(specialist),
            ..request(b, r#"test:call:file_write:{"path":"a.md"}"#)
        })
        .await
        .expect("start");
    crate::wait_for(&mut rx, |e| {
        matches!(e.event, RunEvent::ApprovalRequested { .. }).then_some(())
    })
    .await;
    let (status, body) = send(&delete(&b_text)).await;
    assert_eq!(status, 409);
    assert_eq!(body["error"]["message"], "this team has a task running");

    assert!(app.engine.cancel(root));
    events_until_finished(&mut rx, root).await;
    // The task leaves the engine's map once its watcher sees the run end.
    for _ in 0..50 {
        if !app.engine.has_live_task(b) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let (status, _) = send(&delete(&b_text)).await;
    assert_eq!(status, 204);
    let (status, _) = send(&delete(&b_text)).await;
    assert_eq!(status, 404, "already deleted");

    let (_, view) = send(&client.get(format!("{base}/api/teams/{b_text}"))).await;
    assert!(view["deleted_at"].is_string());
    assert_eq!(view["agent_count"], 0);
    let (_, runs) = send(&client.get(format!("{base}/api/runs?team_id={b_text}"))).await;
    assert_eq!(
        runs["runs"].as_array().map(Vec::len),
        Some(1),
        "history stays"
    );
    let (status, _) = send(
        &client
            .post(format!("{base}/api/teams"))
            .json(&json!({"name": "Contracts"})),
    )
    .await;
    assert_eq!(status, 201, "the name is free again");

    let (status, body) = send(
        &client
            .post(format!("{base}/api/runs"))
            .json(&json!({"task": "t", "team_id": b_text})),
    )
    .await;
    assert_eq!(
        (status, body["error"]["field"].clone()),
        (422, json!("team_id"))
    );

    let (_, list) = send(&client.get(format!("{base}/api/teams"))).await;
    let ids: Vec<String> = list["teams"]
        .as_array()
        .expect("teams")
        .iter()
        .map(|t| t["id"].as_str().expect("id").to_string())
        .collect();
    assert_eq!(ids.len(), 2);
    let (status, _) = send(&delete(&ids[1])).await;
    assert_eq!(status, 204);
    let (status, body) = send(&delete(&ids[0])).await;
    assert_eq!(status, 409);
    assert_eq!(body["error"]["message"], "keep at least one team");

    let lead = orchestrator_server::sqlite::agents::list(&app.pool, Some(app.team))
        .await
        .expect("agents")
        .into_iter()
        .find(|a| a.spec.slug().as_str() == "lead")
        .expect("lead");
    let (status, _) = send(&client.delete(format!(
        "{base}/api/agents/{}",
        json!(lead.spec.id()).as_str().expect("id")
    )))
    .await;
    assert_eq!(status, 422, "a live team's Lead stays");
}

/// A slug is derived once from the name, gets `-2` on a live clash, survives a rename and is
/// free again once its team is deleted; the delete also drops every file grant naming the team.
#[sqlx::test]
async fn a_team_slug_is_fixed_at_create_and_its_grants_go_with_it(pool: SqlitePool) {
    let app = app(pool.clone()).await;
    let base = serve(&app).await;
    let client = reqwest::Client::new();
    let create = |name: &str| {
        client
            .post(format!("{base}/api/teams"))
            .json(&json!({"name": name}))
    };

    let (_, ops) = send(&create("Ops desk")).await;
    assert_eq!(ops["slug"], "ops-desk");
    let (_, second) = send(&create("Ops-desk!")).await;
    assert_eq!(second["slug"], "ops-desk-2");
    let ops_id = ops["id"].as_str().expect("id").to_string();
    let (status, renamed) = send(
        &client
            .put(format!("{base}/api/teams/{ops_id}"))
            .json(&json!({"name": "Field ops"})),
    )
    .await;
    assert_eq!((status, renamed["slug"].clone()), (200, json!("ops-desk")));

    let second_id = second["id"].as_str().expect("id").to_string();
    sqlx::query!(
        "INSERT INTO team_file_grants (team_id, of_team_id, access, updated_at)
         VALUES (?1, ?2, 'read', 'now'), (?2, ?1, 'read_write', 'now')",
        ops_id,
        second_id
    )
    .execute(&pool)
    .await
    .expect("grants");
    let (status, _) = send(&client.delete(format!("{base}/api/teams/{ops_id}"))).await;
    assert_eq!(status, 204);
    let left = sqlx::query_scalar!("SELECT COUNT(*) FROM team_file_grants")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(left, 0);

    let (_, again) = send(&create("Ops desk")).await;
    assert_eq!(again["slug"], "ops-desk");
}

/// `/teams/{id}/file-access`: every other live team starts at `none`; a grant is set, changed
/// and cleared; the team itself, an unknown team and a bad value are refused.
#[sqlx::test]
async fn file_access_lists_other_teams_and_sets_each_grant(pool: SqlitePool) {
    let app = app(pool).await;
    let base = serve(&app).await;
    let client = reqwest::Client::new();
    let (_, contracts) = send(
        &client
            .post(format!("{base}/api/teams"))
            .json(&json!({"name": "Contracts"})),
    )
    .await;
    let own = crate::id_text(app.team);
    let other = contracts["id"].as_str().expect("id").to_string();
    let access = |of: &str, value: Value| {
        client
            .put(format!("{base}/api/teams/{own}/file-access/{of}"))
            .json(&json!({"access": value}))
    };
    let listed = || async {
        send(&client.get(format!("{base}/api/teams/{own}/file-access")))
            .await
            .1["teams"]
            .clone()
    };

    assert_eq!(
        listed().await,
        json!([{"team_id": other, "name": "Contracts", "slug": "contracts", "access": "none"}])
    );
    for value in ["read", "read_write", "none"] {
        assert_eq!(send(&access(&other, json!(value))).await.0, 204);
        assert_eq!(listed().await[0]["access"], value);
    }
    assert_eq!(send(&access(&own, json!("read"))).await.0, 422);
    assert_eq!(
        send(&access(&crate::id_text(TeamId::new()), json!("read")))
            .await
            .0,
        404
    );
    assert_eq!(send(&access(&other, json!("write"))).await.0, 422);
}

/// A Lead with no teammate is never offered `delegate`: its agent list would be empty, which a
/// provider rejects, so the task would fail before any answer.
#[sqlx::test]
async fn a_lead_without_teammates_is_not_offered_delegate(pool: SqlitePool) {
    let recording = std::sync::Arc::new(crate::recording::Recording::new());
    let app = crate::app_with_llm(pool, crate::test_config(), recording.clone()).await;
    let team = orchestrator_server::sqlite::teams::create(
        &app.pool,
        orchestrator_server::sqlite::teams::TeamInput {
            name: "Solo".to_string(),
        },
    )
    .await
    .expect("team")
    .id;
    let mut rx = app.store.subscribe();
    let run = app
        .engine
        .start(request(team, "Summarise the week."))
        .await
        .expect("start");
    events_until_finished(&mut rx, run).await;
    let offered: Vec<_> = recording
        .requests()
        .into_iter()
        .flat_map(|r| r.tools)
        .map(|spec| spec.name)
        .collect();
    assert!(!offered.is_empty(), "the Lead was asked");
    assert!(
        !offered.contains(&orchestrator_core::tools::ToolName::Delegate),
        "{offered:?}"
    );
}
