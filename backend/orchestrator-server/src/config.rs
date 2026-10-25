//! Server constants; the only environment inputs are `XAI_API_KEY` and `RUST_LOG`.
//! Paths are relative to the working directory the server starts in.

use std::time::Duration;

use orchestrator_core::money::Micros;

pub const DATA_DIR: &str = "var";
pub const DATABASE_FILE: &str = "var/orchestrator.db";
/// The only directory file tools can reach.
pub const WORKSPACE_DIR: &str = "var/workspace";
/// The one configuration variable; unset means the scripted demo provider.
pub const XAI_KEY_VAR: &str = "XAI_API_KEY";
pub const ADDRESS: &str = "127.0.0.1:8787";

pub fn engine_config() -> orchestrator_core::engine::Config {
    orchestrator_core::engine::Config {
        llm_concurrency: 4,
        llm_timeout: Duration::from_secs(120),
        // Long bases because xAI sends no Retry-After and a capped key refills per minute.
        retry_backoff: vec![
            Duration::from_secs(5),
            Duration::from_secs(10),
            Duration::from_secs(20),
            Duration::from_secs(40),
        ],
        default_budget: Micros(500_000),
        approval_timeout: Duration::from_secs(300),
        // Plain hosts; each also covers its subdomains. A `*.example.com` entry never matches.
        allowlist: vec!["example.com".to_string()],
        max_depth: 2,
        max_runs: 12,
    }
}
