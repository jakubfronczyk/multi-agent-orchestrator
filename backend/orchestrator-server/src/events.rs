//! The envelope every run event travels in, over SSE and in API responses.
//! Its fields flatten to `{id, root_run_id, run_id, ts, type, data}`.

use chrono::{DateTime, Utc};
use orchestrator_core::ids::{EventId, RunId};
use orchestrator_core::runs::RunEvent;
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct EventEnvelope {
    pub id: EventId,
    pub root_run_id: RunId,
    pub run_id: RunId,
    pub ts: DateTime<Utc>,
    #[serde(flatten)]
    pub event: RunEvent,
}
