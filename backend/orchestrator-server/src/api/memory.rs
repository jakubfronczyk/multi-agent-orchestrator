//! `GET /api/memory` lists notes with author and trust; `/memory/{id}/versions` shows a note's
//! history. The person creates, edits, deletes and marks shared notes as trusted here; each write
//! names the version it saw, and a newer one answers 409.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use orchestrator_core::ids::TeamId;
use serde::Deserialize;
use ts_rs::TS;

use super::AppState;
use super::error::ApiError;
use super::json::ApiJson;
use super::query;
use crate::sqlite::memory::{
    self, MemoryEntryView, MemoryFilter, MemoryList, MemoryVersionList, NoteError,
};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    scope: Option<String>,
    team_id: Option<String>,
    agent_id: Option<String>,
    q: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<MemoryList>, ApiError> {
    let filter = MemoryFilter {
        scope: query::value("scope", query.scope)?,
        team_id: query::id("team_id", query.team_id)?,
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

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct NoteCreate {
    pub team_id: TeamId,
    pub key: String,
    pub content: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct NoteEdit {
    pub content: String,
    pub expected_version: i64,
}

/// The version the person saw, for a delete or a mark.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct NoteVersion {
    pub expected_version: i64,
}

#[derive(Debug, Deserialize)]
pub struct DeleteQuery {
    expected_version: Option<String>,
}

/// 201, a shared note in the person's words, trusted. 404 for an unknown or deleted team, 409 when
/// the team already has a live note with the key, 422 for a bad key or content.
pub async fn create(
    State(state): State<AppState>,
    ApiJson(input): ApiJson<NoteCreate>,
) -> Result<(StatusCode, Json<MemoryEntryView>), ApiError> {
    let note = memory::create(&state.pool, input.team_id, &input.key, &input.content).await?;
    Ok((StatusCode::CREATED, Json(note)))
}

/// A new version in the person's words; the label stays as it was.
pub async fn edit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<NoteEdit>,
) -> Result<Json<MemoryEntryView>, ApiError> {
    Ok(Json(
        memory::edit(&state.pool, &id, &input.content, input.expected_version).await?,
    ))
}

/// 204; the note is hidden from every reader and its versions stay.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DeleteQuery>,
) -> Result<StatusCode, ApiError> {
    let expected = query
        .expected_version
        .and_then(|raw| raw.parse::<i64>().ok())
        .ok_or_else(|| ApiError::invalid("expected_version", "name the version you saw"))?;
    memory::delete(&state.pool, &id, expected).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The person vouches for the note: a new version, same text, trusted. Tasks already tainted by
/// it stay tainted.
pub async fn mark_trusted(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<NoteVersion>,
) -> Result<Json<MemoryEntryView>, ApiError> {
    Ok(Json(
        memory::mark_trusted(&state.pool, &id, input.expected_version).await?,
    ))
}

impl From<NoteError> for ApiError {
    fn from(error: NoteError) -> Self {
        match error {
            NoteError::Invalid { field, message } => ApiError::invalid(field, &message),
            NoteError::NotFound => ApiError::not_found(),
            NoteError::Stale => ApiError::conflict("the note changed since you opened it"),
            NoteError::Conflict(message) => ApiError::conflict(message),
            NoteError::Read(error) => error.into(),
            NoteError::Storage(error) => {
                tracing::error!(%error, "note storage failed");
                ApiError::internal()
            }
        }
    }
}
