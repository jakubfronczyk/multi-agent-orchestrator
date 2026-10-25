//! The one live test: real Grok calls, skipped unless `XAI_API_KEY` is set. It sends every
//! tool schema at once, so a schema keyword xAI rejects fails here first.

use std::collections::BTreeMap;
use std::sync::Arc;

use orchestrator_core::agents::{AgentInput, AgentSpec, Role};
use orchestrator_core::engine::{delegate_spec, plan_spec};
use orchestrator_core::ids::{AgentId, RunId};
use orchestrator_core::llm::{
    CompletionRequest, Item, LlmProvider, Model, ReasoningEffort, ScriptKey, ToolChoice,
};
use orchestrator_core::tools::ToolName;
use orchestrator_server::llm::XaiProvider;
use orchestrator_server::tools;
use orchestrator_server::tools::workspace::Workspace;
use secrecy::SecretString;
use sqlx::SqlitePool;

use crate::TempDir;

fn key() -> Option<SecretString> {
    std::env::var("XAI_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .map(SecretString::from)
}

fn request(
    tools: Vec<orchestrator_core::tools::ToolSpec>,
    transcript: Vec<Item>,
) -> CompletionRequest {
    CompletionRequest {
        model: Model::Grok43,
        reasoning_effort: Some(ReasoningEffort::Low),
        instructions: "You are a test agent. Follow the user's instruction exactly.".to_string(),
        transcript,
        tools,
        tool_choice: ToolChoice::Auto,
        script: ScriptKey {
            root_run_id: RunId::new(),
            run_id: RunId::new(),
            agent: orchestrator_core::agents::Slug::new("lead").expect("slug"),
            step: 0,
            scenario: None,
        },
    }
}

fn reader() -> Arc<AgentSpec> {
    let input = AgentInput {
        role: Role::Specialist,
        slug: "reader".to_string(),
        name: "Reader".to_string(),
        description: "Reads files and reports.".to_string(),
        system_prompt: "Read and report.".to_string(),
        model: Model::Grok43,
        reasoning_effort: None,
        icon: "i".to_string(),
        colour: "c".to_string(),
        tools: BTreeMap::new(),
        delegate_targets: Vec::new(),
    };
    Arc::new(AgentSpec::validated(AgentId::new(), 1, input).expect("valid"))
}

#[sqlx::test]
async fn live_grok_accepts_every_schema_and_echoes_a_two_turn_exchange(pool: SqlitePool) {
    let Some(key) = key() else {
        eprintln!("skipped: XAI_API_KEY is not set");
        return;
    };
    let xai = XaiProvider::new(key).expect("client");
    xai.check()
        .await
        .expect("the key and both models are accepted");

    let temp = TempDir::new();
    let set = tools::all(pool.clone(), Workspace::new(temp.workspace(), pool));
    let mut every = vec![delegate_spec(&[reader()]), plan_spec()];
    for name in [
        ToolName::MemoryRead,
        ToolName::MemorySearch,
        ToolName::MemoryWrite,
        ToolName::FileList,
        ToolName::FileRead,
        ToolName::FileWrite,
        ToolName::InboxList,
        ToolName::InboxRead,
        ToolName::HttpFetch,
        ToolName::CsvQuery,
        ToolName::SendMessage,
    ] {
        every.push(set.get(name).expect("real tool").spec().clone());
    }
    assert_eq!(every.len(), 13);
    let quiet = vec![Item::User {
        text: "Do not call any tool. Reply with the single word: ready".to_string(),
    }];
    let first = xai
        .complete(request(every, quiet))
        .await
        .expect("all 13 schemas are accepted");
    assert!(first.cost_micros.0 > 0, "the reply carries a cost");

    // Two turns: Grok calls a tool, gets its output, and answers; its raw items go back verbatim.
    let memory_read = set.get(ToolName::MemoryRead).expect("tool").spec().clone();
    let ask = Item::User {
        text: "Call memory_read with scope shared and key status, then tell me what it returned."
            .to_string(),
    };
    let called = xai
        .complete(request(vec![memory_read.clone()], vec![ask.clone()]))
        .await
        .expect("first turn");
    let call = called
        .calls
        .first()
        .expect("Grok called memory_read")
        .clone();
    assert_eq!(call.name, "memory_read");

    let mut transcript = vec![ask];
    transcript.extend(called.raw.into_iter().map(Item::Provider));
    transcript.push(Item::ToolOutput {
        call_id: call.call_id,
        output: r#"{"ok":true,"result":{"found":false,"key":"status"}}"#.to_string(),
    });
    let answered = xai
        .complete(request(vec![memory_read], transcript))
        .await
        .expect("second turn with echoed items");
    assert!(answered.text.is_some() || !answered.calls.is_empty());
}
