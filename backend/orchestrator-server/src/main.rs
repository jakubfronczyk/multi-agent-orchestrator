//! Server entry point: opens the database and serves the API on loopback.
//! All behaviour lives in the library so integration tests can reach it.

use std::path::PathBuf;
use std::sync::Arc;

use orchestrator_core::engine::Engine;
use orchestrator_core::llm::LlmProvider;
use orchestrator_server::api::{self, AppState};
use orchestrator_server::llm::{ScriptedProvider, XaiProvider};
use orchestrator_server::sqlite::SqliteStore;
use orchestrator_server::{config, seed, sqlite, tools};
use secrecy::SecretString;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // A missing .env is fine; the environment may already hold the variables.
    dotenvy::dotenv().ok();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    std::fs::create_dir_all(config::WORKSPACE_DIR)?;
    let pool = sqlite::connect(config::DATABASE_FILE).await?;
    let workspace = PathBuf::from(config::WORKSPACE_DIR);
    if seed::run(&pool, &workspace).await? {
        tracing::info!("seeded the team, the inbox and the workspace");
    }
    let store = Arc::new(SqliteStore::new(pool.clone()));
    let llm = provider().await?;
    let engine = Arc::new(Engine::new(
        store.clone(),
        llm,
        tools::all(pool.clone(), workspace),
        config::engine_config(),
    ));
    let listener = tokio::net::TcpListener::bind(config::ADDRESS).await?;
    tracing::info!(address = config::ADDRESS, "listening");
    axum::serve(
        listener,
        api::router(AppState {
            pool,
            store,
            engine,
        }),
    )
    .await?;
    Ok(())
}

/// The active provider is never silent: xAI when `XAI_API_KEY` is set and accepted, the
/// scripted demo otherwise. A key xAI rejects stops the boot instead of falling back.
async fn provider() -> anyhow::Result<Arc<dyn LlmProvider>> {
    match std::env::var(config::XAI_KEY_VAR) {
        Ok(key) if !key.trim().is_empty() => {
            let xai = XaiProvider::new(SecretString::from(key)).map_err(anyhow::Error::msg)?;
            xai.check().await.map_err(anyhow::Error::msg)?;
            tracing::info!("provider: xai");
            Ok(Arc::new(xai))
        }
        Ok(_) | Err(_) => {
            tracing::info!("provider: scripted (demo mode: scripted replies)");
            Ok(Arc::new(ScriptedProvider::new()))
        }
    }
}
