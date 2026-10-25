//! `POST /api/runs` starts a task and answers 202 with its root run id; progress arrives on SSE.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use orchestrator_core::engine::{StartError, StartRequest};
use orchestrator_core::ids::{AgentId, RunId};
use orchestrator_core::money::Micros;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::AppState;
use super::error::{ApiError, ApiJson};

#[derive(Debug, Deserialize, TS)]
#[ts(export, optional_fields)]
pub struct StartRunRequest {
    pub agent_id: Option<AgentId>,
    pub task: String,
    pub title: Option<String>,
    pub budget_micros: Option<Micros>,
    /// Scripted mode only: which fixed replies to play.
    pub scenario: Option<String>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct StartRunResponse {
    pub run_id: RunId,
}

pub async fn start(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<StartRunRequest>,
) -> Result<(StatusCode, Json<StartRunResponse>), ApiError> {
    let run_id = state
        .engine
        .start(StartRequest {
            agent_id: request.agent_id,
            task: request.task,
            title: request.title,
            budget: request.budget_micros,
            scenario: request.scenario,
        })
        .await
        .map_err(start_error)?;
    Ok((StatusCode::ACCEPTED, Json(StartRunResponse { run_id })))
}

fn start_error(error: StartError) -> ApiError {
    match error {
        StartError::NoLead => ApiError::invalid("agent_id", "the team has no Lead"),
        StartError::UnknownAgent => ApiError::invalid("agent_id", "no live agent has this id"),
        StartError::EmptyTask => ApiError::invalid("task", "must not be empty"),
        StartError::Store(error) => {
            tracing::error!(%error, "could not start the run");
            ApiError::internal()
        }
    }
}
