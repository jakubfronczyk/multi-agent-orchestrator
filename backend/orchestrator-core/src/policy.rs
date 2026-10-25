//! The vocabulary of approval: decisions, effects, trust, taint sources and rules.
//! `Facts` is everything `decide` reads about one call; it borrows and is never stored.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::tools::Action;

mod decide;

pub use decide::decide;

/// An agent's setting for one tool; a tool with no decision is off and not offered.
/// The order `Allow < Ask < Deny` is the policy: `decide` takes the `max`, so reordering flips verdicts.
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
    OffAllowlist {
        host: String,
    },
    Taint {
        source: TaintSource,
    },
    NotOffered,
    InvalidArgs,
    RepeatOfRefused,
    /// An external call the agent allows; external effects always ask.
    ExternalEffect,
}

impl Rule {
    /// The rule's name as it appears in `kind` on the wire.
    pub fn kind(&self) -> &'static str {
        match self {
            Rule::AgentPolicy => "agent_policy",
            Rule::OffAllowlist { .. } => "off_allowlist",
            Rule::Taint { .. } => "taint",
            Rule::NotOffered => "not_offered",
            Rule::InvalidArgs => "invalid_args",
            Rule::RepeatOfRefused => "repeat_of_refused",
            Rule::ExternalEffect => "external_effect",
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_kind_matches_the_serialized_tag() {
        let source = TaintSource::Web {
            url: "u".to_string(),
        };
        for rule in [
            Rule::AgentPolicy,
            Rule::OffAllowlist {
                host: "h".to_string(),
            },
            Rule::Taint { source },
            Rule::NotOffered,
            Rule::InvalidArgs,
            Rule::RepeatOfRefused,
            Rule::ExternalEffect,
        ] {
            let tag = serde_json::to_value(&rule).expect("json")["kind"].clone();
            assert_eq!(tag, rule.kind());
        }
    }
}
