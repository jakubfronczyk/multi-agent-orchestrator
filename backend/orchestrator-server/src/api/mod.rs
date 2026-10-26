//! The HTTP API under `/api`: JSON in and out, errors as `{error: {code, message, field?}}`.
//! Handlers read through `sqlite` and act through the engine.

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{get, post, put};
use orchestrator_core::engine::Engine;
use sqlx::SqlitePool;
use tower_http::trace::TraceLayer;

use crate::sqlite::SqliteStore;
use crate::tools::workspace::Workspace;

mod agents;
mod approvals;
mod drill;
mod error;
mod events;
mod files;
mod json;
mod memory;
mod meta;
mod origin;
mod outbox;
mod query;
mod runs;
mod teams;
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
        .route("/runs/{id}/messages", post(runs::follow_up))
        .route("/tool-calls", get(tool_calls::list))
        .route("/memory", get(memory::list).post(memory::create))
        .route("/memory/{id}", put(memory::edit).delete(memory::delete))
        .route("/memory/{id}/versions", get(memory::versions))
        .route("/memory/{id}/trust", post(memory::mark_trusted))
        .route("/outbox", get(outbox::list))
        .route(
            "/files",
            get(files::view)
                .post(files::upload)
                .put(files::write)
                .delete(files::remove)
                .layer(DefaultBodyLimit::max(files::UPLOAD_BYTES + 64 * 1024)),
        )
        .route("/files/trust", post(files::mark_trusted))
        .route("/drill", post(drill::start))
        .route("/drill/{id}/report", get(drill::report))
        .route("/approvals", get(approvals::list))
        .route("/approvals/{id}", post(approvals::answer))
        .route("/events", get(events::stream))
        .route("/teams", get(teams::list).post(teams::create))
        .route(
            "/teams/{id}",
            get(teams::get).put(teams::rename).delete(teams::delete),
        )
        .route("/teams/{id}/file-access", get(teams::file_access))
        .route(
            "/teams/{id}/file-access/{of_team_id}",
            put(teams::set_file_access),
        )
        .route(
            "/agents/{id}",
            get(agents::get).put(agents::replace).delete(agents::delete),
        );
    Router::new()
        .nest("/api", api)
        .layer(middleware::from_fn(origin::loopback_only))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
