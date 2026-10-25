//! The HTTP API under `/api`: JSON in and out, errors as `{error: {code, message, field?}}`.
//! Handlers write through `sqlite` and, from checkpoint A on, the engine.

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use orchestrator_core::engine::Engine;
use sqlx::SqlitePool;

use crate::sqlite::SqliteStore;
use crate::tools::files::Workspace;
use tower_http::trace::TraceLayer;

mod agents;
mod approvals;
mod drill;
mod error;
mod events;
mod files;
mod json;
mod memory;
mod meta;
mod outbox;
mod runs;
mod tool_calls;

pub use error::{ApiError, ErrorCode};
pub use meta::ActiveProvider;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub store: Arc<SqliteStore>,
    pub engine: Arc<Engine>,
    pub provider: ActiveProvider,
    /// Shared with the file tools, so the file view takes the same lock.
    pub workspace: Workspace,
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/agents", get(agents::list).post(agents::create))
        .route("/health", get(meta::health))
        .route("/models", get(meta::models))
        .route("/tools", get(meta::tools))
        .route("/runs", get(runs::list).post(runs::start))
        .route("/runs/{id}", get(runs::get))
        .route("/runs/{id}/cancel", post(runs::cancel))
        .route("/tool-calls", get(tool_calls::list))
        .route("/memory", get(memory::list))
        .route("/memory/{id}/versions", get(memory::versions))
        .route("/outbox", get(outbox::list))
        .route("/files", get(files::view))
        .route("/drill", post(drill::start))
        .route("/drill/{id}/report", get(drill::report))
        .route("/approvals", get(approvals::list))
        .route("/approvals/{id}", post(approvals::answer))
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
