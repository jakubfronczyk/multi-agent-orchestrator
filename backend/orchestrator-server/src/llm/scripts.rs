//! Fixed replies for the scripted provider, keyed by scenario, step and attempt.
//! Replies use the xAI output-item shape so they pass through the same parser.

use orchestrator_core::llm::{LlmError, ToolChoice};
use serde_json::{Value, json};

pub const DEMO_REPLY: &str = "Demo mode: try one of the example tasks.";

pub enum Reply {
    Items(Vec<Value>),
    Fail(LlmError),
    /// Never answers; only cancellation ends the call.
    Hang,
}

/// `test:call:<tool>:<json args>` calls once, then answers. `test:loop:<tool>` calls on every
/// step it may. In `test:delegate*` the Lead delegates on its first step and teammates answer
/// with their slug. With `tool_choice` none, every script answers in text, as xAI does.
pub fn reply(
    scenario: Option<&str>,
    agent: &str,
    step: u32,
    attempt: u32,
    tool_choice: ToolChoice,
) -> Reply {
    if tool_choice == ToolChoice::None {
        return message("out of steps");
    }
    if let Some(reply) = delegation(scenario, agent, step) {
        return reply;
    }
    if let Some((tool, args)) = scenario
        .and_then(|s| s.strip_prefix("test:call:"))
        .and_then(|rest| rest.split_once(':'))
    {
        return if step == 0 {
            function_call(step, tool, args)
        } else {
            message("done")
        };
    }
    if let Some(tool) = scenario.and_then(|s| s.strip_prefix("test:loop:")) {
        return function_call(step, tool, "{}");
    }
    match scenario {
        Some("test:retry") if attempt == 0 => {
            Reply::Fail(LlmError::Server("scripted 503".to_string()))
        }
        Some("test:retry") => message("recovered"),
        Some("test:fatal") => Reply::Fail(LlmError::Rejected("scripted 400".to_string())),
        Some("test:hang") => Reply::Hang,
        Some("test:hello") => message("hello"),
        _ => message(DEMO_REPLY),
    }
}

fn delegation(scenario: Option<&str>, agent: &str, step: u32) -> Option<Reply> {
    let scenario = scenario.filter(|s| s.starts_with("test:delegate") || *s == "test:plan")?;
    if agent != "lead" {
        return Some(match scenario {
            "test:delegate-fail" => Reply::Fail(LlmError::Rejected("scripted 400".to_string())),
            "test:delegate-hang" => Reply::Hang,
            _ => message(&format!("from {agent}")),
        });
    }
    if step > 0 {
        return Some(message("done"));
    }
    let call = |n: usize, tool: &str, args: Value| json!({"type": "function_call", "call_id": format!("call-0-{n}"), "name": tool, "arguments": args.to_string()});
    let delegate = |n: usize, to: &str| {
        call(
            n,
            "delegate",
            json!({"agent": to, "task": format!("help {to}")}),
        )
    };
    Some(Reply::Items(match scenario {
        "test:delegate-two" => vec![delegate(0, "helper"), delegate(1, "helper2")],
        "test:plan" => vec![call(
            0,
            "plan",
            json!({"steps": [{"agent": "helper", "goal": "look"}, {"agent": "self", "goal": "sum up"}]}),
        )],
        _ => vec![delegate(0, "helper")],
    }))
}

fn function_call(step: u32, tool: &str, args: &str) -> Reply {
    Reply::Items(vec![json!({
        "type": "function_call",
        "call_id": format!("call-{step}"),
        "name": tool,
        "arguments": args
    })])
}

fn message(text: &str) -> Reply {
    Reply::Items(vec![json!({
        "type": "message",
        "role": "assistant",
        "content": [{"type": "output_text", "text": text}]
    })])
}
