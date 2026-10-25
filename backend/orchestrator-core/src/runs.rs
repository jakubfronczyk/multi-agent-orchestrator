//! The run event log: every state change in a task is one `RunEvent`.
//! Events are append-only; tables and the UI are folded from them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::agents::Slug;
use crate::ids::{AgentId, RunId, ToolCallId, WarrantId};
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
        task: String,
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
    WarrantRequested {
        warrant_id: WarrantId,
        call_id: ToolCallId,
        action: Action,
        custody: Custody,
        preview: String,
        expires_at: DateTime<Utc>,
    },
    WarrantResolved {
        warrant_id: WarrantId,
        outcome: WarrantOutcome,
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
}

/// What a warrant shows about how its call came to be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Custody {
    pub agent_path: Vec<Slug>,
    pub parent_task: String,
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

/// A warrant's state: pending until a `WarrantOutcome` resolves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WarrantStatus {
    Pending,
    Approved,
    Denied,
    Expired,
    Cancelled,
}

impl From<WarrantOutcome> for WarrantStatus {
    fn from(outcome: WarrantOutcome) -> Self {
        match outcome {
            WarrantOutcome::Approved => WarrantStatus::Approved,
            WarrantOutcome::Denied => WarrantStatus::Denied,
            WarrantOutcome::Expired => WarrantStatus::Expired,
            WarrantOutcome::Cancelled => WarrantStatus::Cancelled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WarrantOutcome {
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

/// A human's answer on a warrant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum Answer {
    Approve { note: Option<String> },
    Deny { note: Option<String> },
}
