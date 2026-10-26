//! A second team with the seeded team's agents: the gates hold on it, files cross between teams
//! only by the user's grant, and what crosses carries its label.

use orchestrator_core::agents::AgentInput;
use orchestrator_core::engine::StartRequest;
use orchestrator_core::ids::{AgentId, TeamId};
use orchestrator_core::policy::{Rule, TaintSource, Trust};
use orchestrator_core::runs::{Answer, CallStatus, RunEvent};
use orchestrator_core::tools::Target;
use orchestrator_server::sqlite::teams::FileAccess;
use orchestrator_server::sqlite::{agents, drill, outbox, teams, tool_calls};
use sqlx::SqlitePool;

use crate::{
    Seeded, answer_approvals, cards, deny_rolf_naming_anna, id_text, request, tainted, task_events,
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

/// Contracts cannot open Access desk's folder until the user grants it; with a read grant, a
/// file a tainted Access desk task saved taints the Contracts task that reads it.
#[sqlx::test]
async fn another_teams_folder_opens_only_by_grant_and_its_labels_follow(pool: SqlitePool) {
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
    let read = |path: &str| StartRequest {
        agent_id: Some(reader),
        ..request(team, &format!(r#"test:call:file_read:{{"path":"{path}"}}"#))
    };
    let theirs = "teams/access-desk/decisions/AR-1042.md";

    let (_, events) = run(&seeded, read("decisions/AR-1042.md")).await;
    assert_eq!(
        finished(&events),
        [CallStatus::Failed],
        "not in its own folder"
    );
    let (_, events) = run(&seeded, read(theirs)).await;
    assert_eq!(finished(&events), [CallStatus::Refused]);
    assert_eq!(refused_by(&events), [Rule::NoAccess]);
    assert!(tainted(&events).is_empty() && cards(&events).is_empty());

    grant(&seeded, team, seeded.team, FileAccess::Read).await;
    let (_, events) = run(&seeded, read(theirs)).await;
    assert_eq!(finished(&events), [CallStatus::Ok]);
    assert_eq!(
        tainted(&events),
        [TaintSource::File {
            path: theirs.to_string()
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

/// Writing into Access desk's folder from Contracts: refused with no card, even from a tainted
/// task, until the grant is read and write; then a clean save runs and stays trusted, and a task
/// tainted by its own upload asks and stores it untrusted. `data/` is never writable.
#[sqlx::test]
async fn a_write_into_another_team_needs_read_write_and_keeps_the_taint_rule(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let team = contracts(&seeded).await;
    answer_approvals(&seeded.store, &seeded.engine, |_| Answer::Approve {
        note: None,
    });
    let own = "reports/shared.md";
    let theirs = "teams/access-desk/reports/shared.md";
    let writer = in_team(&seeded, seeded.team, "writer").await;
    run(
        &seeded,
        StartRequest {
            agent_id: Some(writer),
            ..request(
                seeded.team,
                &format!(r#"test:call:file_write:{{"path":"{own}","content":"first"}}"#),
            )
        },
    )
    .await;
    let physical = format!("teams/{}/{own}", id_text(seeded.team));
    assert_eq!(trust_of(&seeded, &physical).await, Trust::Trusted);

    let upload = seeded
        .workspace()
        .store_upload(team, "note.txt", "outside text")
        .await
        .expect("upload");
    let writer = in_team(&seeded, team, "writer").await;
    let write = |script: String| StartRequest {
        agent_id: Some(writer),
        ..request(team, &script)
    };
    let tainted_write = || {
        write(format!(
            r#"test:seq:file_read:{{"path":"{upload}"}}|file_write:{{"path":"{theirs}","content":"tainted"}}"#
        ))
    };
    let clean_write = || {
        write(format!(
            r#"test:call:file_write:{{"path":"{theirs}","content":"clean"}}"#
        ))
    };

    for access in [None, Some(FileAccess::Read)] {
        if let Some(access) = access {
            grant(&seeded, team, seeded.team, access).await;
        }
        let (_, events) = run(&seeded, tainted_write()).await;
        assert!(
            cards(&events).is_empty(),
            "{access:?}: refused before any card"
        );
        assert_eq!(refused_by(&events), [Rule::NoAccess], "{access:?}");
    }
    let (_, events) = run(
        &seeded,
        write(r#"test:call:file_write:{"path":"data/x.md","content":"x"}"#.to_string()),
    )
    .await;
    assert_eq!(refused_by(&events), [Rule::NoAccess], "data/ is read-only");
    assert_eq!(trust_of(&seeded, &physical).await, Trust::Trusted);

    grant(&seeded, team, seeded.team, FileAccess::ReadWrite).await;
    let (_, events) = run(&seeded, clean_write()).await;
    assert!(
        cards(&events).is_empty(),
        "a clean granted save asks nothing"
    );
    assert_eq!(trust_of(&seeded, &physical).await, Trust::Trusted);
    let (_, events) = run(&seeded, tainted_write()).await;
    assert_eq!(cards(&events).len(), 1, "the tainted save asks");
    assert_eq!(trust_of(&seeded, &physical).await, Trust::Untrusted);
}

/// An upload lands in its team's folder: it taints the task that reads it there, and another
/// team reaches it only through a grant, where it taints just the same.
#[sqlx::test]
async fn an_upload_stays_in_its_team_and_taints_through_a_grant(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let team = contracts(&seeded).await;
    let upload = seeded
        .workspace()
        .store_upload(seeded.team, "people.csv", "name,team\nAda,payroll\n")
        .await
        .expect("upload");
    assert_eq!(upload, "uploads/people.csv");
    let source = |path: &str| TaintSource::File {
        path: path.to_string(),
    };

    let events = read_as(&seeded, seeded.team, &upload).await;
    assert_eq!(tainted(&events), [source(&upload)]);
    let events = read_as(&seeded, team, &upload).await;
    assert_eq!(
        finished(&events),
        [CallStatus::Failed],
        "not in Contracts' folder"
    );
    assert!(tainted(&events).is_empty());

    grant(&seeded, team, seeded.team, FileAccess::Read).await;
    let theirs = format!("teams/access-desk/{upload}");
    let events = read_as(&seeded, team, &theirs).await;
    assert_eq!(tainted(&events), [source(&theirs)]);
    assert!(events.iter().all(|e| e.team_id == team));

    grant(&seeded, team, seeded.team, FileAccess::None).await;
    let events = read_as(&seeded, team, &theirs).await;
    assert_eq!(
        refused_by(&events),
        [Rule::NoAccess],
        "a revoke applies at once"
    );
}

async fn read_as(
    seeded: &Seeded,
    team: TeamId,
    path: &str,
) -> Vec<orchestrator_server::events::EventEnvelope> {
    let reader = in_team(seeded, team, "reader").await;
    let start = StartRequest {
        agent_id: Some(reader),
        ..request(team, &format!(r#"test:call:file_read:{{"path":"{path}"}}"#))
    };
    run(seeded, start).await.1
}

async fn grant(seeded: &Seeded, team: TeamId, of: TeamId, access: FileAccess) {
    teams::set_file_access(&seeded.pool, team, of, access)
        .await
        .expect("grant");
}

fn finished(events: &[orchestrator_server::events::EventEnvelope]) -> Vec<CallStatus> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            RunEvent::ToolCallFinished { status, .. } => Some(*status),
            _ => None,
        })
        .collect()
}

/// The rules of every refused call, in order.
fn refused_by(events: &[orchestrator_server::events::EventEnvelope]) -> Vec<Rule> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            RunEvent::ToolCallRequested {
                decision: Some(orchestrator_core::policy::Decision::Deny),
                reasons,
                ..
            } => reasons.first().cloned(),
            _ => None,
        })
        .collect()
}

/// By physical path.
async fn trust_of(seeded: &Seeded, path: &str) -> Trust {
    seeded.workspace().read(path).await.expect("read").1
}

/// A team's listing shows its own folder and `data`; `teams` appears only once a grant exists,
/// and lists only the granted teams.
#[sqlx::test]
async fn a_listing_shows_only_what_the_team_can_open(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let team = contracts(&seeded).await;
    let workspace = seeded.workspace();
    let names = |entries: Vec<orchestrator_server::tools::workspace::Entry>| {
        entries.into_iter().map(|e| e.path).collect::<Vec<_>>()
    };

    assert_eq!(
        names(workspace.list_for(team, ".").await.expect("list")),
        ["data"]
    );
    assert!(
        workspace
            .list_for(team, "teams")
            .await
            .expect("list")
            .is_empty()
    );
    grant(&seeded, team, seeded.team, FileAccess::Read).await;
    assert_eq!(
        names(workspace.list_for(team, ".").await.expect("list")),
        ["data", "teams"]
    );
    assert_eq!(
        names(workspace.list_for(team, "teams").await.expect("list")),
        ["teams/access-desk"]
    );
}

/// A grant check that cannot answer stops the call as a server failure, never as `no_access`:
/// no decision, no rule, no card, no taint.
#[sqlx::test]
async fn a_grant_check_that_fails_is_internal_not_a_refusal(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let team = contracts(&seeded).await;
    sqlx::raw_sql("DROP TABLE team_file_grants")
        .execute(&seeded.pool)
        .await
        .expect("drop");
    let events = read_as(&seeded, team, "teams/access-desk/data.md").await;
    let requested: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.event {
            RunEvent::ToolCallRequested {
                decision, reasons, ..
            } => Some((*decision, reasons.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(requested, [(None, Vec::new())]);
    assert_eq!(finished(&events), [CallStatus::Internal]);
    assert!(cards(&events).is_empty() && tainted(&events).is_empty());
}

/// `teams/<own slug>/...` is the team's own folder, not a second copy of it.
#[sqlx::test]
async fn the_own_slug_path_is_the_own_folder(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let writer = in_team(&seeded, seeded.team, "writer").await;
    run(
        &seeded,
        StartRequest {
            agent_id: Some(writer),
            ..request(
                seeded.team,
                r#"test:call:file_write:{"path":"teams/access-desk/reports/own.md","content":"mine"}"#,
            )
        },
    )
    .await;
    let events = read_as(&seeded, seeded.team, "reports/own.md").await;
    assert_eq!(finished(&events), [CallStatus::Ok]);
    assert_eq!(
        trust_of(&seeded, &seeded.own("reports/own.md")).await,
        Trust::Trusted
    );
}

/// A grant dies with its team: a new team that takes the deleted team's slug is not opened by it.
#[sqlx::test]
async fn a_reused_slug_is_not_opened_by_the_deleted_teams_grant(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let team = contracts(&seeded).await;
    let ops = |name: &str| teams::TeamInput {
        name: name.to_string(),
    };
    let old = teams::create(&seeded.pool, ops("Ops")).await.expect("team");
    grant(&seeded, team, old.id, FileAccess::Read).await;
    teams::delete(&seeded.pool, old.id, || false)
        .await
        .expect("delete");
    let new = teams::create(&seeded.pool, ops("Ops")).await.expect("team");
    assert_eq!(new.slug, old.slug);

    let events = read_as(&seeded, team, "teams/ops/notes.md").await;
    assert_eq!(refused_by(&events), [Rule::NoAccess]);
}

/// A grant revoked while the save's card waits: the approved call checks again and fails, and
/// the target keeps its bytes and label.
#[sqlx::test]
async fn a_revoke_while_the_card_waits_stops_the_approved_write(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let team = contracts(&seeded).await;
    let theirs = "teams/access-desk/reports/shared.md";
    let writer = in_team(&seeded, seeded.team, "writer").await;
    run(
        &seeded,
        StartRequest {
            agent_id: Some(writer),
            ..request(
                seeded.team,
                r#"test:call:file_write:{"path":"reports/shared.md","content":"first"}"#,
            )
        },
    )
    .await;
    grant(&seeded, team, seeded.team, FileAccess::ReadWrite).await;
    let upload = seeded
        .workspace()
        .store_upload(team, "note.txt", "outside text")
        .await
        .expect("upload");

    // A fresh engine whose cards are answered only after the grant is revoked.
    let engine = seeded.engine_with(std::sync::Arc::new(
        orchestrator_server::llm::ScriptedProvider::new(),
    ));
    let mut cards_rx = seeded.store.subscribe();
    let (pool, answering, of, me) = (seeded.pool.clone(), engine.clone(), seeded.team, team);
    tokio::spawn(async move {
        while let Ok(envelope) = cards_rx.recv().await {
            if let RunEvent::ApprovalRequested { approval_id, .. } = envelope.event {
                teams::set_file_access(&pool, me, of, FileAccess::None)
                    .await
                    .expect("revoke");
                answering.answer(approval_id, Answer::Approve { note: None });
            }
        }
    });
    let mut rx = seeded.store.subscribe();
    let root = engine
        .start(StartRequest {
            agent_id: Some(in_team(&seeded, team, "writer").await),
            ..request(
                team,
                &format!(
                    r#"test:seq:file_read:{{"path":"{upload}"}}|file_write:{{"path":"{theirs}","content":"tainted"}}"#
                ),
            )
        })
        .await
        .expect("start");
    let events = crate::task_events(&mut rx, root).await;
    assert_eq!(cards(&events).len(), 1, "the save asked while granted");
    assert_eq!(finished(&events), [CallStatus::Ok, CallStatus::Failed]);
    let physical = seeded.own("reports/shared.md");
    assert_eq!(trust_of(&seeded, &physical).await, Trust::Trusted);
    let bytes = std::fs::read_to_string(seeded.temp.workspace().join(&physical)).expect("file");
    assert_eq!(bytes, "first");
}
