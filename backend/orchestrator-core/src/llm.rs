//! The models an agent can use, as an enum because their price table lives in code.
//! Only two stable xAI ids are offered; reasoning effort applies to grok-4.3 alone.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Model {
    #[serde(rename = "grok-4.3")]
    Grok43,
    #[serde(rename = "grok-4.20-0309-non-reasoning")]
    Grok420NonReasoning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReasoningEffort {
    None,
    Low,
    Medium,
    High,
}
