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
use orchestrator_core::llm::{LlmProvider, Model};
use orchestrator_core::money::Micros;
use orchestrator_core::policy::Decision;
use orchestrator_core::runs::RunEvent;
use orchestrator_core::tools::ToolName;
use orchestrator_server::events::EventEnvelope;
use orchestrator_server::llm::ScriptedProvider;
use orchestrator_server::sqlite::{SqliteStore, agents};
use sqlx::SqlitePool;
use tokio::sync::broadcast;

mod delegate;
mod files;
mod gateway;
mod memory;
mod messages;
mod recording;
mod runs;
mod taint;
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

/// Seeds a Lead that plans and delegates to `helper` and `helper2`, and the `worker`.
pub async fn app_with_llm(pool: SqlitePool, config: Config, llm: Arc<dyn LlmProvider>) -> App {
    for slug in ["helper", "helper2"] {
        agents::create(&pool, helper(slug))
            .await
            .expect("seed helper");
    }
    for agent in drill_team() {
        agents::create(&pool, agent).await.expect("seed drill team");
    }
    agents::create(&pool, lead()).await.expect("seed lead");
    let worker = agents::create(&pool, worker())
        .await
        .expect("seed worker")
        .spec
        .id();
    let store = Arc::new(SqliteStore::new(pool.clone()));
    let engine = Arc::new(Engine::new(store.clone(), llm, tools::all(), config));
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
        tools: BTreeMap::from([
            (ToolName::Delegate, Decision::Allow),
            (ToolName::Plan, Decision::Allow),
        ]),
        delegate_targets: ["helper", "helper2", "reader", "messenger"]
            .map(String::from)
            .to_vec(),
    }
}

/// The seed's split of the trifecta: Reader reads outside content but cannot send; Messenger
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

fn worker() -> AgentInput {
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
        slug: "worker".to_string(),
        description: "uses every test tool".to_string(),
        tools,
        delegate_targets: vec![],
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

/// A tool context for a seeded agent, with a real run row behind it.
pub async fn tool_ctx(app: &App, slug: &str) -> orchestrator_core::tools::ToolCtx {
    use orchestrator_core::store::{RunRef, Store};
    let agent = agents::list(&app.pool)
        .await
        .expect("agents")
        .into_iter()
        .find(|record| record.spec.slug().as_str() == slug)
        .map(|record| record.spec.id())
        .expect("seeded agent");
    let run = RunId::new();
    let at = RunRef {
        root_run_id: run,
        run_id: run,
    };
    let started = RunEvent::RunStarted {
        agent_id: agent,
        parent_run_id: None,
        task: "t".to_string(),
        title: None,
        budget: None,
    };
    app.store.emit(at, started).await.expect("run row");
    orchestrator_core::tools::ToolCtx {
        run_id: run,
        root_run_id: run,
        agent_id: agent,
        trust: orchestrator_core::policy::Trust::Trusted,
    }
}
