//! `GET /api/outbox`: what `send_message` sent in the mock world.

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use super::AppState;
use super::error::ApiError;
use super::query;
use crate::sqlite::outbox::{self, OutboxFilter, OutboxList};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    team_id: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<OutboxList>, ApiError> {
    let filter = OutboxFilter {
        team_id: query::id("team_id", query.team_id)?,
    };
    Ok(Json(outbox::list(&state.pool, filter).await?))
}
