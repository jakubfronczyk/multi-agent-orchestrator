//! The HTTP API under `/api`: JSON in and out, errors as `{error: {code, message, field?}}`.
//! Handlers write through `sqlite` and, from checkpoint A on, the engine.

use axum::Router;
use axum::routing::get;
use sqlx::SqlitePool;
use tower_http::trace::TraceLayer;

mod agents;
mod error;

pub use error::{ApiError, ErrorCode};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/agents", get(agents::list).post(agents::create))
        .route(
            "/agents/{id}",
            get(agents::get).put(agents::replace).delete(agents::delete),
        );
    Router::new()
        .nest("/api", api)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
