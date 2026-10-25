//! The xAI adapter: plain reqwest against the Responses API (§11), and everything shaped by xAI.
//! The request is a typed struct because xAI ignores unknown fields, so a misspelt one would change nothing.

mod parse;

pub use parse::parse_output;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use orchestrator_core::llm::{
    CompletionRequest, CompletionResponse, Item, LlmError, LlmProvider, Model, ReasoningEffort,
    ToolChoice,
};
use orchestrator_core::money::Micros;
use orchestrator_core::runs::Usage;
use reqwest::StatusCode;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sqlite::text::to_text;

const BASE_URL: &str = "https://api.x.ai/v1";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// `cost_in_usd_ticks` counts 10^10 ticks per USD; dividing by 10^4 gives micros.
const TICKS_PER_MICRO: i64 = 10_000;
/// Marks a 429 caused by the key's own token cap, not the team's rate limit.
const KEY_CAP_MARKER: &str = "API key's token limit";

pub struct XaiProvider {
    client: reqwest::Client,
    /// Exposed only to build the Authorization header; never logged or formatted.
    key: SecretString,
    key_cap_logged: AtomicBool,
}

#[derive(Serialize)]
struct ResponsesRequest<'a> {
    model: Model,
    instructions: &'a str,
    input: Vec<InputItem<'a>>,
    tools: Vec<FunctionTool<'a>>,
    tool_choice: &'static str,
    /// Nothing is kept on xAI's side; the full transcript is sent every call.
    store: bool,
    /// Reasoning comes back encrypted so it can be echoed in the next call.
    include: [&'static str; 1],
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<Reasoning>,
    /// One key per task, so every call of a task's runs can hit the same prompt cache.
    prompt_cache_key: String,
}

#[derive(Serialize)]
struct Reasoning {
    effort: ReasoningEffort,
}

#[derive(Serialize)]
#[serde(untagged)]
enum InputItem<'a> {
    Message {
        role: &'static str,
        content: &'a str,
    },
    /// A provider output item from an earlier call, sent back exactly as received.
    Echo(&'a Value),
    Output {
        r#type: &'static str,
        call_id: &'a str,
        output: &'a str,
    },
}

#[derive(Serialize)]
struct FunctionTool<'a> {
    r#type: &'static str,
    name: String,
    description: &'a str,
    parameters: &'a Value,
}

#[derive(Deserialize)]
struct ResponsesReply {
    output: Vec<Value>,
    usage: Option<ReplyUsage>,
}

#[derive(Deserialize)]
struct ReplyUsage {
    input_tokens: u64,
    output_tokens: u64,
    input_tokens_details: Option<InputDetails>,
    output_tokens_details: Option<OutputDetails>,
    cost_in_usd_ticks: Option<i64>,
}

#[derive(Deserialize)]
struct InputDetails {
    cached_tokens: u64,
}

#[derive(Deserialize)]
struct OutputDetails {
    reasoning_tokens: u64,
}

/// xAI's error body is flat: `{"code": "...", "error": "..."}`.
#[derive(Deserialize)]
struct XaiError {
    code: String,
    error: String,
}

#[derive(Deserialize)]
struct ModelList {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

impl XaiProvider {
    pub fn new(key: SecretString) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|_| "the HTTP client could not start".to_string())?;
        Ok(Self {
            client,
            key,
            key_cap_logged: AtomicBool::new(false),
        })
    }

    /// Run at boot: the key must work and every offered model must exist. A bad key is a
    /// 400 from xAI, not a 401.
    pub async fn check(&self) -> Result<(), String> {
        let response = self
            .client
            .get(format!("{BASE_URL}/models"))
            .bearer_auth(self.key.expose_secret())
            .send()
            .await
            .map_err(|_| "xAI could not be reached".to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "xAI rejected the key (HTTP {})",
                response.status().as_u16()
            ));
        }
        let listed: ModelList = response
            .json()
            .await
            .map_err(|_| "xAI's model list is unreadable".to_string())?;
        // Every model this app offers must be listed for the key to be accepted.
        for model in Model::ALL {
            let id = model_id(model);
            if !listed.data.iter().any(|entry| entry.id == id) {
                return Err(format!("xAI does not list the model {id}"));
            }
        }
        Ok(())
    }
}

fn model_id(model: Model) -> String {
    to_text(&model).unwrap_or_default()
}

