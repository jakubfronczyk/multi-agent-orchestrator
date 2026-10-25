//! Reads function calls and text out of raw Responses-API output items.
//! Items it does not read (reasoning, unknown types) still go back to the model verbatim.

use orchestrator_core::llm::{FnCall, LlmError};
use serde_json::Value;

pub fn parse_output(raw: &[Value]) -> Result<(Vec<FnCall>, Option<String>), LlmError> {
    let mut calls = Vec::new();
    let mut texts = Vec::new();
    for item in raw {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => calls.push(FnCall {
                call_id: string_field(item, "call_id")?,
                name: string_field(item, "name")?,
                arguments: string_field(item, "arguments")?,
            }),
            Some("message") => {
                let parts = item.get("content").and_then(Value::as_array);
                for part in parts.into_iter().flatten() {
                    if part.get("type").and_then(Value::as_str) == Some("output_text")
                        && let Some(text) = part.get("text").and_then(Value::as_str)
                    {
                        texts.push(text.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    let text = (!texts.is_empty()).then(|| texts.concat());
    Ok((calls, text))
}

fn string_field(item: &Value, name: &str) -> Result<String, LlmError> {
    item.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| LlmError::BadOutput(format!("function_call without a string {name}")))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_calls_and_text_and_skips_other_items() {
        let raw = vec![
            json!({"type": "reasoning", "encrypted_content": "opaque"}),
            json!({"type": "message", "content": [{"type": "output_text", "text": "Hel"}, {"type": "output_text", "text": "lo"}]}),
            json!({"type": "function_call", "call_id": "call-1", "name": "plan", "arguments": "{\"steps\":[]}"}),
        ];
        let (calls, text) = parse_output(&raw).expect("parsed");
        assert_eq!(text.as_deref(), Some("Hello"));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "plan");
        assert_eq!(calls[0].arguments, "{\"steps\":[]}");
    }

    #[test]
    fn a_call_without_arguments_is_bad_output() {
        let raw = vec![json!({"type": "function_call", "call_id": "c", "name": "plan"})];
        assert!(matches!(parse_output(&raw), Err(LlmError::BadOutput(_))));
    }
}
