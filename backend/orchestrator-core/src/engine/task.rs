//! Live state of one task, shared by every run in it. Only `apply` changes it, and only
//! after the event that justifies the change has been committed.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio_util::sync::CancellationToken;

use crate::agents::{AgentSpec, Slug};
use crate::money::Micros;
use crate::policy::{Decision, Effect, Facts, TaintSource};
use crate::runs::RunEvent;
use crate::tools::Action;

pub struct Task {
    budget: Micros,
    /// Copied from the config at start; fixed for the task's life.
    allowlist: Vec<String>,
    /// The team as it was when the task started; edits during the task do not reach it.
    team: Vec<Arc<AgentSpec>>,
    cancel: CancellationToken,
    /// A std mutex: it is never held across an await.
    state: Mutex<TaskState>,
}

struct TaskState {
    spent: Micros,
    /// Every source in read order; nothing removes one.
    taint: Vec<TaintSource>,
    /// Runs started in this task, the root included.
    runs_started: u32,
}

impl Task {
    pub fn new(budget: Micros, allowlist: Vec<String>, team: Vec<Arc<AgentSpec>>) -> Self {
        Self {
            budget,
            allowlist,
            team,
            cancel: CancellationToken::new(),
            state: Mutex::new(TaskState {
                spent: Micros(0),
                taint: Vec::new(),
                runs_started: 1,
            }),
        }
    }

    pub fn teammate(&self, slug: &str) -> Option<Arc<AgentSpec>> {
        self.team
            .iter()
            .find(|agent| agent.slug().as_str() == slug)
            .cloned()
    }

    pub fn teammates(&self, slugs: &[Slug]) -> Vec<Arc<AgentSpec>> {
        slugs
            .iter()
            .filter_map(|slug| self.teammate(slug.as_str()))
            .collect()
    }

    /// Checks and counts a new run in one locked step, so parallel delegations cannot both
    /// take the last slot. The one state change that does not go through `apply`.
    pub fn try_start_run(&self, limit: u32) -> bool {
        let mut state = self.lock();
        if state.runs_started >= limit {
            return false;
        }
        state.runs_started += 1;
        true
    }

    /// The taint is cloned under the lock, so no lock is held while deciding.
    pub fn facts<'a>(&'a self, own: Decision, effect: Effect, action: &'a Action) -> Facts<'a> {
        Facts {
            own,
            effect,
            action,
            taint: self.lock().taint.clone(),
            allowlist: &self.allowlist,
        }
    }

    pub fn taint_sources(&self) -> Vec<TaintSource> {
        self.lock().taint.clone()
    }

    pub fn budget(&self) -> Micros {
        self.budget
    }

    pub fn spent(&self) -> Micros {
        self.lock().spent
    }

    /// Cancelling it cancels every run in the task.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Every variant is listed, so a new event type fails to compile until its effect is decided.
    pub fn apply(&self, event: &RunEvent) {
        match event {
            RunEvent::LlmResponded { cost, .. } => {
                let mut state = self.lock();
                state.spent = state.spent.saturating_add(*cost);
            }
            RunEvent::TaskTainted { source } => {
                let mut state = self.lock();
                if !state.taint.contains(source) {
                    state.taint.push(source.clone());
                }
            }
            RunEvent::RunStarted { .. }
            | RunEvent::PlanRecorded { .. }
            | RunEvent::LlmRequested { .. }
            | RunEvent::LlmRetrying { .. }
            | RunEvent::ToolCallRequested { .. }
            | RunEvent::ApprovalRequested { .. }
            | RunEvent::ApprovalResolved { .. }
            | RunEvent::ToolCallStarted { .. }
            | RunEvent::ToolCallFinished { .. }
            | RunEvent::RunFinished { .. } => {}
        }
    }

    /// A poisoned lock still holds consistent data: every write is a single assignment.
    fn lock(&self) -> MutexGuard<'_, TaskState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
