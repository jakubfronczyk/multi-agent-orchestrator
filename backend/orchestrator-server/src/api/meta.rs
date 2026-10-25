//! Fixed facts the UI reads once: the active provider, the models and the 13 tools.

use axum::Json;
use axum::extract::State;
use orchestrator_core::llm::{Model, ReasoningEffort};
use orchestrator_core::policy::{Effect, LeadAccess};
use orchestrator_core::tools::ToolName;
use serde::Serialize;
use ts_rs::TS;

use super::AppState;

/// Which provider answers model calls; `Scripted` is the no-key demo mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ActiveProvider {
    Xai,
    Scripted,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct Health {
    /// Who answers free-text tasks.
    pub provider: ActiveProvider,
    /// Who answers tasks started with a scenario (s1, clean, launder, drill): always scripted.
    pub scenarios: ActiveProvider,
}

/// No prices: cost comes from each xAI reply, not from a table (§11).
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ModelInfo {
    pub id: Model,
    /// Empty when the model takes no reasoning effort.
    pub efforts: Vec<ReasoningEffort>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ModelList {
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ToolInfo {
    pub name: ToolName,
    pub description: String,
    pub effect: Effect,
    /// Whether its output is outside content that taints the task.
    pub taints: bool,
    pub lead: LeadAccess,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ToolList {
    pub tools: Vec<ToolInfo>,
}

pub async fn health(State(state): State<AppState>) -> Json<Health> {
    Json(Health {
        provider: state.provider,
        scenarios: ActiveProvider::Scripted,
    })
}

pub async fn models() -> Json<ModelList> {
    let models = Model::ALL
        .into_iter()
        .map(|id| ModelInfo {
            id,
            efforts: id.efforts().to_vec(),
        })
        .collect();
    Json(ModelList { models })
}

pub async fn tools(State(state): State<AppState>) -> Json<ToolList> {
    let tools = state
        .engine
        .tool_specs()
        .into_iter()
        .map(|spec| ToolInfo {
            name: spec.name,
            description: spec.description,
            effect: spec.name.effect(),
            taints: spec.name.taints(),
            lead: spec.name.lead(),
        })
        .collect();
    Json(ToolList { tools })
}
