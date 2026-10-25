//! The xAI provider: plain reqwest against the Responses API (§11). The request is a typed
//! struct because xAI ignores unknown fields, so a misspelt one would silently change nothing.

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

use super::parse_output;

const BASE_URL: &str = "https://api.x.ai/v1";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// `cost_in_usd_ticks` counts 10^10 ticks per USD; dividing by 10^4 gives micros.
const TICKS_PER_MICRO: i64 = 10_000;
/// Every model this app offers must be listed by `/v1/models` for the key to be accepted.
const REQUIRED_MODELS: [Model; 2] = [Model::Grok43, Model::Grok420NonReasoning];

pub struct XaiProvider {
    client: reqwest::Client,
    /// Exposed only to build the Authorization header; never logged or formatted.
    key: SecretString,
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
}

#[derive(Serialize)]
struct Reasoning {
    effort: ReasoningEffort,
}

#[derive(Serialize)]
#[serde(untagged)]
enum InputItem<'a> {
    User {
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
    output_tokens_details: Option<OutputDetails>,
    cost_in_usd_ticks: Option<i64>,
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
        Ok(Self { client, key })
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
        for model in REQUIRED_MODELS {
            let id = model_id(model);
            if !listed.data.iter().any(|entry| entry.id == id) {
                return Err(format!("xAI does not list the model {id}"));
            }
        }
        Ok(())
    }
}

fn model_id(model: Model) -> String {
    serde_json::to_value(model)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn request_body(request: &CompletionRequest) -> ResponsesRequest<'_> {
    let input = request
        .transcript
        .iter()
        .map(|item| match item {
            Item::User { text } => InputItem::User {
                role: "user",
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
            name: serde_json::to_value(spec.name)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
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
    }
}

/// 429 and 5xx are retried; every other non-success status fails the call.
fn status_error(status: StatusCode, body: &str) -> LlmError {
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
            return Err(status_error(status, &body));
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
}