fn request_body(request: &CompletionRequest) -> ResponsesRequest<'_> {
    let input = request
        .transcript
        .iter()
        .map(|item| match item {
            Item::User { text } => InputItem::Message {
                role: "user",
                content: text,
            },
            Item::Assistant { text } => InputItem::Message {
                role: "assistant",
                content: text,
            },
            Item::Provider(raw) => InputItem::Echo(raw),
            Item::ToolOutput { call_id, output } => InputItem::Output {
                r#type: "function_call_output",
                call_id,
                output,
            },
        })
        .collect();
    let tools = request
        .tools
        .iter()
        .map(|spec| FunctionTool {
            r#type: "function",
            name: to_text(&spec.name).unwrap_or_default(),
            description: &spec.description,
            parameters: &spec.schema,
        })
        .collect();
    // Only grok-4.3 takes a reasoning effort.
    let reasoning = match request.model {
        Model::Grok43 => request.reasoning_effort.map(|effort| Reasoning { effort }),
        Model::Grok420NonReasoning => None,
    };
    ResponsesRequest {
        model: request.model,
        instructions: &request.instructions,
        input,
        tools,
        tool_choice: match request.tool_choice {
            ToolChoice::Auto => "auto",
            ToolChoice::None => "none",
        },
        store: false,
        include: ["reasoning.encrypted_content"],
        reasoning,
        prompt_cache_key: request.script.root_run_id.to_string(),
    }
}

/// The `(actual, limit)` tokens per minute from a key-cap message, when it states them.
fn key_cap(message: &str) -> Option<(u64, u64)> {
    let (_, rest) = message.split_once("(actual/limit): ")?;
    let figures = rest.split(|c: char| c.is_whitespace() || c == '.').next()?;
    let (actual, limit) = figures.split_once('/')?;
    Some((actual.parse().ok()?, limit.parse().ok()?))
}

/// The provider's key-cap text names the key, so the message is built from the figures alone.
fn key_cap_message(body: &str) -> String {
    match key_cap(body) {
        Some((actual, limit)) => {
            format!("this API key is capped at {limit} tokens/min (used {actual})")
        }
        None => "this API key is over its tokens/min cap".to_string(),
    }
}

/// A 429 caused by the key's own token cap, not the team's rate limit.
fn is_key_cap(status: StatusCode, body: &str) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS && body.contains(KEY_CAP_MARKER)
}

/// 429 and 5xx are retried; every other non-success status fails the call.
fn status_error(status: StatusCode, body: &str) -> LlmError {
    if is_key_cap(status, body) {
        return LlmError::RateLimited(key_cap_message(body));
    }
    let detail = match serde_json::from_str::<XaiError>(body) {
        Ok(error) => format!("{} ({}): {}", status.as_u16(), error.code, error.error),
        Err(_) => format!("HTTP {}", status.as_u16()),
    };
    if status == StatusCode::TOO_MANY_REQUESTS {
        LlmError::RateLimited(detail)
    } else if status.is_server_error() {
        LlmError::Server(detail)
    } else {
        LlmError::Rejected(detail)
    }
}

#[async_trait]
impl LlmProvider for XaiProvider {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let response = self
            .client
            .post(format!("{BASE_URL}/responses"))
            .bearer_auth(self.key.expose_secret())
            .json(&request_body(&request))
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    LlmError::Timeout
                } else {
                    LlmError::Server("xAI could not be reached".to_string())
                }
            })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|_| LlmError::Server("the reply broke off".to_string()))?;
        if !status.is_success() {
            let error = status_error(status, &body);
            if is_key_cap(status, body.as_str())
                && !self.key_cap_logged.swap(true, Ordering::Relaxed)
            {
                tracing::warn!(
                    "xAI: {}; calls keep retrying with backoff",
                    key_cap_message(&body)
                );
            }
            return Err(error);
        }

        let reply: ResponsesReply = serde_json::from_str(&body)
            .map_err(|_| LlmError::BadOutput("the reply is not a Responses body".to_string()))?;
        let usage = reply
            .usage
            .ok_or_else(|| LlmError::BadOutput("the reply has no usage".to_string()))?;
        // §11: a missing cost fails loudly rather than being counted as free.
        let ticks = usage
            .cost_in_usd_ticks
            .ok_or_else(|| LlmError::BadOutput("the reply has no cost".to_string()))?;
        let (calls, text) = parse_output(&reply.output)?;
        Ok(CompletionResponse {
            raw: reply.output,
            calls,
            text,
            usage: Usage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                reasoning_tokens: usage
                    .output_tokens_details
                    .map_or(0, |d| d.reasoning_tokens),
                cached_tokens: usage.input_tokens_details.map_or(0, |d| d.cached_tokens),
            },
            cost_micros: Micros(ticks / TICKS_PER_MICRO),
        })
    }
}

