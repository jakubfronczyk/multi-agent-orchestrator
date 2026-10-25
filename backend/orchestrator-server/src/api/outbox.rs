//! `GET /api/outbox`: what `send_message` sent in the mock world.

use axum::Json;
use axum::extract::State;

use super::AppState;
use super::error::ApiError;
use crate::sqlite::outbox::{self, OutboxList};

pub async fn list(State(state): State<AppState>) -> Result<Json<OutboxList>, ApiError> {
    outbox::list(&state.pool).await.map(Json).map_err(|error| {
        tracing::error!(%error, "outbox read failed");
        ApiError::internal()
    })
}
