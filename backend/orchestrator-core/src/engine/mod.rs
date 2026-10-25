//! The engine is the API's only way to change runs: it starts a task, continues a finished one
//! with a follow-up, and cancels it.
//! Each run is its own spawned task, and a watcher awaits it so a panic still ends the run.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::Semaphore;

use crate::agents::{AgentSpec, Role};
use crate::ids::{AgentId, ApprovalId, RunId};
use crate::llm::{Item, LlmProvider};
use crate::money::Micros;
use crate::runs::{Answer, FailReason, RunEvent, RunOutcome};
use crate::store::{RunRef, Store, StoreError};
use crate::tools::{ToolSet, ToolSpec};

mod approvals;
mod envelope;
mod gateway;
mod history;
mod native;
mod prompts;
mod run;
mod task;

pub use native::{delegate_spec, plan_spec};
pub use task::Task;

/// Limits and timings; tests build their own with millisecond values.
#[derive(Debug, Clone)]
pub struct Config {
    pub llm_concurrency: usize,
    /// The longest one model call may take; reaching it is a retryable timeout.
    pub llm_timeout: Duration,
    /// The base delay of each retry of a retryable model error; its length is the retry count.
    /// Each wait is drawn uniformly from half its base to its base (equal jitter).
    pub retry_backoff: Vec<Duration>,
    pub default_budget: Micros,
    /// How long an approval waits for a human before it expires.
    pub approval_timeout: Duration,
    /// Hosts fetchable without asking; each entry also covers its subdomains.
    pub allowlist: Vec<String>,
    /// How many delegations deep a run may be; a root run is at depth 0.
    pub max_depth: u32,
    /// Runs one turn of a task may start, its top-level run included.
    pub max_runs: u32,
}

#[derive(Debug, Clone)]
pub struct StartRequest {
    /// The root agent; the Lead when absent.
    pub agent_id: Option<AgentId>,
    pub task: String,
    pub title: Option<String>,
    pub budget: Option<Micros>,
    pub scenario: Option<String>,
    /// Refuses every save, note, fetch and send in this turn, with no card.
    pub draft_only: bool,
}

