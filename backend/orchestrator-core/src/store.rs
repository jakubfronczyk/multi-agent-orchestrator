//! The engine's only window on storage; the server implements it over SQLite.
//! `emit` appends an event and updates its projections in one transaction.

use async_trait::async_trait;

use crate::ids::{EventId, RunId};
use crate::runs::RunEvent;

/// The run an event belongs to and the root of its task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunRef {
    pub root_run_id: RunId,
    pub run_id: RunId,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("storage failed: {0}")]
    Failed(String),
}

#[async_trait]
pub trait Store: Send + Sync {
    async fn emit(&self, at: RunRef, event: RunEvent) -> Result<EventId, StoreError>;
}
