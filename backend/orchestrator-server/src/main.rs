//! Server entry point: opens the database and starts the server.
//! All behaviour lives in the library so integration tests can reach it.

use orchestrator_server::{config, sqlite};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    std::fs::create_dir_all(config::DATA_DIR)?;
    let pool = sqlite::connect(config::DATABASE_FILE).await?;
    let _store = sqlite::SqliteStore::new(pool);
    println!("database ready at {}", config::DATABASE_FILE);
    Ok(())
}