/// The next turn of a finished task, for its Lead.
#[derive(Debug, Clone)]
pub struct FollowUpRequest {
    pub text: String,
    pub scenario: Option<String>,
    /// Refuses every save, note, fetch and send in this turn, with no card.
    pub draft_only: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum FollowUpError {
    #[error("the text is empty")]
    EmptyText,
    #[error("no task has this root run")]
    UnknownTask,
    #[error("the task is still running")]
    Running,
    #[error("the task's root agent is not the team's Lead")]
    NotLead,
    #[error(transparent)]
    Store(#[from] StoreError),
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("the team has no Lead")]
    NoLead,
    #[error("no live agent has this id")]
    UnknownAgent,
    #[error("the task text is empty")]
    EmptyTask,
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub struct Engine {
    inner: Arc<Inner>,
}

struct Inner {
    store: Arc<dyn Store>,
    llm: Arc<dyn LlmProvider>,
    tools: ToolSet,
    config: Config,
    approvals: approvals::ApprovalWaiter,
    /// Caps model calls in flight across all runs.
    llm_slots: Semaphore,
    /// Live tasks by root run id; an entry leaves when its top-level run has finished.
    tasks: Mutex<HashMap<RunId, Arc<Task>>>,
    /// Held from reading a task's log until its follow-up's `run_started` is recorded, so two
    /// follow-ups cannot both find the task finished.
    follow_ups: tokio::sync::Mutex<()>,
}

impl Engine {
    pub fn new(
        store: Arc<dyn Store>,
        llm: Arc<dyn LlmProvider>,
        tools: ToolSet,
        config: Config,
    ) -> Self {
        let llm_slots = Semaphore::new(config.llm_concurrency);
        Self {
            inner: Arc::new(Inner {
                store,
                llm,
                tools,
                config,
                approvals: approvals::ApprovalWaiter::default(),
                llm_slots,
                tasks: Mutex::new(HashMap::new()),
                follow_ups: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// Emits `run_started` before spawning, so a storage failure is a start error, not a lost run.
    pub async fn start(&self, request: StartRequest) -> Result<RunId, StartError> {
        let task_text = request.task.trim().to_string();
        if task_text.is_empty() {
            return Err(StartError::EmptyTask);
        }
        let team: Vec<Arc<AgentSpec>> = self
            .inner
            .store
            .team()
            .await?
            .into_iter()
            .map(Arc::new)
            .collect();
        let agent = match request.agent_id {
            Some(id) => team
                .iter()
                .find(|agent| agent.id() == id)
                .ok_or(StartError::UnknownAgent)?,
            None => team
                .iter()
                .find(|agent| agent.role() == Role::Lead)
                .ok_or(StartError::NoLead)?,
        };
        let agent = Arc::clone(agent);

        let root = RunId::new();
        let at = RunRef {
            root_run_id: root,
            run_id: root,
        };
        let budget = request.budget.unwrap_or(self.inner.config.default_budget);
        let task = Arc::new(Task::new(
            budget,
            self.inner.config.allowlist.clone(),
            team,
            request.draft_only,
        ));
        self.inner
            .emit(
                at,
                RunEvent::RunStarted {
                    agent_id: agent.id(),
                    parent_run_id: None,
                    parent_call_id: None,
                    task: task_text.clone(),
                    task_sources: Vec::new(),
                    title: request.title,
                    budget: Some(budget),
                    scenario: request.scenario.clone(),
                    agent_version: Some(agent.version()),
                    draft_only: request.draft_only,
                },
            )
            .await?;
        self.launch(at, agent, task_text, request.scenario, task, Vec::new());
        Ok(root)
    }

    /// Starts the Lead's next turn in a finished task (completed, failed, cancelled or
    /// interrupted). Taint, spending and refused calls are rebuilt from the task's log.
    pub async fn follow_up(
        &self,
        root: RunId,
        request: FollowUpRequest,
    ) -> Result<RunId, FollowUpError> {
        let text = request.text.trim().to_string();
        if text.is_empty() {
            return Err(FollowUpError::EmptyText);
        }
        let _claim = self.inner.follow_ups.lock().await;
        let events = self.inner.store.task_events(root).await?;
        let history = history::History::read(root, &events).ok_or(FollowUpError::UnknownTask)?;
        if history.open {
            return Err(FollowUpError::Running);
        }
        let team: Vec<Arc<AgentSpec>> = self
            .inner
            .store
            .team()
            .await?
            .into_iter()
            .map(Arc::new)
            .collect();
        let agent = team
            .iter()
            .find(|agent| agent.id() == history.root_agent && agent.role() == Role::Lead)
            .map(Arc::clone)
            .ok_or(FollowUpError::NotLead)?;

        let task = Arc::new(Task::new(
            history.budget,
            self.inner.config.allowlist.clone(),
            team,
            request.draft_only,
        ));
        history::replay(&task, &events);
        let earlier = history.items(&task.taint_sources());
        let at = RunRef {
            root_run_id: root,
            run_id: RunId::new(),
        };
        self.inner
            .emit(
                at,
                RunEvent::RunStarted {
                    agent_id: agent.id(),
                    parent_run_id: None,
                    parent_call_id: None,
                    task: text.clone(),
                    task_sources: Vec::new(),
                    title: None,
                    budget: None,
                    scenario: request.scenario.clone(),
                    agent_version: Some(agent.version()),
                    draft_only: request.draft_only,
                },
            )
            .await?;
        self.launch(at, agent, text, request.scenario, task, earlier);
        Ok(at.run_id)
    }

    /// Registers the task as live and spawns its top-level run. A watcher records a panic as
    /// the run's end and removes the entry, unless a later turn has replaced it.
    fn launch(
        &self,
        at: RunRef,
        agent: Arc<AgentSpec>,
        task_text: String,
        scenario: Option<String>,
        task: Arc<Task>,
        history: Vec<Item>,
    ) {
        let root = at.root_run_id;
        self.inner.tasks().insert(root, Arc::clone(&task));
        let ctx = run::RunCtx {
            at,
            agent_path: vec![agent.slug().clone()],
            agent,
            task_text,
            task_sources: Vec::new(),
            scenario,
            task: Arc::clone(&task),
            depth: 0,
            cancel: task.cancel_token(),
            caller: None,
            history,
        };
        let handle = run::spawn_run(Arc::clone(&self.inner), ctx);
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            if let Err(join_error) = handle.await {
                task.cancel_token().cancel();
                let outcome = RunOutcome::Failed {
                    reason: FailReason::Internal,
                    message: format!("the run stopped unexpectedly: {join_error}"),
                };
                if let Err(error) = inner.emit(at, RunEvent::RunFinished { outcome }).await {
                    tracing::error!(%error, run_id = ?at.run_id, "could not record the run's end");
                }
            }
            let mut tasks = inner.tasks();
            if tasks
                .get(&root)
                .is_some_and(|live| Arc::ptr_eq(live, &task))
            {
                tasks.remove(&root);
            }
        });
    }

    /// False when the approval is not pending: unknown, already answered, expired or cancelled.
    pub fn answer(&self, approval: ApprovalId, answer: Answer) -> bool {
        self.inner.approvals.answer(approval, answer)
    }

    /// Every tool, in `ToolName` order. `delegate` is listed without targets; each Lead's
    /// schema names its own.
    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        let mut specs = vec![delegate_spec(&[]), plan_spec()];
        specs.extend(self.inner.tools.specs().cloned());
        specs.sort_by_key(|spec| spec.name);
        specs
    }

    /// Hosts fetchable without asking, as every task receives them.
    pub fn allowlist(&self) -> &[String] {
        &self.inner.config.allowlist
    }

    /// Cancels every run in the task; false when no live task has this root.
    pub fn cancel(&self, root: RunId) -> bool {
        match self.inner.tasks().get(&root) {
            Some(task) => {
                task.cancel_token().cancel();
                true
            }
            None => false,
        }
    }
}

impl Inner {
    /// The engine's one write path; an error ends the caller's run.
    async fn emit(&self, at: RunRef, event: RunEvent) -> Result<(), StoreError> {
        self.store.emit(at, event).await.map(|_| ())
    }

    fn tasks(&self) -> MutexGuard<'_, HashMap<RunId, Arc<Task>>> {
        self.tasks.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
