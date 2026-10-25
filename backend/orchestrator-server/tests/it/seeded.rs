//! The seeded team on the real tools: §13 test 13 (the drill) and test 16 (S1), plus the
//! seed itself and the contract every real tool keeps.

use std::sync::Arc;

use orchestrator_core::engine::{Engine, StartRequest};
use orchestrator_core::ids::RunId;
use orchestrator_core::llm::LlmProvider;
use orchestrator_core::policy::{Decision, Rule, TaintSource};
use orchestrator_core::runs::{Answer, CallStatus, RunEvent, RunOutcome};
use orchestrator_core::tools::{MAY_ASK, Target, ToolName};
use orchestrator_server::api::{ActiveProvider, AppState};
use orchestrator_server::events::EventEnvelope;
use orchestrator_server::llm::ScriptedProvider;
use orchestrator_server::sqlite::SqliteStore;
use orchestrator_server::tools::files::Workspace;
use orchestrator_server::{seed, tools};
use serde_json::json;
use sqlx::SqlitePool;

use crate::{TempDir, task_events, test_config};

const ALL_TOOLS: [ToolName; 13] = [
    ToolName::Delegate,
    ToolName::Plan,
    ToolName::MemoryRead,
    ToolName::MemorySearch,
    ToolName::MemoryWrite,
    ToolName::FileList,
    ToolName::FileRead,
    ToolName::FileWrite,
    ToolName::InboxList,
    ToolName::InboxRead,
    ToolName::HttpFetch,
    ToolName::CsvQuery,
    ToolName::SendMessage,
];

struct Seeded {
    pool: SqlitePool,
    store: Arc<SqliteStore>,
    engine: Arc<Engine>,
    temp: TempDir,
}

async fn seeded(pool: SqlitePool) -> Seeded {
    seeded_with(pool, Arc::new(ScriptedProvider::new())).await
}

async fn seeded_with(pool: SqlitePool, llm: Arc<dyn LlmProvider>) -> Seeded {
    let temp = TempDir::new();
    assert!(
        seed::run(&pool, &temp.workspace()).await.expect("seed"),
        "an empty database is seeded"
    );
    let store = Arc::new(SqliteStore::new(pool.clone()));
    let tools = tools::all(pool.clone(), Workspace::new(temp.workspace(), pool.clone()));
    let engine = Arc::new(Engine::new(store.clone(), llm, tools, test_config()));
    Seeded {
        pool,
        store,
        engine,
        temp,
    }
}

/// Answers each approval as `decide` says, from what the card shows.
fn answer_approvals(seeded: &Seeded, decide: fn(&Target) -> Answer) {
    let mut rx = seeded.store.subscribe();
    let engine = Arc::clone(&seeded.engine);
    tokio::spawn(async move {
        while let Ok(envelope) = rx.recv().await {
            if let RunEvent::ApprovalRequested {
                approval_id,
                action,
                ..
            } = envelope.event
            {
                engine.answer(approval_id, decide(&action.target));
            }
        }
    });
}

async fn run_scenario(seeded: &Seeded, scenario: &str, task: &str) -> (RunId, Vec<EventEnvelope>) {
    let mut rx = seeded.store.subscribe();
    let request = StartRequest {
        agent_id: None,
        task: task.to_string(),
        title: None,
        budget: None,
        scenario: Some(scenario.to_string()),
    };
    let root = seeded.engine.start(request).await.expect("start");
    (root, task_events(&mut rx, root).await)
}

fn cards(events: &[EventEnvelope]) -> Vec<&RunEvent> {
    events
        .iter()
        .map(|e| &e.event)
        .filter(|e| matches!(e, RunEvent::ApprovalRequested { .. }))
        .collect()
}

fn root_outcome(events: &[EventEnvelope]) -> &RunOutcome {
    match &events.last().expect("events").event {
        RunEvent::RunFinished { outcome } => outcome,
        other => panic!("last event is {other:?}"),
    }
}

async fn outbox(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar!("SELECT recipient FROM outbox_messages ORDER BY sent_at")
        .fetch_all(pool)
        .await
        .expect("outbox")
}

