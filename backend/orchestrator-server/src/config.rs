//! Server constants; the only environment inputs are `XAI_API_KEY` and `RUST_LOG`.
//! Paths are relative to the working directory the server starts in.

pub const DATA_DIR: &str = "var";
pub const DATABASE_FILE: &str = "var/orchestrator.db";
pub const ADDRESS: &str = "127.0.0.1:8787";
