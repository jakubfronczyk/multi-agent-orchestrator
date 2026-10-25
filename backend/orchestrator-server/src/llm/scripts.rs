//! Fixed replies for the scripted provider, keyed by scenario and attempt.
//! Replies use the xAI output-item shape so they pass through the same parser.

use orchestrator_core::llm::LlmError;
use serde_json::{Value, json};

pub const DEMO_REPLY: &str = "Demo mode: try one of the example tasks.";

pub enum Reply {
    Items(Vec<Value>),
    Fail(LlmError),
    /// Never answers; only cancellation ends the call.
    Hang,
}

pub fn reply(scenario: Option<&str>, attempt: u32) -> Reply {
    match scenario {
        Some("test:retry") if attempt == 0 => {
            Reply::Fail(LlmError::Server("scripted 503".to_string()))
        }
        Some("test:retry") => message("recovered"),
        Some("test:fatal") => Reply::Fail(LlmError::Rejected("scripted 400".to_string())),
        Some("test:hang") => Reply::Hang,
        Some("test:call") => Reply::Items(vec![json!({
            "type": "function_call",
            "call_id": "call-1",
            "name": "memory_read",
            "arguments": "{}"
        })]),
        Some("test:hello") => message("hello"),
        _ => message(DEMO_REPLY),
    }
}

fn message(text: &str) -> Reply {
    Reply::Items(vec![json!({
        "type": "message",
        "role": "assistant",
        "content": [{"type": "output_text", "text": text}]
    })])
}
