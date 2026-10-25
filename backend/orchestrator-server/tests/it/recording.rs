//! A provider around the scripted one: records every request, and slows or panics for one agent.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use orchestrator_core::llm::{CompletionRequest, CompletionResponse, LlmError, LlmProvider};
use orchestrator_server::llm::ScriptedProvider;

pub struct Recording {
    scripted: ScriptedProvider,
    slow_agent: &'static str,
    panic_agent: &'static str,
    requests: Mutex<Vec<CompletionRequest>>,
}

impl Recording {
    pub fn slowing(slow_agent: &'static str) -> Self {
        Self {
            scripted: ScriptedProvider::new(),
            slow_agent,
            panic_agent: "",
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Panics inside that agent's run, the way a bug in the run loop would.
    pub fn panicking(panic_agent: &'static str) -> Self {
        Self {
            panic_agent,
            ..Self::slowing("")
        }
    }

    pub fn requests(&self) -> Vec<CompletionRequest> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl LlmProvider for Recording {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        assert_ne!(
            request.script.agent.as_str(),
            self.panic_agent,
            "scripted panic"
        );
        if request.script.agent.as_str() == self.slow_agent {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        self.scripted.complete(request).await
    }
}
