//! `/api/agents`: list, create, read, full replace and soft delete.
//! Every rule lives in `AgentSpec::validated` and the storage transaction, not here.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use orchestrator_core::agents::AgentInput;
use orchestrator_core::ids::AgentId;

use super::AppState;
use super::error::ApiError;
use super::json::ApiJson;
use crate::sqlite::agents::{self, AgentRecord};

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<AgentRecord>>, ApiError> {
    Ok(Json(agents::list(&state.pool).await?))
}

pub async fn create(
    State(state): State<AppState>,
    ApiJson(input): ApiJson<AgentInput>,
) -> Result<(StatusCode, Json<AgentRecord>), ApiError> {
    let record = agents::create(&state.pool, input).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<AgentRecord>, ApiError> {
    Ok(Json(agents::get(&state.pool, agent_id(&id)?).await?))
}

pub async fn replace(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<AgentInput>,
) -> Result<Json<AgentRecord>, ApiError> {
    Ok(Json(
        agents::replace(&state.pool, agent_id(&id)?, input).await?,
    ))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    agents::delete(&state.pool, agent_id(&id)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A malformed id cannot name an agent, so it is a 404 like an unknown one.
fn agent_id(raw: &str) -> Result<AgentId, ApiError> {
    raw.parse().map_err(|_| ApiError::not_found())
}
