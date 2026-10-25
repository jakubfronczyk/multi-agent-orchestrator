//! `POST /api/drill` starts the seeded security drill as a task led by the Lead;
//! `GET /api/drill/{run_id}/report` lists every exit the task tried and who decided it.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use orchestrator_core::engine::StartRequest;
use orchestrator_core::ids::RunId;

use super::error::ApiError;
use super::runs::{StartRunResponse, start_error};
use super::{AppState, query};
use crate::config;
use crate::llm::DRILL;
use crate::sqlite::drill::{self, DrillReport};

/// Always scripted: it plays a model that obeys the planted note, so the gates are what stop it.
pub async fn start(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<StartRunResponse>), ApiError> {
    let run_id = state
        .engine
        .start(StartRequest {
            agent_id: None,
            task: config::DRILL_TASK.to_string(),
            title: Some(config::DRILL_TITLE.to_string()),
            budget: None,
            scenario: Some(DRILL.to_string()),
            draft_only: false,
        })
        .await
        .map_err(start_error)?;
    Ok((StatusCode::ACCEPTED, Json(StartRunResponse { run_id })))
}

pub async fn report(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DrillReport>, ApiError> {
    let root: RunId = query::path_id(&id)?;
    drill::report(&state.pool, root)
        .await?
        .map(Json)
        .ok_or_else(ApiError::not_found)
}
