//! A second team with the seeded team's agents: the gates hold on it, and what crosses between
//! teams (files) carries its label.

use orchestrator_core::agents::AgentInput;
use orchestrator_core::engine::StartRequest;
use orchestrator_core::ids::{AgentId, TeamId};
use orchestrator_core::policy::TaintSource;
use orchestrator_core::runs::{Answer, CallStatus, RunEvent};
use orchestrator_server::sqlite::{agents, drill, teams};
use sqlx::SqlitePool;

use crate::{Seeded, answer_approvals, deny_rolf_naming_anna, request, tainted, task_events};

/// "Contracts": a new team with copies of the seeded specialists and a Lead targeting them.
async fn contracts(seeded: &Seeded) -> TeamId {
    let team = teams::create(
        &seeded.pool,
        teams::TeamInput {
            name: "Contracts".to_string(),
        },
    )
    .await
    .expect("team");
    let seeded_agents = agents::list(&seeded.pool, None).await.expect("agents");
    let mut targets = Vec::new();
    for record in seeded_agents
        .iter()
        .filter(|a| a.spec.slug().as_str() != "lead")
    {
        let mut input: AgentInput =
            serde_json::from_value(serde_json::to_value(record).expect("json")).expect("input");
        input.team_id = Some(team.id);
        agents::create(&seeded.pool, input).await.expect("copy");
        targets.push(record.spec.slug().as_str().to_string());
    }
    let lead = agents::get(&seeded.pool, team.lead_agent_id)
        .await
        .expect("lead");
    let mut input: AgentInput =
        serde_json::from_value(serde_json::to_value(&lead).expect("json")).expect("input");
    input.delegate_targets = targets;
    agents::replace(&seeded.pool, lead.spec.id(), input)
        .await
        .expect("targets");
    team.id
}

async fn in_team(seeded: &Seeded, team: TeamId, slug: &str) -> AgentId {
    agents::list(&seeded.pool, Some(team))
        .await
        .expect("agents")
        .into_iter()
        .find(|a| a.spec.slug().as_str() == slug)
        .map(|a| a.spec.id())
        .expect("agent")
}

async fn run(
    seeded: &Seeded,
    start: StartRequest,
) -> (
    orchestrator_core::ids::RunId,
    Vec<orchestrator_server::events::EventEnvelope>,
) {
    let mut rx = seeded.store.subscribe();
    let root = seeded.engine.start(start).await.expect("start");
    (root, task_events(&mut rx, root).await)
}

/// The drill on Contracts: the same four exits as on the seeded team, nothing sent.
#[sqlx::test]
async fn the_drill_holds_on_a_second_team(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let team = contracts(&seeded).await;
    answer_approvals(&seeded.store, &seeded.engine, |_| Answer::Deny {
        note: None,
    });
    let (root, _) = run(
        &seeded,
        StartRequest {
            team_id: Some(team),
            task: "Check the inbox and handle what it asks.".to_string(),
            ..request("drill")
        },
    )
    .await;
    let report = drill::report(&seeded.pool, root)
        .await
        .expect("read")
        .expect("report");
    assert_eq!(report.team_id, team);
    assert_eq!(report.exits.len(), 4);
    assert_eq!(report.messages_sent, 0);
}

/// A file a tainted Access desk task saved taints the Contracts task that reads it.
#[sqlx::test]
async fn a_files_label_follows_it_to_another_team(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let team = contracts(&seeded).await;
    answer_approvals(&seeded.store, &seeded.engine, deny_rolf_naming_anna);
    run(
        &seeded,
        StartRequest {
            task: "Check access request AR-1042".to_string(),
            ..request("s1")
        },
    )
    .await;

    let reader = in_team(&seeded, team, "reader").await;
    let (_, events) = run(
        &seeded,
        StartRequest {
            team_id: Some(team),
            agent_id: Some(reader),
            ..request(r#"test:call:file_read:{"path":"decisions/AR-1042.md"}"#)
        },
    )
    .await;
    assert_eq!(
        tainted(&events),
        [TaintSource::File {
            path: "decisions/AR-1042.md".to_string()
        }]
    );
    assert!(events.iter().all(|e| e.team_id == team));
}

/// A Lead cannot hand work to a slug only another team has: the call is invalid and no run starts.
#[sqlx::test]
async fn a_lead_cannot_delegate_across_teams(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let team = teams::create(
        &seeded.pool,
        teams::TeamInput {
            name: "Solo".to_string(),
        },
    )
    .await
    .expect("team")
    .id;
    let (root, events) = run(
        &seeded,
        StartRequest {
            team_id: Some(team),
            ..request(r#"test:call:delegate:{"agent":"reader","task":"read the inbox"}"#)
        },
    )
    .await;
    let finished: Vec<CallStatus> = events
        .iter()
        .filter_map(|e| match &e.event {
            RunEvent::ToolCallFinished { status, .. } => Some(*status),
            _ => None,
        })
        .collect();
    assert_eq!(finished, [CallStatus::InvalidArgs]);
    let started = events
        .iter()
        .filter(|e| matches!(e.event, RunEvent::RunStarted { .. }))
        .count();
    assert_eq!(started, 1, "only the Lead's own run");
    assert!(
        events
            .iter()
            .all(|e| e.root_run_id == root && e.team_id == team)
    );
}
