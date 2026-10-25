//! `GET /api/memory` lists notes with author and trust; `/memory/{id}/versions` shows a note's history.

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;

use super::AppState;
use super::error::ApiError;
use crate::sqlite::memory::{self, MemoryFilter, MemoryList, MemoryReadError, MemoryVersionList};
use crate::sqlite::text::from_text;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    scope: Option<String>,
    agent_id: Option<String>,
    q: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<MemoryList>, ApiError> {
    let filter = MemoryFilter {
        scope: query
            .scope
            .map(|raw| {
                from_text(&raw).map_err(|_| ApiError::invalid("scope", "unknown memory scope"))
            })
            .transpose()?,
        agent_id: query
            .agent_id
            .map(|id| {
                id.parse()
                    .map_err(|_| ApiError::invalid("agent_id", "not an agent id"))
            })
            .transpose()?,
        q: query.q.filter(|q| !q.is_empty()),
    };
    Ok(Json(
        memory::list(&state.pool, filter)
            .await
            .map_err(read_error)?,
    ))
}

pub async fn versions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MemoryVersionList>, ApiError> {
    memory::versions(&state.pool, &id)
        .await
        .map_err(read_error)?
        .map(Json)
        .ok_or_else(ApiError::not_found)
}

fn read_error(error: MemoryReadError) -> ApiError {
    tracing::error!(%error, "memory read failed");
    ApiError::internal()
}
