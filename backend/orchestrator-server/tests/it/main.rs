//! Integration tests: the real store, engine and scripted provider, with millisecond timings.
//! Each test gets a fresh migrated database from `#[sqlx::test]`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use orchestrator_core::agents::{AgentInput, Role};
pub use orchestrator_core::engine::Config;
use orchestrator_core::engine::{Engine, StartRequest};
use orchestrator_core::ids::RunId;
use orchestrator_core::ids::{AgentId, TeamId, ToolCallId};
use orchestrator_core::llm::{LlmProvider, Model};
use orchestrator_core::money::Micros;
use orchestrator_core::policy::Decision;
use orchestrator_core::policy::TaintSource;
use orchestrator_core::runs::{Answer, RunEvent, RunOutcome};
use orchestrator_core::tools::{Target, ToolName};
use orchestrator_server::api::{ActiveProvider, AppState, router};
use orchestrator_server::events::EventEnvelope;
use orchestrator_server::llm::ScriptedProvider;
use orchestrator_server::sqlite::{SqliteStore, agents};
use orchestrator_server::tools::workspace::Workspace;
use orchestrator_server::{seed, tools as real_tools};
use sqlx::SqlitePool;
use tokio::sync::broadcast;

mod delegate;
mod endpoints;
mod fetch;
mod files;
mod follow_up;
mod gateway;
mod memory;
mod messages;
mod recording;
mod recovery;
mod runs;
mod seeded;
mod taint;
mod team_runs;
mod teams;
mod teams_safety;
mod tools;
mod uploads;
mod xai;

/// Upper bound for any wait in a test; reaching it is a failure, not a delay.
pub const DEADLINE: Duration = Duration::from_secs(5);

pub struct App {
    pub pool: SqlitePool,
    pub store: Arc<SqliteStore>,
    pub engine: Arc<Engine>,
    /// The one team the harness seeds.
    pub team: TeamId,
    /// A specialist holding every test tool.
    pub specialist: AgentId,
}

pub fn test_config() -> Config {
    Config {
        llm_concurrency: 4,
        llm_timeout: DEADLINE,
        retry_backoff: vec![Duration::from_millis(5); 3],
        default_budget: Micros(500_000),
        approval_timeout: DEADLINE,
        allowlist: vec!["example.com".to_string()],
        max_depth: 2,
        max_runs: 12,
    }
}

pub async fn app(pool: SqlitePool) -> App {
    app_with(pool, test_config()).await
}

pub async fn app_with(pool: SqlitePool, config: Config) -> App {
    app_with_llm(pool, config, Arc::new(ScriptedProvider::new())).await
}

/// Seeds a Lead that plans and delegates to `helper` and `helper2`, and the `specialist`.
pub async fn app_with_llm(pool: SqlitePool, config: Config, llm: Arc<dyn LlmProvider>) -> App {
    let team = new_team(&pool, "Test team").await;
    for slug in ["helper", "helper2"] {
        agents::create(&pool, helper(slug))
            .await
            .expect("seed helper");
    }
    for agent in drill_team() {
        agents::create(&pool, agent).await.expect("seed drill team");
    }
    agents::create(&pool, lead()).await.expect("seed lead");
    let specialist = agents::create(&pool, specialist())
        .await
        .expect("seed specialist")
        .spec
        .id();
    let store = Arc::new(SqliteStore::new(pool.clone()));
    let engine = Arc::new(Engine::new(store.clone(), llm, tools::all(), config));
    App {
        pool,
        store,
        engine,
        team,
        specialist,
    }
}

/// Team "Contracts", a copy of the harness team: the same slugs, its own ids, and a Lead that
/// also targets `writer`, so every scripted scenario can run on it.
pub async fn second_team(app: &App) -> TeamId {
    let team = new_team(&app.pool, "Contracts").await;
    let on = |input: AgentInput| AgentInput {
        team_id: Some(team),
        ..input
    };
    for slug in ["helper", "helper2"] {
        agents::create(&app.pool, on(helper(slug)))
            .await
            .expect("second team helper");
    }
    for agent in drill_team() {
        agents::create(&app.pool, on(agent))
            .await
            .expect("second team drill agent");
    }
    let mut lead = on(lead());
    lead.delegate_targets.push("writer".to_string());
    agents::create(&app.pool, lead)
        .await
        .expect("second team lead");
    team
}

