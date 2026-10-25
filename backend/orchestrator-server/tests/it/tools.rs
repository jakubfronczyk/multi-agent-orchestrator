//! Test tools: each borrows a real tool name and behaves in one fixed way.
//! The name decides the effect (`ToolName::effect`); the body decides the outcome.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orchestrator_core::tools::{
    Action, Target, Tool, ToolCtx, ToolError, ToolName, ToolOutput, ToolSet, ToolSpec,
};
use serde_json::{Value, json};

pub const TIMEOUT: Duration = Duration::from_millis(20);

#[derive(Clone, Copy)]
enum Behaviour {
    Echo,
    Hang,
    Panic,
    Fail,
}

struct TestTool {
    spec: ToolSpec,
    behaviour: Behaviour,
}

#[async_trait]
impl Tool for TestTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    /// Arguments must be a JSON object; anything else is invalid.
    fn check(&self, args: &Value) -> Result<Action, ToolError> {
        if !args.is_object() {
            return Err(ToolError::InvalidArgs(
                "arguments must be an object".to_string(),
            ));
        }
        Ok(Action {
            verb: "test".to_string(),
            target: Target::Query {
                text: args.to_string(),
            },
        })
    }

    async fn run(&self, _ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        match self.behaviour {
            Behaviour::Echo => Ok(ToolOutput {
                content: json!({ "echo": args }),
            }),
            Behaviour::Hang => std::future::pending().await,
            Behaviour::Panic => panic!("test tool panicked"),
            Behaviour::Fail => Err(ToolError::Failed("test tool failed".to_string())),
        }
    }
}

fn tool(name: ToolName, behaviour: Behaviour) -> Arc<dyn Tool> {
    Arc::new(TestTool {
        spec: ToolSpec {
            name,
            description: "test tool".to_string(),
            schema: json!({"type": "object"}),
            timeout: Some(TIMEOUT),
        },
        behaviour,
    })
}

/// memory_read echoes, file_write echoes (Write effect, for ask), csv_query hangs past its
/// timeout, file_list panics, file_read fails, send_message echoes (External, set to deny).
pub fn all() -> ToolSet {
    ToolSet::new(vec![
        tool(ToolName::MemoryRead, Behaviour::Echo),
        tool(ToolName::FileWrite, Behaviour::Echo),
        tool(ToolName::CsvQuery, Behaviour::Hang),
        tool(ToolName::FileList, Behaviour::Panic),
        tool(ToolName::FileRead, Behaviour::Fail),
        tool(ToolName::SendMessage, Behaviour::Echo),
    ])
}
