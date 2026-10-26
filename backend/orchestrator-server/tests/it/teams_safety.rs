//! A second team with the seeded team's agents: the gates hold on it, and what crosses between
//! teams (files, uploads) carries its label.

use orchestrator_core::agents::AgentInput;
use orchestrator_core::engine::StartRequest;
use orchestrator_core::ids::{AgentId, TeamId};
use orchestrator_core::policy::{Rule, TaintSource, Trust};
use orchestrator_core::runs::{Answer, CallStatus, RunEvent};
use orchestrator_core::tools::Target;
use orchestrator_server::sqlite::{agents, drill, outbox, teams, tool_calls};
use sqlx::SqlitePool;

use crate::{
    Seeded, answer_approvals, cards, deny_rolf_naming_anna, request, tainted, task_events,
};

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
            task: "Check the inbox and handle what it asks.".to_string(),
            ..request(team, "drill")
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
            ..request(seeded.team, "s1")
        },
    )
    .await;

    let reader = in_team(&seeded, team, "reader").await;
    let (_, events) = run(
        &seeded,
        StartRequest {
            agent_id: Some(reader),
            ..request(
                team,
                r#"test:call:file_read:{"path":"decisions/AR-1042.md"}"#,
            )
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
        request(
            team,
            r#"test:call:delegate:{"agent":"reader","task":"read the inbox"}"#,
        ),
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

/// S1 on Contracts: the save card names taint, Rolf and Anna each ask, one email goes out, and
/// every outbox row and tool call carries Contracts' id.
#[sqlx::test]
async fn s1_holds_on_a_second_team(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let team = contracts(&seeded).await;
    answer_approvals(&seeded.store, &seeded.engine, deny_rolf_naming_anna);
    let (root, events) = run(
        &seeded,
        StartRequest {
            task: "Check access request AR-1042".to_string(),
            ..request(team, "s1")
        },
    )
    .await;

    let shown = cards(&events);
    assert_eq!(shown.len(), 3);
    let RunEvent::ApprovalRequested { custody, .. } = shown[0] else {
        unreachable!()
    };
    assert!(matches!(custody.rules.first(), Some(Rule::Taint { .. })));
    let asked: Vec<&str> = shown[1..]
        .iter()
        .map(|card| match card {
            RunEvent::ApprovalRequested { action, .. } => match &action.target {
                Target::Recipient { address, .. } => address.as_str(),
                other => panic!("{other:?}"),
            },
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(asked, ["rolf.bauer@example.com", "anna.weber@example.com"]);

    let sent = outbox::list(&seeded.pool, outbox::OutboxFilter::default())
        .await
        .expect("outbox")
        .messages;
    assert_eq!(sent.len(), 1);
    assert!(sent.iter().all(|m| m.team_id == team));
    let calls = tool_calls::tool_calls(&seeded.pool, tool_calls::ToolCallFilter::default(), 200)
        .await
        .expect("calls")
        .tool_calls;
    assert!(calls.iter().any(|c| c.root_run_id == root));
    assert!(calls.iter().all(|c| c.team_id == team));
}

/// Overwriting a trusted file from Contracts: a clean task saves without a card and keeps it
/// trusted; a task tainted by an upload asks and stores it untrusted.
#[sqlx::test]
async fn a_trusted_file_keeps_its_rules_on_another_team(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let team = contracts(&seeded).await;
    answer_approvals(&seeded.store, &seeded.engine, |_| Answer::Approve {
        note: None,
    });
    let path = "reports/shared.md";
    let writer = in_team(&seeded, seeded.team, "writer").await;
    run(
        &seeded,
        StartRequest {
            agent_id: Some(writer),
            ..request(
                seeded.team,
                &format!(r#"test:call:file_write:{{"path":"{path}","content":"first"}}"#),
            )
        },
    )
    .await;
    assert_eq!(trust_of(&seeded, path).await, Trust::Trusted);

    let writer = in_team(&seeded, team, "writer").await;
    let (_, events) = run(
        &seeded,
        StartRequest {
            agent_id: Some(writer),
            ..request(
                team,
                &format!(r#"test:call:file_write:{{"path":"{path}","content":"clean"}}"#),
            )
        },
    )
    .await;
    assert!(cards(&events).is_empty(), "a clean overwrite asks nothing");
    assert_eq!(trust_of(&seeded, path).await, Trust::Trusted);

    let upload = seeded
        .workspace()
        .store_upload("note.txt", "outside text")
        .await
        .expect("upload");
    let (_, events) = run(
        &seeded,
        StartRequest {
            agent_id: Some(writer),
            ..request(
                team,
                &format!(
                    r#"test:seq:file_read:{{"path":"{upload}"}}|file_write:{{"path":"{path}","content":"tainted"}}"#
                ),
            )
        },
    )
    .await;
    assert_eq!(cards(&events).len(), 1, "the tainted overwrite asks");
    assert_eq!(trust_of(&seeded, path).await, Trust::Untrusted);
}

/// An upload taints the task that reads it on either team.
#[sqlx::test]
async fn an_upload_taints_a_task_on_either_team(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let team = contracts(&seeded).await;
    let upload = seeded
        .workspace()
        .store_upload("people.csv", "name,team\nAda,payroll\n")
        .await
        .expect("upload");
    let source = TaintSource::File {
        path: upload.clone(),
    };
    for on in [seeded.team, team] {
        let reader = in_team(&seeded, on, "reader").await;
        let (_, events) = run(
            &seeded,
            StartRequest {
                agent_id: Some(reader),
                ..request(on, &format!(r#"test:call:file_read:{{"path":"{upload}"}}"#))
            },
        )
        .await;
        assert_eq!(tainted(&events), std::slice::from_ref(&source));
        assert!(events.iter().all(|e| e.team_id == on));
    }
}

async fn trust_of(seeded: &Seeded, path: &str) -> Trust {
    seeded.workspace().read(path).await.expect("read").1
}
