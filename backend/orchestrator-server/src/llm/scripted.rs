//! The provider used without an API key: fixed replies, zero cost, no network.
//! It counts attempts per (task, agent, step), so every task replays its script from the start.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use orchestrator_core::ids::RunId;
use orchestrator_core::llm::{CompletionRequest, CompletionResponse, LlmError, LlmProvider};
use orchestrator_core::money::Micros;
use orchestrator_core::runs::Usage;

use super::parse_output;
use super::scripts::{Reply, reply};

/// (root run id, agent slug, step)
type AttemptKey = (RunId, String, u32);

#[derive(Default)]
pub struct ScriptedProvider {
    attempts: Mutex<HashMap<AttemptKey, u32>>,
}

impl ScriptedProvider {
    pub fn new() -> Self {
        Self::default()
    }

    fn next_attempt(&self, request: &CompletionRequest) -> u32 {
        let key = (
            request.root_run_id,
            request.agent.as_str().to_string(),
            request.step,
        );
        let mut attempts = self.attempts.lock().unwrap_or_else(PoisonError::into_inner);
        let count = attempts.entry(key).or_insert(0);
        *count += 1;
        *count - 1
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let attempt = self.next_attempt(&request);
        match reply(request.scenario.as_deref(), attempt) {
            Reply::Items(raw) => {
                let (calls, text) = parse_output(&raw)?;
                Ok(CompletionResponse {
                    raw,
                    calls,
                    text,
                    usage: Usage::default(),
                    cost_micros: Micros(0),
                })
            }
            Reply::Fail(error) => Err(error),
            Reply::Hang => std::future::pending().await,
        }
    }
}
