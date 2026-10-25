//! `GET /api/memory` lists notes with author and trust; `/memory/{id}/versions` shows a note's history.

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;

use super::AppState;
use super::error::ApiError;
use super::query;
use crate::sqlite::memory::{self, MemoryFilter, MemoryList, MemoryVersionList};

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
        scope: query::value("scope", query.scope)?,
        agent_id: query::id("agent_id", query.agent_id)?,
        q: query.q.filter(|q| !q.is_empty()),
    };
    Ok(Json(memory::list(&state.pool, filter).await?))
}

pub async fn versions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MemoryVersionList>, ApiError> {
    memory::versions(&state.pool, &id)
        .await?
        .map(Json)
        .ok_or_else(ApiError::not_found)
}