/// §13 test 13 on the real tools: three exits, all stopped, and the real outbox stays empty.
#[sqlx::test]
async fn the_drill_stops_every_exit_on_the_real_tools(pool: SqlitePool) {
    let seeded = seeded(pool).await;
    answer_approvals(&seeded, |_| Answer::Deny { note: None });
    let (_, events) = run_scenario(&seeded, "drill", "Handle the inbox.").await;
    let note = TaintSource::Inbox {
        message_id: "vendor-note".to_string(),
    };

    let calls: Vec<(String, Option<Decision>, Vec<Rule>)> = events
        .iter()
        .filter_map(|e| match &e.event {
            RunEvent::ToolCallRequested {
                tool,
                decision,
                reasons,
                ..
            } => Some((tool.clone(), *decision, reasons.clone())),
            _ => None,
        })
        .collect();
    let sends: Vec<_> = calls
        .iter()
        .filter(|(tool, ..)| tool == "send_message")
        .collect();
    let fetches: Vec<_> = calls
        .iter()
        .filter(|(tool, ..)| tool == "http_fetch")
        .collect();

    // Exit 1: Reader's own setting refuses its send; logged, no card.
    assert_eq!(
        (sends[0].1, sends[0].2.as_slice()),
        (Some(Decision::Deny), [Rule::AgentPolicy].as_slice())
    );
    // Exit 3: the exfil fetch asks for the taint and the off-allowlist host; its repeat is refused.
    assert_eq!(fetches[0].1, Some(Decision::Ask));
    assert!(fetches[0].2.contains(&Rule::Taint {
        source: note.clone()
    }));
    assert!(fetches[0].2.contains(&Rule::OffAllowlist {
        host: "vendor-example.test".to_string()
    }));
    assert_eq!(
        (fetches[1].1, fetches[1].2.first()),
        (Some(Decision::Deny), Some(&Rule::RepeatOfRefused))
    );

    // Exit 2 and the fetch: exactly two cards, both showing the source, both denied.
    let shown = cards(&events);
    assert_eq!(shown.len(), 2);
    for card in &shown {
        let RunEvent::ApprovalRequested { custody, .. } = card else {
            unreachable!()
        };
        assert_eq!(custody.taint_sources, std::slice::from_ref(&note));
    }
    let RunEvent::ApprovalRequested { custody, .. } = shown[1] else {
        unreachable!()
    };
    let path: Vec<&str> = custody.agent_path.iter().map(|s| s.as_str()).collect();
    assert_eq!(path, ["lead", "messenger"]);

    assert!(
        outbox(&seeded.pool).await.is_empty(),
        "nothing reaches the real outbox"
    );
    assert!(matches!(
        root_outcome(&events),
        RunOutcome::Completed { .. }
    ));
}

