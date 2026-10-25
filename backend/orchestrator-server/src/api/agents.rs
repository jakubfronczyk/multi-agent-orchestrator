//! `/api/agents`: list, create, read, full replace and soft delete.
//! Every rule lives in `AgentSpec::validated` and the storage transaction, not here.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use orchestrator_core::agents::AgentInput;
use orchestrator_core::ids::{AgentId, TeamId};
use serde::Deserialize;

use super::AppState;
use super::error::ApiError;
use super::json::ApiJson;
use super::query;
use crate::sqlite::agents::{self, AgentRecord};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    team_id: Option<String>,
}

/// Live agents, oldest first; `team_id` narrows to one team.
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<AgentRecord>>, ApiError> {
    let team = query::id::<TeamId>("team_id", query.team_id)?;
    Ok(Json(agents::list(&state.pool, team).await?))
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
    Ok(Json(
        agents::get(&state.pool, query::path_id::<AgentId>(&id)?).await?,
    ))
}

pub async fn replace(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<AgentInput>,
) -> Result<Json<AgentRecord>, ApiError> {
    Ok(Json(
        agents::replace(&state.pool, query::path_id::<AgentId>(&id)?, input).await?,
    ))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    agents::delete(&state.pool, query::path_id::<AgentId>(&id)?).await?;
    Ok(StatusCode::NO_CONTENT)
}
