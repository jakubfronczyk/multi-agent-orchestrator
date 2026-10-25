//! SQLite storage: the connection pool, the `Store` implementation and text encoding.
//! One file in WAL mode, migrated at boot from the embedded migrations.

use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{Sqlite, SqlitePool, Transaction};

pub mod agents;
pub mod approvals;
pub mod memory;
pub mod outbox;
pub mod runs;
mod store;
pub(crate) mod text;

pub use store::SqliteStore;

pub async fn connect(path: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePool::connect_with(options).await?;
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}

/// Takes the write lock before any read, so checks and writes see the same state and a
/// waiting writer is retried by `busy_timeout` instead of failing on a stale snapshot.
pub(crate) async fn begin_write(
    pool: &SqlitePool,
) -> Result<Transaction<'static, Sqlite>, sqlx::Error> {
    pool.begin_with("BEGIN IMMEDIATE").await
}