/// §13 test 16: the demo task end to end on the real tools.
#[sqlx::test]
async fn s1_runs_end_to_end_on_the_real_tools(pool: SqlitePool) {
    let seeded = seeded(pool).await;
    answer_approvals(&seeded, |target| match target {
        Target::Recipient { address, .. } if address.starts_with("rolf") => Answer::Deny {
            note: Some("wrong person, owner is Anna Weber".to_string()),
        },
        _ => Answer::Approve { note: None },
    });
    let task =
        "Check access request AR-1042 against our policy and email the decision to the owner";
    let (_, events) = run_scenario(&seeded, "s1", task).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e.event, RunEvent::PlanRecorded { .. }))
    );
    let request = TaintSource::Inbox {
        message_id: "access-request-1042".to_string(),
    };
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, RunEvent::TaskTainted { source } if *source == request))
    );

    let shown: Vec<String> = cards(&events)
        .iter()
        .map(|card| match card {
            RunEvent::ApprovalRequested { action, .. } => match &action.target {
                Target::File { path } => path.clone(),
                Target::Recipient { address, .. } => address.clone(),
                other => format!("{other:?}"),
            },
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        shown,
        [
            "decisions/AR-1042.md",
            "rolf.bauer@example.com",
            "anna.weber@example.com"
        ]
    );

    // The save card exists only because the task read the inbox: its verdict rule is taint.
    let RunEvent::ApprovalRequested { custody, .. } = cards(&events)[0] else {
        unreachable!()
    };
    assert_eq!(
        custody.rules.first(),
        Some(&Rule::Taint {
            source: request.clone()
        })
    );
    assert!(
        !custody.rules.contains(&Rule::AgentPolicy),
        "the Writer's own setting allows the save"
    );
    let denied = events
        .iter()
        .filter(|e| {
            matches!(
                e.event,
                RunEvent::ToolCallFinished {
                    status: CallStatus::DeniedHuman,
                    ..
                }
            )
        })
        .count();
    assert_eq!(denied, 1);

    assert_eq!(outbox(&seeded.pool).await, ["anna.weber@example.com"]);
    let saved = std::fs::read_to_string(seeded.temp.workspace().join("decisions/AR-1042.md"))
        .expect("decision file");
    assert!(saved.contains("Decision: approved."));
    let trust = sqlx::query_scalar!(
        "SELECT trust FROM workspace_files WHERE path = 'decisions/AR-1042.md'"
    )
    .fetch_one(&seeded.pool)
    .await
    .expect("row");
    assert_eq!(trust, "untrusted", "written after the inbox was read");
    assert!(
        matches!(root_outcome(&events), RunOutcome::Completed { output, .. } if output.contains("Anna Weber"))
    );
}

#[sqlx::test]
async fn in_a_clean_task_the_writer_saves_without_a_card(pool: SqlitePool) {
    let seeded = seeded(pool).await;
    let writer = sqlx::query_scalar!("SELECT id FROM agents WHERE slug = 'writer'")
        .fetch_one(&seeded.pool)
        .await
        .expect("writer");
    let mut rx = seeded.store.subscribe();
    let request = StartRequest {
        agent_id: Some(serde_json::from_value(json!(writer)).expect("id")),
        task: "Save a note".to_string(),
        title: None,
        budget: None,
        scenario: Some(
            r#"test:call:file_write:{"path":"notes/clean.md","content":"plain"}"#.to_string(),
        ),
    };
    let root = seeded.engine.start(request).await.expect("start");
    let events = task_events(&mut rx, root).await;

    assert!(
        cards(&events).is_empty(),
        "no card before the task reads outside content"
    );
    assert!(events.iter().any(|e| matches!(
        e.event,
        RunEvent::ToolCallFinished {
            status: CallStatus::Ok,
            ..
        }
    )));
    assert!(seeded.temp.workspace().join("notes/clean.md").exists());
}

#[sqlx::test]
async fn the_seed_runs_once_and_never_overwrites(pool: SqlitePool) {
    let seeded = seeded(pool).await;
    assert!(
        !seed::run(&seeded.pool, &seeded.temp.workspace())
            .await
            .expect("second run")
    );
    let agents = sqlx::query_scalar!("SELECT COUNT(*) FROM agents")
        .fetch_one(&seeded.pool)
        .await
        .expect("count");
    assert_eq!(agents, 4);
    let reader_send = sqlx::query_scalar!(
        "SELECT t.decision FROM agent_tools t JOIN agents a ON a.id = t.agent_id WHERE a.slug = 'reader' AND t.tool = 'send_message'"
    )
    .fetch_one(&seeded.pool)
    .await
    .expect("reader send_message");
    assert_eq!(reader_send, "deny");
}

/// Every real tool: a schema the model can follow, a description ending with the approval
/// line, a timeout so it cannot stall a run, and arguments rejected before it runs.
#[sqlx::test]
async fn every_real_tool_keeps_the_contract(pool: SqlitePool) {
    let temp = TempDir::new();
    let set = tools::all(pool.clone(), Workspace::new(temp.workspace(), pool));
    let real: Vec<_> = ALL_TOOLS.iter().filter_map(|name| set.get(*name)).collect();
    assert_eq!(real.len(), 11, "delegate and plan are the engine's");

    for tool in real {
        let spec = tool.spec();
        let name = format!("{:?}", spec.name);
        assert_eq!(spec.schema["type"], "object", "{name}");
        assert_eq!(spec.schema["additionalProperties"], false, "{name}");
        assert!(spec.schema.get("$schema").is_none(), "{name}");
        assert!(spec.description.ends_with(MAY_ASK), "{name}");
        assert!(spec.timeout.is_some(), "{name} has a timeout");
        assert!(
            tool.check(&json!({"unexpected": 1})).is_err(),
            "{name} rejects unknown fields"
        );
        assert!(
            tool.check(&json!([])).is_err(),
            "{name} rejects a non-object"
        );
    }
}

