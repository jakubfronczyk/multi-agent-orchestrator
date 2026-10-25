//! The provider used without an API key: fixed replies, zero cost, no network.
//! It counts attempts per (task, agent, step), so every task replays its script from the start.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use orchestrator_core::ids::RunId;
use orchestrator_core::llm::{CompletionRequest, CompletionResponse, Item, LlmError, LlmProvider};
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
            request.script.root_run_id,
            request.script.agent.as_str().to_string(),
            request.script.step,
        );
        let mut attempts = self.attempts.lock().unwrap_or_else(PoisonError::into_inner);
        let count = attempts.entry(key).or_insert(0);
        *count += 1;
        *count - 1
    }
}

fn task_text(transcript: &[Item]) -> String {
    transcript
        .iter()
        .find_map(|item| match item {
            Item::User { text } => Some(text.clone()),
            Item::Provider(_) | Item::ToolOutput { .. } => None,
        })
        .unwrap_or_default()
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let attempt = self.next_attempt(&request);
        let script = &request.script;
        match reply(
            script.scenario.as_deref(),
            script.agent.as_str(),
            script.step,
            attempt,
            request.tool_choice,
            &task_text(&request.transcript),
        ) {
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
