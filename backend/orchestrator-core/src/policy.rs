//! The vocabulary of approval: decisions, effects, trust, taint sources and rules.
//! `Facts` is everything `decide` reads about one call; it borrows and is never stored.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::tools::Action;

/// An agent's setting for one tool. A tool with no decision is off and not offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Decision {
    Allow,
    Ask,
    Deny,
}

/// The fixed kind of risk a tool carries; it never depends on the arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Effect {
    Control,
    Read,
    Memory,
    Write,
    Egress,
    External,
}

/// Whether the Lead must, may or must not have a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum LeadAccess {
    Only,
    Allowed,
    Forbidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Trust {
    Trusted,
    Untrusted,
}

/// One piece of outside content that tainted a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum TaintSource {
    Inbox { message_id: String },
    Web { url: String },
}

/// A named reason that shaped a call's decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum Rule {
    AgentPolicy,
    OffAllowlist { host: String },
    Taint { source: TaintSource },
    NotOffered,
    InvalidArgs,
    RepeatOfRefused,
}

pub struct Facts<'a> {
    pub own: Decision,
    pub effect: Effect,
    pub action: &'a Action,
    /// An owned snapshot, cloned under the task lock so no lock is held while deciding.
    pub taint: Vec<TaintSource>,
    /// Borrowed from the task's fixed allowlist.
    pub allowlist: &'a [String],
}
