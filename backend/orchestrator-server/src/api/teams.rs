//! `/api/teams`: list, create (with its Lead), read, rename and delete, and which other teams'
//! folders a team may open. No event is emitted: a team is configuration, like an agent.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use orchestrator_core::ids::TeamId;
use serde::Deserialize;

use super::AppState;
use super::error::ApiError;
use super::json::ApiJson;
use super::query;
use crate::sqlite::teams::{
    self, FileAccessInput, FileAccessList, TeamError, TeamInput, TeamList, TeamView,
};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    include_deleted: Option<String>,
}

/// Live teams, oldest first; `include_deleted=true` adds deleted ones.
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<TeamList>, ApiError> {
    let include_deleted = query::flag("include_deleted", query.include_deleted)?;
    Ok(Json(teams::list(&state.pool, include_deleted).await?))
}

pub async fn create(
    State(state): State<AppState>,
    ApiJson(input): ApiJson<TeamInput>,
) -> Result<(StatusCode, Json<TeamView>), ApiError> {
    let team = teams::create(&state.pool, input).await?;
    Ok((StatusCode::CREATED, Json(team)))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TeamView>, ApiError> {
    Ok(Json(
        teams::get(&state.pool, query::path_id::<TeamId>(&id)?).await?,
    ))
}

pub async fn rename(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<TeamInput>,
) -> Result<Json<TeamView>, ApiError> {
    let team = query::path_id::<TeamId>(&id)?;
    Ok(Json(teams::rename(&state.pool, team, input).await?))
}

/// Every other live team with how much of its folder this team's agents may open.
pub async fn file_access(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<FileAccessList>, ApiError> {
    let team = query::path_id::<TeamId>(&id)?;
    Ok(Json(teams::file_access(&state.pool, team).await?))
}

/// 204, applied at a running task's next file call; 404 for an unknown or deleted team on
/// either side, 422 for the team itself.
pub async fn set_file_access(
    State(state): State<AppState>,
    Path((id, of)): Path<(String, String)>,
    ApiJson(input): ApiJson<FileAccessInput>,
) -> Result<StatusCode, ApiError> {
    let team = query::path_id::<TeamId>(&id)?;
    let of = query::path_id::<TeamId>(&of)?;
    // No file write is between its reach check and its bytes while the grant changes.
    let _files = state.workspace.exclusive().await;
    teams::set_file_access(&state.pool, team, of, input.access).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 204; 409 while a task of the team is live (waiting on a card counts) or when it is the last.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let team = query::path_id::<TeamId>(&id)?;
    // No file write into the team's folder is between its reach check and its bytes.
    let _files = state.workspace.exclusive().await;
    teams::delete(&state.pool, team, || state.engine.has_live_task(team)).await?;
    Ok(StatusCode::NO_CONTENT)
}

impl From<TeamError> for ApiError {
    fn from(error: TeamError) -> Self {
        match error {
            TeamError::Invalid(invalid) => ApiError::invalid(&invalid.field, &invalid.message),
            TeamError::NotFound => ApiError::not_found(),
            TeamError::Conflict(message) => ApiError::conflict(message),
            TeamError::Agent(error) => error.into(),
            TeamError::Storage(error) => {
                tracing::error!(%error, "team storage failed");
                ApiError::internal()
            }
        }
    }
}