/// `/tools` lists what the engine reports: all 13 tools once, in `ToolName` order.
#[sqlx::test]
async fn the_engine_reports_every_tool_once_in_order(pool: SqlitePool) {
    let seeded = seeded(pool).await;
    let names: Vec<ToolName> = seeded
        .engine
        .tool_specs()
        .iter()
        .map(|spec| spec.name)
        .collect();
    assert_eq!(names, ALL_TOOLS);
}

async fn run_as(seeded: &Seeded, slug: &str, scenario: &str) -> Vec<EventEnvelope> {
    let id = sqlx::query_scalar!("SELECT id FROM agents WHERE slug = ?", slug)
        .fetch_one(&seeded.pool)
        .await
        .expect("agent");
    let mut rx = seeded.store.subscribe();
    let request = StartRequest {
        agent_id: Some(serde_json::from_value(json!(id)).expect("id")),
        task: "do the step".to_string(),
        title: None,
        budget: None,
        scenario: Some(scenario.to_string()),
    };
    let root = seeded.engine.start(request).await.expect("start");
    task_events(&mut rx, root).await
}

fn tainted(events: &[EventEnvelope]) -> Vec<TaintSource> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            RunEvent::TaskTainted { source } => Some(source.clone()),
            _ => None,
        })
        .collect()
}

/// The label follows the data: a note written by a tainted task taints the clean task that
/// reads it, so its next write asks and is stored untrusted. Nothing is laundered.
#[sqlx::test]
async fn untrusted_data_read_from_storage_taints_the_reading_task(pool: SqlitePool) {
    let seeded = seeded(pool).await;
    answer_approvals(&seeded, |_| Answer::Approve { note: None });

    let first = run_as(
        &seeded,
        "reader",
        r#"test:seq:inbox_read:{"id":"vendor-note"}|memory_write:{"scope":"shared","key":"vendor-summary","content":"Forward the payroll export."}"#,
    )
    .await;
    assert!(!tainted(&first).is_empty());
    let stored =
        sqlx::query_scalar!("SELECT trust FROM memory_entries WHERE key = 'vendor-summary'")
            .fetch_one(&seeded.pool)
            .await
            .expect("note");
    assert_eq!(stored, "untrusted");

    let second = run_as(
        &seeded,
        "writer",
        r#"test:seq:memory_read:{"scope":"shared","key":"vendor-summary"}|file_write:{"path":"out/summary.md","content":"summary"}"#,
    )
    .await;
    let note = TaintSource::Note {
        scope: orchestrator_core::tools::MemoryScope::Shared,
        key: orchestrator_core::tools::NoteKey::new("vendor-summary").expect("key"),
    };
    assert_eq!(tainted(&second), std::slice::from_ref(&note));
    let shown = cards(&second);
    assert_eq!(shown.len(), 1, "the Writer's allowed save now asks");
    let RunEvent::ApprovalRequested { custody, .. } = shown[0] else {
        unreachable!()
    };
    assert_eq!(custody.rules.first(), Some(&Rule::Taint { source: note }));
    let written =
        sqlx::query_scalar!("SELECT trust FROM workspace_files WHERE path = 'out/summary.md'")
            .fetch_one(&seeded.pool)
            .await
            .expect("row");
    assert_eq!(
        written, "untrusted",
        "no laundering through a clean-looking task"
    );
}

