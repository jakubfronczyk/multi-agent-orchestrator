//! Server constants; the only environment inputs are `XAI_API_KEY` and `RUST_LOG`.
//! Paths are relative to the working directory the server starts in.

use std::time::Duration;

use orchestrator_core::money::Micros;

pub const DATA_DIR: &str = "var";
pub const DATABASE_FILE: &str = "var/orchestrator.db";
/// The only directory file tools can reach.
pub const WORKSPACE_DIR: &str = "var/workspace";
pub const ADDRESS: &str = "127.0.0.1:8787";

pub fn engine_config() -> orchestrator_core::engine::Config {
    orchestrator_core::engine::Config {
        llm_concurrency: 4,
        llm_timeout: Duration::from_secs(120),
        retry_backoff: vec![
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(4),
        ],
        default_budget: Micros(500_000),
        approval_timeout: Duration::from_secs(300),
        allowlist: Vec::new(),
        max_depth: 2,
        max_runs: 12,
    }
}
