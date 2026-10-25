//! Live state of one task, shared by every run in it. `apply` changes it after the event that
//! justifies the change is committed; `try_start_run` and `claim_taint` check and set under the lock.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::agents::{AgentSpec, Slug};
use crate::ids::TeamId;
use crate::money::Micros;
use crate::policy::{Decision, Effect, Facts, TaintSource};
use crate::runs::RunEvent;
use crate::tools::{Action, ToolName};

pub(super) struct Task {
    /// The team the task runs in, fixed at its start.
    team_id: TeamId,
    budget: Micros,
    /// Copied from the config at start; fixed for the task's life.
    allowlist: Vec<String>,
    /// The team as it was when the task started; edits during the task do not reach it.
    team: Vec<Arc<AgentSpec>>,
    /// The current turn was started as draft only; every run in the turn inherits it.
    draft_only: bool,
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
    /// Calls refused by agent policy or denied by a person; a repeat is refused without a card.
    /// A draft-only refusal is not recorded, so a later turn may make the same call.
    refused: Vec<(ToolName, Value)>,
}

impl Task {
    pub(super) fn new(
        team_id: TeamId,
        budget: Micros,
        allowlist: Vec<String>,
        team: Vec<Arc<AgentSpec>>,
        draft_only: bool,
    ) -> Self {
        Self {
            team_id,
            budget,
            allowlist,
            team,
            draft_only,
            cancel: CancellationToken::new(),
            state: Mutex::new(TaskState {
                spent: Micros(0),
                taint: Vec::new(),
                runs_started: 1,
                refused: Vec::new(),
            }),
        }
    }

    pub(super) fn team_id(&self) -> TeamId {
        self.team_id
    }

    /// A slug names an agent of the task's own team only; the team check holds even if the
    /// snapshot were ever built from more than one team.
    pub(super) fn teammate(&self, slug: &str) -> Option<Arc<AgentSpec>> {
        self.team
            .iter()
            .find(|agent| agent.team_id() == self.team_id && agent.slug().as_str() == slug)
            .cloned()
    }

    pub(super) fn teammates(&self, slugs: &[Slug]) -> Vec<Arc<AgentSpec>> {
        slugs
            .iter()
            .filter_map(|slug| self.teammate(slug.as_str()))
            .collect()
    }

    /// Checks and counts a new run in one locked step, so parallel delegations cannot both
    /// take the last slot.
    pub(super) fn try_start_run(&self, limit: u32) -> bool {
        let mut state = self.lock();
        if state.runs_started >= limit {
            return false;
        }
        state.runs_started += 1;
        true
    }

    /// The taint is cloned under the lock, so no lock is held while deciding.
    pub(super) fn facts<'a>(
        &'a self,
        own: Decision,
        effect: Effect,
        action: &'a Action,
    ) -> Facts<'a> {
        Facts {
            own,
            effect,
            action,
            taint: self.lock().taint.clone(),
            allowlist: &self.allowlist,
            draft_only: self.draft_only,
        }
    }

    pub(super) fn draft_only(&self) -> bool {
        self.draft_only
    }

    pub(super) fn taint_sources(&self) -> Vec<TaintSource> {
        self.lock().taint.clone()
    }

    pub(super) fn is_tainted(&self) -> bool {
        !self.lock().taint.is_empty()
    }

    /// Records a source the first time it is seen and says whether it was new, in one locked
    /// step, so parallel calls reading the same source emit `task_tainted` once.
    pub(super) fn claim_taint(&self, source: &TaintSource) -> bool {
        let mut state = self.lock();
        if state.taint.contains(source) {
            return false;
        }
        state.taint.push(source.clone());
        true
    }

    /// Same tool, equal arguments (`Value ==`).
    pub(super) fn is_repeat_of_refused(&self, tool: ToolName, args: &Value) -> bool {
        self.lock()
            .refused
            .iter()
            .any(|(t, a)| *t == tool && a == args)
    }

    pub(super) fn budget(&self) -> Micros {
        self.budget
    }

    pub(super) fn spent(&self) -> Micros {
        self.lock().spent
    }

    /// Cancelling it cancels every run in the task.
    pub(super) fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Every variant is listed, so a new event type fails to compile until its effect is decided.
    pub(super) fn apply(&self, event: &RunEvent) {
        match event {
            RunEvent::LlmResponded { cost, .. } => {
                let mut state = self.lock();
                state.spent = state.spent.saturating_add(*cost);
            }
            // Refused and denied calls carry their tool and arguments; nothing else does.
            RunEvent::ToolCallFinished {
                tool: Some(tool),
                args: Some(args),
                ..
            } => {
                let mut state = self.lock();
                if !state.refused.iter().any(|(t, a)| t == tool && a == args) {
                    state.refused.push((*tool, args.clone()));
                }
            }
            // The source was recorded by `claim_taint` before this event was emitted.
            RunEvent::TaskTainted { .. }
            | RunEvent::RunStarted { .. }
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::agents::{AgentInput, Role};
    use crate::ids::AgentId;
    use crate::llm::Model;

    fn agent(team: TeamId, slug: &str) -> Arc<AgentSpec> {
        let input = AgentInput {
            team_id: None,
            role: Role::Specialist,
            slug: slug.to_string(),
            name: slug.to_string(),
            description: "d".to_string(),
            system_prompt: "p".to_string(),
            model: Model::Grok43,
            reasoning_effort: None,
            icon: "i".to_string(),
            colour: "c".to_string(),
            tools: BTreeMap::new(),
            delegate_targets: Vec::new(),
        };
        Arc::new(AgentSpec::validated(AgentId::new(), team, 1, input).expect("valid"))
    }

    /// A slug resolves only to an agent of the task's own team, even in a mixed snapshot.
    #[test]
    fn a_teammate_is_always_of_the_tasks_team() {
        let (own, other) = (TeamId::new(), TeamId::new());
        let team = vec![agent(other, "reader"), agent(own, "writer")];
        let task = Task::new(own, Micros(0), Vec::new(), team, false);
        assert!(task.teammate("reader").is_none());
        assert_eq!(task.teammate("writer").map(|a| a.team_id()), Some(own));
    }
}