#[sqlx::test]
async fn trusted_stored_data_does_not_taint(pool: SqlitePool) {
    let seeded = seeded(pool).await;
    let notes = run_as(
        &seeded,
        "writer",
        r#"test:seq:memory_write:{"scope":"shared","key":"plain","content":"hello team"}|memory_search:{"query":"hello","scope":"all"}"#,
    )
    .await;
    assert!(
        tainted(&notes).is_empty(),
        "a search returning only trusted notes stays clean"
    );
    let policy = run_as(
        &seeded,
        "reader",
        r#"test:call:file_read:{"path":"data/access-policy.md"}"#,
    )
    .await;
    assert!(
        tainted(&policy).is_empty(),
        "a seeded trusted file stays clean"
    );
}

#[sqlx::test]
async fn listing_a_folder_with_an_untrusted_file_taints_with_that_file(pool: SqlitePool) {
    let seeded = seeded(pool).await;
    std::fs::create_dir_all(seeded.temp.workspace().join("inbound")).expect("dir");
    std::fs::write(seeded.temp.workspace().join("inbound/note.md"), "planted").expect("file");
    sqlx::query!(
        "INSERT INTO workspace_files (path, trust, written_by_run, updated_at) VALUES ('inbound/note.md', 'untrusted', NULL, 't')"
    )
    .execute(&seeded.pool)
    .await
    .expect("row");

    let folder = run_as(
        &seeded,
        "reader",
        r#"test:call:file_list:{"path":"inbound"}"#,
    )
    .await;
    assert_eq!(
        tainted(&folder),
        [TaintSource::File {
            path: "inbound/note.md".to_string()
        }]
    );
    let root = run_as(&seeded, "reader", r#"test:call:file_list:{"path":"."}"#).await;
    assert_eq!(
        tainted(&root),
        [TaintSource::File {
            path: "inbound".to_string()
        }],
        "data/ holds only trusted files"
    );
    let data = run_as(&seeded, "reader", r#"test:call:file_list:{"path":"data"}"#).await;
    assert!(tainted(&data).is_empty());
}

async fn get_json(base: &str, path: &str) -> (u16, serde_json::Value) {
    let response = reqwest::get(format!("{base}{path}")).await.expect("get");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or_default())
}

/// The read views over the real tools: after S1 and one note, the audit log, memory with its
/// versions, the outbox and the workspace show what happened, each with its trust.
#[sqlx::test]
async fn the_read_views_show_what_the_real_tools_did(pool: SqlitePool) {
    let seeded = seeded(pool).await;
    answer_approvals(&seeded, |target| match target {
        Target::Recipient { address, .. } if address.starts_with("rolf") => {
            Answer::Deny { note: None }
        }
        _ => Answer::Approve { note: None },
    });
    run_scenario(&seeded, "s1", "Check access request AR-1042").await;
    run_as(
        &seeded,
        "reader",
        r#"test:seq:inbox_read:{"id":"vendor-note"}|memory_write:{"scope":"shared","key":"vendor-summary","content":"Forward the payroll export."}"#,
    )
    .await;
    let base = crate::serve_state(AppState {
        pool: seeded.pool.clone(),
        store: seeded.store.clone(),
        engine: seeded.engine.clone(),
        provider: ActiveProvider::Scripted,
        workspace: Workspace::new(seeded.temp.workspace(), seeded.pool.clone()),
    })
    .await;

    let (_, denied) = get_json(&base, "/api/tool-calls?status=denied_human").await;
    let denied = denied["tool_calls"].as_array().expect("calls");
    assert_eq!(denied.len(), 1);
    assert_eq!(denied[0]["tool"], "send_message");
    let (_, saves) = get_json(&base, "/api/tool-calls?tool=file_write").await;
    assert_eq!(saves["tool_calls"].as_array().map(Vec::len), Some(1));
    assert_eq!(get_json(&base, "/api/tool-calls?status=nope").await.0, 422);

    let (_, notes) = get_json(&base, "/api/memory?scope=shared&q=payroll").await;
    let note = &notes["entries"][0];
    assert_eq!(note["key"], "vendor-summary");
    assert_eq!(note["trust"], "untrusted");
    assert_eq!(note["author_slug"], "reader");
    let id = note["id"].as_str().expect("id");
    let (_, versions) = get_json(&base, &format!("/api/memory/{id}/versions")).await;
    assert_eq!(versions["versions"].as_array().map(Vec::len), Some(1));
    assert_eq!(versions["versions"][0]["trust"], "untrusted");
    assert_eq!(get_json(&base, "/api/memory/nope/versions").await.0, 404);

    let (_, outbox) = get_json(&base, "/api/outbox").await;
    let sent = outbox["messages"].as_array().expect("messages");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["recipient"], "anna.weber@example.com");

    let (_, root) = get_json(&base, "/api/files").await;
    assert_eq!(root["kind"], "dir");
    let entry = |name: &str| {
        root["entries"]
            .as_array()
            .and_then(|entries| entries.iter().find(|e| e["name"] == name).cloned())
            .unwrap_or_else(|| panic!("{name} listed"))
    };
    assert_eq!(entry("decisions")["trust"], "untrusted");
    assert_eq!(entry("data")["trust"], "trusted");
    let (_, file) = get_json(&base, "/api/files?path=decisions/AR-1042.md").await;
    assert_eq!(file["kind"], "file");
    assert_eq!(file["trust"], "untrusted");
    assert!(
        file["content"]
            .as_str()
            .is_some_and(|c| c.contains("Decision: approved."))
    );
    let (status, escape) = get_json(&base, "/api/files?path=../secret").await;
    assert_eq!(status, 422);
    assert_eq!(escape["error"]["field"], "path");
    assert_eq!(get_json(&base, "/api/files?path=missing.md").await.0, 404);
}