/// An empty team, committed on its own.
pub async fn new_team(pool: &SqlitePool, name: &str) -> TeamId {
    let mut conn = pool.acquire().await.expect("connection");
    orchestrator_server::sqlite::teams::insert(&mut conn, name)
        .await
        .expect("team")
}

fn lead() -> AgentInput {
    AgentInput {
        team_id: None,
        role: Role::Lead,
        slug: "lead".to_string(),
        name: "Lead".to_string(),
        description: "plans".to_string(),
        system_prompt: "You lead.".to_string(),
        model: Model::Grok43,
        reasoning_effort: None,
        icon: "i".to_string(),
        colour: "c".to_string(),
        tools: BTreeMap::from([
            (ToolName::Delegate, Decision::Allow),
            (ToolName::Plan, Decision::Allow),
            (ToolName::MemorySearch, Decision::Allow),
        ]),
        delegate_targets: ["helper", "helper2", "reader", "messenger"]
            .map(String::from)
            .to_vec(),
    }
}

/// The seed's roles: Reader reads outside content but cannot send; Messenger
/// sends but only with approval; Writer saves files without asking until the task is tainted.
fn drill_team() -> Vec<AgentInput> {
    let agent = |slug: &str, tools: &[(ToolName, Decision)]| AgentInput {
        tools: tools.iter().copied().collect(),
        ..helper(slug)
    };
    vec![
        agent(
            "reader",
            &[
                (ToolName::InboxList, Decision::Allow),
                (ToolName::InboxRead, Decision::Allow),
                (ToolName::HttpFetch, Decision::Allow),
                (ToolName::SendMessage, Decision::Deny),
            ],
        ),
        agent("messenger", &[(ToolName::SendMessage, Decision::Ask)]),
        agent(
            "writer",
            &[
                (ToolName::InboxRead, Decision::Allow),
                (ToolName::FileWrite, Decision::Allow),
                (ToolName::MemoryWrite, Decision::Allow),
            ],
        ),
    ]
}

fn helper(slug: &str) -> AgentInput {
    AgentInput {
        role: Role::Specialist,
        slug: slug.to_string(),
        name: slug.to_string(),
        description: format!("{slug} answers questions"),
        tools: BTreeMap::from([(ToolName::MemoryRead, Decision::Allow)]),
        delegate_targets: vec![],
        ..lead()
    }
}

fn specialist() -> AgentInput {
    let tools = BTreeMap::from([
        (ToolName::MemoryRead, Decision::Allow),
        (ToolName::FileWrite, Decision::Ask),
        (ToolName::CsvQuery, Decision::Allow),
        (ToolName::FileList, Decision::Allow),
        (ToolName::FileRead, Decision::Allow),
        (ToolName::SendMessage, Decision::Deny),
        (ToolName::InboxList, Decision::Allow),
        (ToolName::InboxRead, Decision::Allow),
        (ToolName::HttpFetch, Decision::Allow),
    ]);
    AgentInput {
        role: Role::Specialist,
        slug: "specialist".to_string(),
        description: "uses every test tool".to_string(),
        tools,
        delegate_targets: vec![],
        ..lead()
    }
}

/// Serves the API in scripted mode; the file view finds nothing, since the test tools have no
/// workspace.
pub async fn serve(app: &App) -> String {
    let workspace = Workspace::new(
        std::env::temp_dir().join("orchestrator-test-no-workspace"),
        app.pool.clone(),
    );
    serve_state(AppState {
        pool: app.pool.clone(),
        store: app.store.clone(),
        engine: app.engine.clone(),
        provider: ActiveProvider::Scripted,
        workspace,
    })
    .await
}

/// Serves `state` on a free loopback port and returns its base URL.
pub async fn serve_state(state: AppState) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    tokio::spawn(async move { axum::serve(listener, router(state)).await });
    base
}

pub fn request(team: TeamId, scenario: &str) -> StartRequest {
    StartRequest {
        team_id: team,
        agent_id: None,
        task: "say hello".to_string(),
        title: None,
        budget: None,
        scenario: Some(scenario.to_string()),
        draft_only: false,
    }
}

