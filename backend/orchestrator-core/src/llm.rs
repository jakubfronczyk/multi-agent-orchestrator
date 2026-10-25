//! The model boundary: which models exist, the transcript, and the `LlmProvider` trait.
//! Provider items are opaque and echoed verbatim; only the server's parser reads their shape.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::agents::Slug;
use crate::ids::RunId;
use crate::money::Micros;
use crate::runs::Usage;
use crate::tools::ToolSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Model {
    #[serde(rename = "grok-4.3")]
    Grok43,
    #[serde(rename = "grok-4.20-0309-non-reasoning")]
    Grok420NonReasoning,
}

/// Who serves a model. A second provider is a new variant here plus a server adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Xai,
}

impl Model {
    pub const ALL: [Model; 2] = [Model::Grok43, Model::Grok420NonReasoning];

    /// Every model is listed, so a new model fails to compile until its provider is chosen.
    pub fn provider(self) -> Provider {
        match self {
            Model::Grok43 | Model::Grok420NonReasoning => Provider::Xai,
        }
    }

    /// The reasoning efforts the model accepts; empty means it takes none.
    pub fn efforts(self) -> &'static [ReasoningEffort] {
        match self {
            Model::Grok43 => &[
                ReasoningEffort::None,
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ],
            Model::Grok420NonReasoning => &[],
        }
    }
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

/// One transcript entry: the user's task text, a provider output item, or a call's output.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    User { text: String },
    Provider(Value),
    ToolOutput { call_id: String, output: String },
}

/// One function call the model made; `arguments` is the JSON string the model sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnCall {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolChoice {
    Auto,
    None,
}

/// Identifies one model step for the scripted provider; other providers ignore it.
#[derive(Debug, Clone)]
pub struct ScriptKey {
    pub root_run_id: RunId,
    pub agent: Slug,
    pub step: u32,
    pub scenario: Option<String>,
}

/// Everything one model step needs.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: Model,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub instructions: String,
    pub transcript: Vec<Item>,
    pub tools: Vec<ToolSpec>,
    pub tool_choice: ToolChoice,
    /// Used only by the scripted provider.
    pub script: ScriptKey,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletionResponse {
    /// Output items as the provider sent them, appended to the transcript unchanged.
    pub raw: Vec<Value>,
    pub calls: Vec<FnCall>,
    pub text: Option<String>,
    pub usage: Usage,
    pub cost_micros: Micros,
}

/// The run loop retries only what `is_retryable` says, never by reading messages.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LlmError {
    #[error("rate limited: {0}")]
    RateLimited(String),
    #[error("provider error: {0}")]
    Server(String),
    #[error("timed out")]
    Timeout,
    #[error("request rejected: {0}")]
    Rejected(String),
    #[error("unreadable provider output: {0}")]
    BadOutput(String),
    #[error("internal: {0}")]
    Internal(String),
}

impl LlmError {
    pub fn is_retryable(&self) -> bool {
        match self {
            LlmError::RateLimited(_) | LlmError::Server(_) | LlmError::Timeout => true,
            LlmError::Rejected(_) | LlmError::BadOutput(_) | LlmError::Internal(_) => false,
        }
    }
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError>;
}
