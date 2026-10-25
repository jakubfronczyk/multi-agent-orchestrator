//! Server entry point: opens the database and serves the API on loopback.
//! All behaviour lives in the library so integration tests can reach it.

use orchestrator_server::api::{self, AppState};
use orchestrator_server::{config, sqlite};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    std::fs::create_dir_all(config::DATA_DIR)?;
    let pool = sqlite::connect(config::DATABASE_FILE).await?;
    let listener = tokio::net::TcpListener::bind(config::ADDRESS).await?;
    tracing::info!(address = config::ADDRESS, "listening");
    axum::serve(listener, api::router(AppState { pool })).await?;
    Ok(())
}
