//! Server library: HTTP API, SQLite storage, the tools and the xAI client.
//! It implements the traits orchestrator-core declares; `main.rs` only wires and starts it.

pub mod api;
pub mod config;
pub mod events;
pub mod llm;
pub mod recovery;
pub mod seed;
pub mod sqlite;
pub mod tools;