/// The events of one run, in broadcast order, up to and including its `run_finished`.
pub async fn events_until_finished(
    rx: &mut broadcast::Receiver<EventEnvelope>,
    run: RunId,
) -> Vec<RunEvent> {
    let mut events = Vec::new();
    loop {
        let envelope = tokio::time::timeout(DEADLINE, rx.recv())
            .await
            .expect("run finished before the deadline")
            .expect("broadcast open");
        if envelope.run_id != run {
            continue;
        }
        let finished = matches!(envelope.event, RunEvent::RunFinished { .. });
        events.push(envelope.event);
        if finished {
            return events;
        }
    }
}

/// Every event of one task (all its runs), up to and including the root's `run_finished`.
pub async fn task_events(
    rx: &mut broadcast::Receiver<EventEnvelope>,
    root: RunId,
) -> Vec<EventEnvelope> {
    let mut events = Vec::new();
    loop {
        let envelope = tokio::time::timeout(DEADLINE, rx.recv())
            .await
            .expect("task finished before the deadline")
            .expect("broadcast open");
        if envelope.root_run_id != root {
            continue;
        }
        let root_finished =
            envelope.run_id == root && matches!(envelope.event, RunEvent::RunFinished { .. });
        events.push(envelope);
        if root_finished {
            return events;
        }
    }
}

/// Every event of the task from now until the turn `run` finishes.
pub async fn turn_events(
    rx: &mut broadcast::Receiver<EventEnvelope>,
    root: RunId,
    run: RunId,
) -> Vec<RunEvent> {
    let mut events = Vec::new();
    loop {
        let envelope = tokio::time::timeout(DEADLINE, rx.recv())
            .await
            .expect("turn finished before the deadline")
            .expect("broadcast open");
        if envelope.root_run_id != root {
            continue;
        }
        let done = envelope.run_id == run && matches!(envelope.event, RunEvent::RunFinished { .. });
        events.push(envelope.event);
        if done {
            return events;
        }
    }
}

/// Waits for the first event `pick` accepts and returns what it picked.
pub async fn wait_for<T>(
    rx: &mut broadcast::Receiver<EventEnvelope>,
    mut pick: impl FnMut(EventEnvelope) -> Option<T>,
) -> T {
    loop {
        let envelope = tokio::time::timeout(DEADLINE, rx.recv())
            .await
            .expect("event before the deadline")
            .expect("broadcast open");
        if let Some(picked) = pick(envelope) {
            return picked;
        }
    }
}

/// Answers each approval as `decide` says from the card's target, as a person at the cards would.
pub fn answer_approvals(
    store: &SqliteStore,
    engine: &Arc<Engine>,
    decide: impl Fn(&Target) -> Answer + Send + 'static,
) {
    let mut rx = store.subscribe();
    let engine = Arc::clone(engine);
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

/// Denies the send to Rolf and approves everything else.
pub fn deny_rolf(target: &Target) -> Answer {
    match target {
        Target::Recipient { address, .. } if address.starts_with("rolf") => {
            Answer::Deny { note: None }
        }
        _ => Answer::Approve { note: None },
    }
}

/// As in the S1 demo: the send to Rolf is denied with a note naming the owner.
pub fn deny_rolf_naming_anna(target: &Target) -> Answer {
    match target {
        Target::Recipient { address, .. } if address.starts_with("rolf") => Answer::Deny {
            note: Some("wrong person, owner is Anna Weber".to_string()),
        },
        _ => Answer::Approve { note: None },
    }
}

pub fn tainted(events: &[EventEnvelope]) -> Vec<TaintSource> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            RunEvent::TaskTainted { source } => Some(source.clone()),
            _ => None,
        })
        .collect()
}

pub fn cards(events: &[EventEnvelope]) -> Vec<&RunEvent> {
    events
        .iter()
        .map(|e| &e.event)
        .filter(|e| matches!(e, RunEvent::ApprovalRequested { .. }))
        .collect()
}

/// The outcome in the task's last event, the root's `run_finished`.
pub fn root_outcome(events: &[EventEnvelope]) -> &RunOutcome {
    match &events.last().expect("events").event {
        RunEvent::RunFinished { outcome } => outcome,
        other => panic!("last event is {other:?}"),
    }
}

pub async fn agent_id(pool: &SqlitePool, slug: &str) -> AgentId {
    agents::list(pool, None)
        .await
        .expect("agents")
        .into_iter()
        .find(|record| record.spec.slug().as_str() == slug)
        .map(|record| record.spec.id())
        .expect("seeded agent")
}

