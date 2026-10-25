//! The engine is the API's only way to change runs: it starts a task and cancels it.
//! Each run is its own spawned task, and a watcher awaits it so a panic still ends the run.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::Semaphore;

use crate::agents::Role;
use crate::ids::{AgentId, RunId};
use crate::llm::LlmProvider;
use crate::money::Micros;
use crate::runs::{FailReason, RunEvent, RunOutcome};
use crate::store::{RunRef, Store, StoreError};

mod run;
mod task;

pub use task::Task;

/// Limits and timings; tests build their own with millisecond values.
#[derive(Debug, Clone)]
pub struct Config {
    pub llm_concurrency: usize,
    /// The longest one model call may take; reaching it is a retryable timeout.
    pub llm_timeout: Duration,
    /// One delay per retry of a retryable model error; its length is the retry count.
    pub retry_backoff: Vec<Duration>,
    pub default_budget: Micros,
}

#[derive(Debug, Clone)]
pub struct StartRequest {
    /// The root agent; the Lead when absent.
    pub agent_id: Option<AgentId>,
    pub task: String,
    pub title: Option<String>,
    pub budget: Option<Micros>,
    pub scenario: Option<String>,
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
    config: Config,
    /// Caps model calls in flight across all runs.
    llm_slots: Semaphore,
    /// Live tasks by root run id; an entry leaves when its root run has finished.
    tasks: Mutex<HashMap<RunId, Arc<Task>>>,
}

impl Engine {
    pub fn new(store: Arc<dyn Store>, llm: Arc<dyn LlmProvider>, config: Config) -> Self {
        let llm_slots = Semaphore::new(config.llm_concurrency);
        Self {
            inner: Arc::new(Inner {
                store,
                llm,
                config,
                llm_slots,
                tasks: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Emits `run_started` before spawning, so a storage failure is a start error, not a lost run.
    pub async fn start(&self, request: StartRequest) -> Result<RunId, StartError> {
        let task_text = request.task.trim().to_string();
        if task_text.is_empty() {
            return Err(StartError::EmptyTask);
        }
        let team = self.inner.store.team().await?;
        let agent = match request.agent_id {
            Some(id) => team
                .into_iter()
                .find(|agent| agent.id() == id)
                .ok_or(StartError::UnknownAgent)?,
            None => team
                .into_iter()
                .find(|agent| agent.role() == Role::Lead)
                .ok_or(StartError::NoLead)?,
        };

        let root = RunId::new();
        let at = RunRef {
            root_run_id: root,
            run_id: root,
        };
        let budget = request.budget.unwrap_or(self.inner.config.default_budget);
        let task = Arc::new(Task::new(budget));
        self.inner
            .store
            .emit(
                at,
                RunEvent::RunStarted {
                    agent_id: agent.id(),
                    parent_run_id: None,
                    task: task_text.clone(),
                    title: request.title,
                    budget: Some(budget),
                },
            )
            .await?;
        self.inner.tasks().insert(root, Arc::clone(&task));

        let ctx = run::RunCtx {
            at,
            agent: Arc::new(agent),
            task_text,
            scenario: request.scenario,
            task: Arc::clone(&task),
        };
        let handle = tokio::spawn(run::run_agent(Arc::clone(&self.inner), ctx));
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            if let Err(join_error) = handle.await {
                task.cancel_token().cancel();
                let outcome = RunOutcome::Failed {
                    reason: FailReason::Internal,
                    message: format!("the run stopped unexpectedly: {join_error}"),
                };
                if let Err(error) = inner
                    .store
                    .emit(at, RunEvent::RunFinished { outcome })
                    .await
                {
                    tracing::error!(%error, run_id = ?root, "could not record the run's end");
                }
            }
            inner.tasks().remove(&root);
        });
        Ok(root)
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
    fn tasks(&self) -> MutexGuard<'_, HashMap<RunId, Arc<Task>>> {
        self.tasks.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
