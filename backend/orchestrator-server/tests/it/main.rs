//! Integration tests: the real store, engine and scripted provider, with millisecond timings.
//! Each test gets a fresh migrated database from `#[sqlx::test]`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use orchestrator_core::agents::{AgentInput, Role};
pub use orchestrator_core::engine::Config;
use orchestrator_core::engine::{Engine, StartRequest};
use orchestrator_core::ids::AgentId;
use orchestrator_core::ids::RunId;
use orchestrator_core::llm::Model;
use orchestrator_core::money::Micros;
use orchestrator_core::policy::Decision;
use orchestrator_core::runs::RunEvent;
use orchestrator_core::tools::ToolName;
use orchestrator_server::events::EventEnvelope;
use orchestrator_server::llm::ScriptedProvider;
use orchestrator_server::sqlite::{SqliteStore, agents};
use sqlx::SqlitePool;
use tokio::sync::broadcast;

mod gateway;
mod runs;
mod tools;

/// Upper bound for any wait in a test; reaching it is a failure, not a delay.
const DEADLINE: Duration = Duration::from_secs(5);

pub struct App {
    pub pool: SqlitePool,
    pub store: Arc<SqliteStore>,
    pub engine: Arc<Engine>,
    /// A specialist holding every test tool.
    pub worker: AgentId,
}

pub fn test_config() -> Config {
    Config {
        llm_concurrency: 4,
        llm_timeout: DEADLINE,
        retry_backoff: vec![Duration::from_millis(5); 3],
        default_budget: Micros(500_000),
        approval_timeout: DEADLINE,
        allowlist: Vec::new(),
    }
}

pub async fn app(pool: SqlitePool) -> App {
    app_with(pool, test_config()).await
}

pub async fn app_with(pool: SqlitePool, config: Config) -> App {
    agents::create(&pool, lead()).await.expect("seed lead");
    let worker = agents::create(&pool, worker())
        .await
        .expect("seed worker")
        .spec
        .id();
    let store = Arc::new(SqliteStore::new(pool.clone()));
    let engine = Arc::new(Engine::new(
        store.clone(),
        Arc::new(ScriptedProvider::new()),
        tools::all(),
        config,
    ));
    App {
        pool,
        store,
        engine,
        worker,
    }
}

fn lead() -> AgentInput {
    AgentInput {
        role: Role::Lead,
        slug: "lead".to_string(),
        name: "Lead".to_string(),
        description: "plans".to_string(),
        system_prompt: "You lead.".to_string(),
        model: Model::Grok43,
        reasoning_effort: None,
        icon: "i".to_string(),
        colour: "c".to_string(),
        tools: BTreeMap::new(),
        delegate_targets: vec![],
    }
}

fn worker() -> AgentInput {
    let tools = BTreeMap::from([
        (ToolName::MemoryRead, Decision::Allow),
        (ToolName::FileWrite, Decision::Ask),
        (ToolName::CsvQuery, Decision::Allow),
        (ToolName::FileList, Decision::Allow),
        (ToolName::FileRead, Decision::Allow),
        (ToolName::SendMessage, Decision::Deny),
    ]);
    AgentInput {
        role: Role::Specialist,
        slug: "worker".to_string(),
        description: "uses every test tool".to_string(),
        tools,
        ..lead()
    }
}

pub fn request(scenario: &str) -> StartRequest {
    StartRequest {
        agent_id: None,
        task: "say hello".to_string(),
        title: None,
        budget: None,
        scenario: Some(scenario.to_string()),
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