/// The seeded team on the real tools in a fresh workspace.
pub struct Seeded {
    pub pool: SqlitePool,
    pub store: Arc<SqliteStore>,
    pub engine: Arc<Engine>,
    /// The seeded team.
    pub team: TeamId,
    pub temp: TempDir,
}

impl Seeded {
    pub async fn new(pool: SqlitePool) -> Self {
        Self::with_llm(pool, Arc::new(ScriptedProvider::new())).await
    }

    pub async fn with_llm(pool: SqlitePool, llm: Arc<dyn LlmProvider>) -> Self {
        let temp = TempDir::new();
        assert!(
            seed::run(&pool, &temp.workspace()).await.expect("seed"),
            "an empty database is seeded"
        );
        let store = Arc::new(SqliteStore::new(pool.clone()));
        let engine = engine_on(&pool, &store, &temp, llm);
        let mut conn = pool.acquire().await.expect("connection");
        let team = orchestrator_server::sqlite::teams::first_live(&mut conn)
            .await
            .expect("team")
            .expect("a seeded team");
        drop(conn);
        Self {
            pool,
            store,
            engine,
            team,
            temp,
        }
    }

    /// Another engine on the same store and workspace; it knows earlier tasks only from the log.
    pub fn engine_with(&self, llm: Arc<dyn LlmProvider>) -> Arc<Engine> {
        engine_on(&self.pool, &self.store, &self.temp, llm)
    }

    pub fn workspace(&self) -> Workspace {
        Workspace::new(self.temp.workspace(), self.pool.clone())
    }

    /// Serves the API in scripted mode over this workspace.
    pub async fn serve(&self) -> String {
        serve_state(AppState {
            pool: self.pool.clone(),
            store: self.store.clone(),
            engine: self.engine.clone(),
            provider: ActiveProvider::Scripted,
            workspace: self.workspace(),
        })
        .await
    }
}

fn engine_on(
    pool: &SqlitePool,
    store: &Arc<SqliteStore>,
    temp: &TempDir,
    llm: Arc<dyn LlmProvider>,
) -> Arc<Engine> {
    let tools = real_tools::all(pool.clone(), Workspace::new(temp.workspace(), pool.clone()));
    Arc::new(Engine::new(store.clone(), llm, tools, test_config()))
}

/// A tool context for a seeded agent, with a real run row behind it.
pub async fn tool_ctx(app: &App, slug: &str) -> orchestrator_core::tools::ToolCtx {
    tool_ctx_in(app, app.team, slug).await
}

/// A tool context for the agent with this slug on `team`, with a real run row behind it.
pub async fn tool_ctx_in(app: &App, team: TeamId, slug: &str) -> orchestrator_core::tools::ToolCtx {
    use orchestrator_core::store::{RunRef, Store};
    let (agent, role) = agents::list(&app.pool, Some(team))
        .await
        .expect("agents")
        .into_iter()
        .find(|record| record.spec.slug().as_str() == slug)
        .map(|record| (record.spec.id(), record.spec.role()))
        .expect("seeded agent");
    let run = RunId::new();
    let at = RunRef {
        root_run_id: run,
        run_id: run,
    };
    let started = RunEvent::RunStarted {
        agent_id: agent,
        team_id: team,
        parent_run_id: None,
        parent_call_id: None,
        task: "t".to_string(),
        task_sources: Vec::new(),
        title: None,
        budget: None,
        scenario: None,
        agent_version: None,
        draft_only: false,
    };
    app.store.emit(at, started).await.expect("run row");
    // A call row, so what the tool writes can name its call.
    let call_id = ToolCallId::new();
    let requested = RunEvent::ToolCallRequested {
        call_id,
        tool: "memory_read".to_string(),
        args: serde_json::json!({}),
        action: None,
        decision: None,
        reasons: Vec::new(),
    };
    app.store.emit(at, requested).await.expect("call row");
    orchestrator_core::tools::ToolCtx {
        run_id: run,
        root_run_id: run,
        call_id,
        agent_id: agent,
        team_id: team,
        role,
        trust: orchestrator_core::policy::Trust::Trusted,
    }
}

/// A fresh directory per test, removed when the guard drops.
pub struct TempDir(pub std::path::PathBuf);

impl TempDir {
    pub fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("orchestrator-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(dir.join("workspace")).expect("temp dir");
        Self(dir)
    }

    pub fn workspace(&self) -> std::path::PathBuf {
        self.0.join("workspace")
    }
}

impl Default for TempDir {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