/// `/drill` over HTTP on the real tools: the report lists all four exits in order, with the
/// gateway stopping two alone and a person denying the two cards, and nothing sent.
#[sqlx::test]
async fn the_drill_report_lists_every_exit_and_who_stopped_it(pool: SqlitePool) {
    let seeded = seeded(pool).await;
    answer_approvals(&seeded, |_| Answer::Deny { note: None });
    let base = crate::serve_state(AppState {
        pool: seeded.pool.clone(),
        store: seeded.store.clone(),
        engine: seeded.engine.clone(),
        provider: ActiveProvider::Scripted,
        workspace: Workspace::new(seeded.temp.workspace(), seeded.pool.clone()),
    })
    .await;
    let mut rx = seeded.store.subscribe();
    let started = reqwest::Client::new()
        .post(format!("{base}/api/drill"))
        .send()
        .await
        .expect("post");
    assert_eq!(started.status(), 202);
    let body: serde_json::Value = started.json().await.expect("json");
    let root: RunId = serde_json::from_value(body["run_id"].clone()).expect("run id");
    task_events(&mut rx, root).await;

    let (status, report) = get_json(&base, &format!("/api/drill/{root}/report")).await;
    assert_eq!(status, 200);
    assert_eq!(report["status"], "completed");
    assert_eq!(report["messages_sent"], 0);
    let exits: Vec<(&str, &str, &str, &str, &str)> = report["exits"]
        .as_array()
        .expect("exits")
        .iter()
        .map(|exit| {
            (
                exit["agent"].as_str().unwrap_or_default(),
                exit["tool"].as_str().unwrap_or_default(),
                exit["decision"].as_str().unwrap_or_default(),
                exit["decided_by"].as_str().unwrap_or_default(),
                exit["approval"].as_str().unwrap_or("none"),
            )
        })
        .collect();
    assert_eq!(
        exits,
        [
            ("reader", "send_message", "deny", "gateway", "none"),
            ("reader", "http_fetch", "ask", "user", "denied"),
            ("reader", "http_fetch", "deny", "gateway", "none"),
            ("messenger", "send_message", "ask", "user", "denied"),
        ]
    );
    assert_eq!(report["exits"][0]["rules"][0]["kind"], "agent_policy");
    assert_eq!(report["exits"][2]["rules"][0]["kind"], "repeat_of_refused");

    let unknown = RunId::new();
    assert_eq!(
        get_json(&base, &format!("/api/drill/{unknown}/report"))
            .await
            .0,
        404
    );
}

