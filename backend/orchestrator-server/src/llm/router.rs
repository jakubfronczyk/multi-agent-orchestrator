//! Picks the provider per request. A task started with a scenario always plays scripted
//! replies, so the demo and the drill show the same path every time, key or no key.

use std::sync::Arc;

use async_trait::async_trait;
use orchestrator_core::llm::{CompletionRequest, CompletionResponse, LlmError, LlmProvider};

use super::ScriptedProvider;

pub struct ScenarioRouter {
    /// Answers free-text tasks; `None` without a key, so they get the scripted demo reply.
    live: Option<Arc<dyn LlmProvider>>,
    scripted: ScriptedProvider,
}

impl ScenarioRouter {
    pub fn new(live: Option<Arc<dyn LlmProvider>>) -> Self {
        Self {
            live,
            scripted: ScriptedProvider::new(),
        }
    }
}

#[async_trait]
impl LlmProvider for ScenarioRouter {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        match (&request.script.scenario, &self.live) {
            (None, Some(live)) => live.complete(request).await,
            (Some(_), _) | (None, None) => self.scripted.complete(request).await,
        }
    }
}