#[cfg(test)]
mod tests {
    use orchestrator_core::agents::Slug;
    use orchestrator_core::ids::RunId;
    use orchestrator_core::llm::ScriptKey;
    use serde_json::json;

    use super::*;

    fn request(model: Model, effort: Option<ReasoningEffort>) -> CompletionRequest {
        CompletionRequest {
            model,
            reasoning_effort: effort,
            instructions: "be brief".to_string(),
            transcript: vec![
                Item::User {
                    text: "hi".to_string(),
                },
                Item::Provider(
                    json!({"type": "function_call", "id": "fc_1", "call_id": "call-1", "name": "plan", "arguments": "{}", "status": "completed"}),
                ),
                Item::ToolOutput {
                    call_id: "call-1".to_string(),
                    output: "{\"ok\":true}".to_string(),
                },
            ],
            tools: Vec::new(),
            tool_choice: ToolChoice::None,
            script: ScriptKey {
                root_run_id: RunId::new(),
                run_id: RunId::new(),
                agent: Slug::new("lead").expect("slug"),
                step: 0,
                scenario: None,
            },
        }
    }

    #[test]
    fn the_body_echoes_provider_items_verbatim_and_sends_effort_only_to_grok_4_3() {
        let grok43 = serde_json::to_value(request_body(&request(
            Model::Grok43,
            Some(ReasoningEffort::Low),
        )))
        .expect("json");
        assert_eq!(grok43["store"], false);
        assert_eq!(grok43["tool_choice"], "none");
        assert_eq!(grok43["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(grok43["reasoning"], json!({"effort": "low"}));
        assert!(
            grok43["prompt_cache_key"]
                .as_str()
                .is_some_and(|key| !key.is_empty()),
            "the root run id is the cache key"
        );
        assert_eq!(grok43["input"][0], json!({"role": "user", "content": "hi"}));
        assert_eq!(
            grok43["input"][1]["id"], "fc_1",
            "echoed item keeps every field"
        );
        assert_eq!(
            grok43["input"][2],
            json!({"type": "function_call_output", "call_id": "call-1", "output": "{\"ok\":true}"})
        );

        let grok420 = serde_json::to_value(request_body(&request(
            Model::Grok420NonReasoning,
            Some(ReasoningEffort::Low),
        )))
        .expect("json");
        assert!(grok420.get("reasoning").is_none());
        assert_eq!(grok420["model"], "grok-4.20-0309-non-reasoning");
    }

    #[test]
    fn statuses_map_to_retryable_or_fatal_errors() {
        let body = r#"{"code": "invalid-argument", "error": "bad schema"}"#;
        assert!(status_error(StatusCode::TOO_MANY_REQUESTS, body).is_retryable());
        assert!(status_error(StatusCode::BAD_GATEWAY, "not json").is_retryable());
        let rejected = status_error(StatusCode::BAD_REQUEST, body);
        assert!(!rejected.is_retryable());
        assert_eq!(
            rejected.to_string(),
            "request rejected: 400 (invalid-argument): bad schema"
        );
    }

    #[test]
    fn a_key_cap_429_is_retryable_and_never_echoes_the_provider_text() {
        let body = r#"{"code": "resource-exhausted", "error": "Too many tokens consumed for API key xai-...XXXX. Your API key's token limit is — Tokens per Minute (actual/limit): 27077/100. Your rate limit tier is 1."}"#;
        let error = status_error(StatusCode::TOO_MANY_REQUESTS, body);
        assert!(error.is_retryable());
        assert_eq!(
            error.to_string(),
            "rate limited: this API key is capped at 100 tokens/min (used 27077)"
        );
        let unstated = status_error(
            StatusCode::TOO_MANY_REQUESTS,
            "Your API key's token limit, key xai-...XXXX",
        );
        assert!(!unstated.to_string().contains("XXXX"));
    }
}