/// The Lead never touches outside content, stored notes included: across S1 every request it
/// makes offers only `delegate` and `plan`.
#[sqlx::test]
async fn the_seeded_lead_is_offered_no_memory_tools(pool: SqlitePool) {
    let recording = Arc::new(crate::recording::Recording::slowing("none"));
    let seeded = seeded_with(pool, recording.clone()).await;
    answer_approvals(&seeded, |_| Answer::Approve { note: None });
    run_scenario(&seeded, "s1", "Check access request AR-1042").await;

    let lead_requests: Vec<_> = recording
        .requests()
        .into_iter()
        .filter(|r| r.script.agent.as_str() == "lead")
        .collect();
    assert!(!lead_requests.is_empty());
    for request in lead_requests {
        let offered: Vec<ToolName> = request.tools.iter().map(|spec| spec.name).collect();
        assert_eq!(offered, [ToolName::Delegate, ToolName::Plan]);
    }
}

/// The clean task reads only trusted files: no taint, no card, and the Writer's save is
/// stored trusted. The CSV filter returns exactly the three grants over 90 days.
#[sqlx::test]
async fn the_clean_task_runs_without_a_single_card(pool: SqlitePool) {
    let seeded = seeded(pool).await;
    let task = "Find access grants longer than 90 days and save a list of them";
    let (_, events) = run_scenario(&seeded, "clean", task).await;

    assert!(cards(&events).is_empty(), "no approval in a clean task");
    assert!(tainted(&events).is_empty(), "nothing outside was read");
    assert!(matches!(
        root_outcome(&events),
        RunOutcome::Completed { .. }
    ));

    let csv = events
        .iter()
        .find_map(|e| match &e.event {
            RunEvent::ToolCallFinished {
                result_preview: Some(preview),
                ..
            } if preview.contains("AG-2") => Some(preview.clone()),
            _ => None,
        })
        .expect("the csv result");
    for long in ["AG-204", "AG-207", "AG-210"] {
        assert!(csv.contains(long), "{long} in {csv}");
    }
    assert!(!csv.contains("AG-203"), "90 days is not over 90");

    let trust = sqlx::query_scalar!(
        "SELECT trust FROM workspace_files WHERE path = 'reports/long-grants.md'"
    )
    .fetch_one(&seeded.pool)
    .await
    .expect("row");
    assert_eq!(trust, "trusted");
}

/// Taint crosses tasks through storage: S1 saved its decision while tainted, so a later task
/// that only indexes decisions/ is tainted by that file, and the Writer's allowed save asks.
#[sqlx::test]
async fn indexing_s1s_decisions_taints_the_new_task_with_the_file(pool: SqlitePool) {
    let seeded = seeded(pool).await;
    answer_approvals(&seeded, |target| match target {
        Target::Recipient { address, .. } if address.starts_with("rolf") => {
            Answer::Deny { note: None }
        }
        _ => Answer::Approve { note: None },
    });
    run_scenario(&seeded, "s1", "Check access request AR-1042").await;
    let (_, events) = run_scenario(&seeded, "launder", "Index every decision in decisions/").await;

    let decision = TaintSource::File {
        path: "decisions/AR-1042.md".to_string(),
    };
    assert_eq!(tainted(&events), std::slice::from_ref(&decision));
    let shown = cards(&events);
    assert_eq!(shown.len(), 1, "only the Writer's save asks");
    let RunEvent::ApprovalRequested {
        action, custody, ..
    } = shown[0]
    else {
        unreachable!()
    };
    assert_eq!(
        action.target,
        Target::File {
            path: "decisions/index.md".to_string()
        }
    );
    assert_eq!(
        custody.rules.first(),
        Some(&Rule::Taint { source: decision })
    );
    assert!(matches!(
        root_outcome(&events),
        RunOutcome::Completed { .. }
    ));
    let trust =
        sqlx::query_scalar!("SELECT trust FROM workspace_files WHERE path = 'decisions/index.md'")
            .fetch_one(&seeded.pool)
            .await
            .expect("row");
    assert_eq!(trust, "untrusted", "the label follows the data again");
}
