//! The run event log: every state change in a task is one `RunEvent`.
//! Events are append-only; tables and the UI are folded from them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::agents::Slug;
use crate::ids::{AgentId, ApprovalId, RunId, ToolCallId};
use crate::money::Micros;
use crate::policy::{Decision, Rule, TaintSource};
use crate::tools::{Action, PlanStep, ToolName};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[ts(export)]
pub enum RunEvent {
    RunStarted {
        agent_id: AgentId,
        parent_run_id: Option<RunId>,
        /// The `delegate` call that started this run; set on delegated runs only.
        #[serde(default)]
        parent_call_id: Option<ToolCallId>,
        /// Plain text; when `task_sources` is set the model receives it wrapped as data.
        task: String,
        /// The task's taint when this run was delegated; empty for a clean task or a root run.
        #[serde(default)]
        task_sources: Vec<TaintSource>,
        title: Option<String>,
        /// Set on the root run only.
        budget: Option<Micros>,
    },
    PlanRecorded {
        steps: Vec<PlanStep>,
    },
    LlmRequested {
        step: u32,
    },
    LlmResponded {
        step: u32,
        text: Option<String>,
        usage: Usage,
        cost: Micros,
    },
    LlmRetrying {
        step: u32,
        attempt: u32,
        message: String,
    },
    ToolCallRequested {
        call_id: ToolCallId,
        /// The name as the model sent it; it may not be a real tool.
        tool: String,
        args: Value,
        /// None when the tool was not offered or its arguments were invalid.
        action: Option<Action>,
        decision: Option<Decision>,
        reasons: Vec<Rule>,
    },
    ApprovalRequested {
        approval_id: ApprovalId,
        call_id: ToolCallId,
        action: Action,
        custody: Custody,
        preview: String,
        expires_at: DateTime<Utc>,
    },
    ApprovalResolved {
        approval_id: ApprovalId,
        outcome: ApprovalOutcome,
        note: Option<String>,
    },
    ToolCallStarted {
        call_id: ToolCallId,
    },
    ToolCallFinished {
        call_id: ToolCallId,
        status: CallStatus,
        result_preview: Option<String>,
        duration_ms: u64,
        child_run_id: Option<RunId>,
        /// Set only when the call was refused or denied, for the repeat check.
        tool: Option<ToolName>,
        args: Option<Value>,
    },
    TaskTainted {
        source: TaintSource,
    },
    RunFinished {
        outcome: RunOutcome,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    /// Input tokens the provider served from its prompt cache; part of `input_tokens`.
    #[serde(default)]
    pub cached_tokens: u64,
}

/// What an approval shows about how its call came to be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Custody {
    pub agent_path: Vec<Slug>,
    /// The calling run's task as plain text.
    pub parent_task: String,
    /// What `parent_task` carried from outside when it was delegated; empty when clean.
    #[serde(default)]
    pub parent_task_sources: Vec<TaintSource>,
    pub rules: Vec<Rule>,
    pub taint_sources: Vec<TaintSource>,
}

/// A run's lifecycle state; every terminal state comes from its `RunOutcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl From<&RunOutcome> for RunStatus {
    fn from(outcome: &RunOutcome) -> Self {
        match outcome {
            RunOutcome::Completed { .. } => RunStatus::Completed,
            RunOutcome::Failed { .. } => RunStatus::Failed,
            RunOutcome::Cancelled => RunStatus::Cancelled,
            RunOutcome::Interrupted => RunStatus::Interrupted,
        }
    }
}

/// Where a call is in its lifecycle; once `Finished`, its `CallStatus` says how it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CallPhase {
    Requested,
    Waiting,
    Running,
    Finished,
}

/// An approval's state: pending until a `ApprovalOutcome` resolves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
    Cancelled,
}

impl From<ApprovalOutcome> for ApprovalStatus {
    fn from(outcome: ApprovalOutcome) -> Self {
        match outcome {
            ApprovalOutcome::Approved => ApprovalStatus::Approved,
            ApprovalOutcome::Denied => ApprovalStatus::Denied,
            ApprovalOutcome::Expired => ApprovalStatus::Expired,
            ApprovalOutcome::Cancelled => ApprovalStatus::Cancelled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ApprovalOutcome {
    Approved,
    Denied,
    Expired,
    Cancelled,
}

/// How one call ended. Every value but `Ok` and `Interrupted` is also the `kind` the model
/// sees; `Interrupted` is set only by restart recovery and never reaches a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CallStatus {
    Ok,
    NotOffered,
    InvalidArgs,
    Refused,
    DeniedHuman,
    Expired,
    Timeout,
    Cancelled,
    Failed,
    Internal,
    ChildFailed,
    ChildCancelled,
    Interrupted,
}

impl CallStatus {
    /// Whether the model may try the same call again; told to it as `retryable`.
    pub fn is_retryable(self) -> bool {
        match self {
            CallStatus::Timeout => true,
            CallStatus::Ok
            | CallStatus::NotOffered
            | CallStatus::InvalidArgs
            | CallStatus::Refused
            | CallStatus::DeniedHuman
            | CallStatus::Expired
            | CallStatus::Cancelled
            | CallStatus::Failed
            | CallStatus::Internal
            | CallStatus::ChildFailed
            | CallStatus::ChildCancelled
            | CallStatus::Interrupted => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum RunOutcome {
    Completed { output: String, truncated: bool },
    Failed { reason: FailReason, message: String },
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum FailReason {
    LlmError,
    BudgetExceeded,
    Internal,
}

/// A human's answer on an approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum Answer {
    Approve { note: Option<String> },
    Deny { note: Option<String> },
}
