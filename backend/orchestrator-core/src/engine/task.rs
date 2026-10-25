//! Live state of one task, shared by every run in it. Only `apply` changes it, and only
//! after the event that justifies the change has been committed.

use std::sync::{Mutex, MutexGuard, PoisonError};

use tokio_util::sync::CancellationToken;

use crate::money::Micros;
use crate::policy::{Decision, Effect, Facts, TaintSource};
use crate::runs::RunEvent;
use crate::tools::Action;

pub struct Task {
    budget: Micros,
    /// Copied from the config at start; fixed for the task's life.
    allowlist: Vec<String>,
    cancel: CancellationToken,
    /// A std mutex: it is never held across an await.
    state: Mutex<TaskState>,
}

struct TaskState {
    spent: Micros,
    /// Every source in read order; nothing removes one.
    taint: Vec<TaintSource>,
}

impl Task {
    pub fn new(budget: Micros, allowlist: Vec<String>) -> Self {
        Self {
            budget,
            allowlist,
            cancel: CancellationToken::new(),
            state: Mutex::new(TaskState {
                spent: Micros(0),
                taint: Vec::new(),
            }),
        }
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
