//! The HTTP API under `/api`: JSON in and out, errors as `{error: {code, message, field?}}`.
//! Handlers write through `sqlite` and, from checkpoint A on, the engine.

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use orchestrator_core::engine::Engine;
use sqlx::SqlitePool;

use crate::sqlite::SqliteStore;
use tower_http::trace::TraceLayer;

mod agents;
mod error;
mod events;
mod json;
mod runs;

pub use error::{ApiError, ErrorCode};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub store: Arc<SqliteStore>,
    pub engine: Arc<Engine>,
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/agents", get(agents::list).post(agents::create))
        .route("/runs", post(runs::start))
        .route("/events", get(events::stream))
        .route(
            "/agents/{id}",
            get(agents::get).put(agents::replace).delete(agents::delete),
        );
    Router::new()
        .nest("/api", api)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
