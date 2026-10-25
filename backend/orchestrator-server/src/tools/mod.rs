//! The server's tools. Each gets its database pool, paths or HTTP client through its
//! constructor; core sees only the `Tool` trait and the `ToolSpec`.

use std::sync::Arc;
use std::time::Duration;

use orchestrator_core::policy::Trust;
use orchestrator_core::tools::{MAY_ASK, Tool, ToolError, ToolName, ToolSet, ToolSpec, schema_for};
use schemars::JsonSchema;
use sqlx::SqlitePool;

use crate::sqlite::text::from_text;

pub mod csv;
pub mod fetch;
pub mod files;
pub mod memory;
pub mod messages;
mod sandbox;

/// The eleven server tools; `delegate` and `plan` are run by the engine itself.
/// `workspace` is shared with the API's file view, so both take the same lock.
pub fn all(pool: SqlitePool, workspace: files::Workspace) -> ToolSet {
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(memory::MemoryRead::new(pool.clone())),
        Arc::new(memory::MemorySearch::new(pool.clone())),
        Arc::new(memory::MemoryWrite::new(pool.clone())),
        Arc::new(files::FileList::new(workspace.clone())),
        Arc::new(files::FileRead::new(workspace.clone())),
        Arc::new(files::FileWrite::new(workspace.clone())),
        Arc::new(csv::CsvQuery::new(workspace)),
        Arc::new(messages::InboxList::new(pool.clone())),
        Arc::new(messages::InboxRead::new(pool.clone())),
        Arc::new(messages::SendMessage::new(pool)),
        Arc::new(fetch::HttpFetch::new()),
    ];
    ToolSet::new(tools)
}

/// The schema comes from the argument struct; its field docs are what the model reads.
pub(crate) fn spec<T: JsonSchema>(name: ToolName, purpose: &str, timeout: Duration) -> ToolSpec {
    ToolSpec {
        name,
        description: format!("{purpose} {MAY_ASK}"),
        schema: schema_for::<T>(),
        timeout: Some(timeout),
    }
}

/// Whether stored data counts as trusted. A missing row or an unknown value counts as
/// untrusted, so a bad row fails closed.
pub(crate) fn is_trusted(trust: Option<&str>) -> bool {
    match trust.map(from_text::<Trust>) {
        Some(Ok(Trust::Trusted)) => true,
        Some(Ok(Trust::Untrusted)) | Some(Err(_)) | None => false,
    }
}

/// Storage errors reach the model as a fixed message, so no stored text leaks through one.
pub(crate) fn storage_failed(error: impl std::fmt::Display) -> ToolError {
    tracing::error!(%error, "tool storage failed");
    ToolError::Internal("storage failed".to_string())
}
