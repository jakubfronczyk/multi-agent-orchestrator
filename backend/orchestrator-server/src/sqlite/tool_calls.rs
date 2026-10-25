//! The audit log: every tool call with its decision, rules and result.

use chrono::{DateTime, Utc};
use orchestrator_core::ids::{AgentId, EventId, RunId, ToolCallId};
use orchestrator_core::policy::{Decision, Rule};
use orchestrator_core::runs::{CallPhase, CallStatus};
use orchestrator_core::tools::Action;
use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;
use ts_rs::TS;

use super::events;
use super::read::{
    ReadError, decode, decode_json, decode_time, encode_opt, json_opt, text_opt, time_opt,
};

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ToolCallView {
    pub id: ToolCallId,
    pub run_id: RunId,
    /// The task's root run, so a row can open its task.
    pub root_run_id: RunId,
    pub agent_id: AgentId,
    /// The name as the model sent it; it may not be a real tool.
    pub tool: String,
    pub args: Value,
    pub action: Option<Action>,
    pub decision: Option<Decision>,
    pub rules: Vec<Rule>,
    pub phase: CallPhase,
    pub status: Option<CallStatus>,
    pub result_preview: Option<String>,
    pub child_run_id: Option<RunId>,
    pub duration_ms: Option<i64>,
    pub requested_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// The audit log, newest first; `last_event_id` is read first, as in `RunList`.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ToolCallList {
    pub tool_calls: Vec<ToolCallView>,
    pub last_event_id: Option<EventId>,
}

/// Audit filters; each one that is set must match.
#[derive(Debug, Clone, Default)]
pub struct ToolCallFilter {
    pub run_id: Option<RunId>,
    pub agent_id: Option<AgentId>,
    /// The name as the model sent it.
    pub tool: Option<String>,
    pub status: Option<CallStatus>,
}

/// One `tool_calls` row, shared by every query that returns calls.
pub(super) struct ToolCallRow {
    pub(super) id: String,
    pub(super) run_id: String,
    pub(super) root_run_id: String,
    pub(super) agent_id: String,
    pub(super) tool: String,
    pub(super) args: String,
    pub(super) action: Option<String>,
    pub(super) decision: Option<String>,
    pub(super) rules: String,
    pub(super) phase: String,
    pub(super) status: Option<String>,
    pub(super) result_preview: Option<String>,
    pub(super) child_run_id: Option<String>,
    pub(super) duration_ms: Option<i64>,
    pub(super) requested_at: String,
    pub(super) finished_at: Option<String>,
}

impl ToolCallRow {
    pub(super) fn view(self) -> Result<ToolCallView, ReadError> {
        Ok(ToolCallView {
            id: decode(&self.id)?,
            run_id: decode(&self.run_id)?,
            root_run_id: decode(&self.root_run_id)?,
            agent_id: decode(&self.agent_id)?,
            tool: self.tool,
            args: decode_json(&self.args)?,
            action: json_opt(self.action.as_deref())?,
            decision: text_opt(self.decision.as_deref())?,
            rules: decode_json(&self.rules)?,
            phase: decode(&self.phase)?,
            status: text_opt(self.status.as_deref())?,
            result_preview: self.result_preview,
            child_run_id: text_opt(self.child_run_id.as_deref())?,
            duration_ms: self.duration_ms,
            requested_at: decode_time(&self.requested_at)?,
            finished_at: time_opt(self.finished_at.as_deref())?,
        })
    }
}

pub async fn tool_calls(
    pool: &SqlitePool,
    filter: ToolCallFilter,
    limit: u32,
) -> Result<ToolCallList, ReadError> {
    let run = encode_opt(filter.run_id.as_ref())?;
    let agent = encode_opt(filter.agent_id.as_ref())?;
    let status = encode_opt(filter.status.as_ref())?;
    let mut tx = pool.begin().await?;
    let last_event_id = events::last_id(&mut *tx).await?;
    let rows = sqlx::query_as!(
        ToolCallRow,
        r#"SELECT t.id, t.run_id, r.root_run_id, t.agent_id, t.tool, t.args, t.action, t.decision,
                  t.rules, t.phase, t.status, t.result_preview, t.child_run_id, t.duration_ms,
                  t.requested_at, t.finished_at
           FROM tool_calls t JOIN runs r ON r.id = t.run_id
           WHERE (?1 IS NULL OR t.run_id = ?1) AND (?2 IS NULL OR t.agent_id = ?2)
             AND (?3 IS NULL OR t.tool = ?3) AND (?4 IS NULL OR t.status = ?4)
           ORDER BY t.requested_at DESC, t.id DESC
           LIMIT ?5"#,
        run,
        agent,
        filter.tool,
        status,
        limit
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let tool_calls = rows
        .into_iter()
        .map(ToolCallRow::view)
        .collect::<Result<Vec<_>, ReadError>>()?;
    Ok(ToolCallList {
        tool_calls,
        last_event_id,
    })
}
