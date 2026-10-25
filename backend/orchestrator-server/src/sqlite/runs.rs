//! Run reads for the API: the task list and one task's detail. Runs are written only by `Store::emit`.
//! Costs are summed from `llm_responded` events, the one record of what a call cost.

use chrono::{DateTime, Utc};
use orchestrator_core::ids::{AgentId, EventId, RunId, ToolCallId};
use orchestrator_core::money::Micros;
use orchestrator_core::policy::{Decision, Rule, TaintSource};
use orchestrator_core::runs::{ApprovalStatus, CallPhase, CallStatus, RunOutcome, RunStatus};
use orchestrator_core::tools::{Action, PlanStep};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;
use ts_rs::TS;

use super::approvals::{self, ApprovalView};
use super::events::{self, LLM_RESPONDED, PLAN_RECORDED};
use super::read::{ReadError, corrupt, count, json_opt, text_opt, time_opt};
use super::text::{from_text, parse_timestamp, to_text};

/// One task, read from its root run.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RunSummary {
    pub id: RunId,
    pub agent_id: AgentId,
    pub title: Option<String>,
    pub task: String,
    pub status: RunStatus,
    pub outcome: Option<RunOutcome>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Approvals in the task still waiting for a person.
    pub pending_approvals: u32,
    /// Every agent that ran in the task, the root's included.
    pub agents: Vec<AgentId>,
    /// Calls refused by policy or denied by a person.
    pub refusals: u32,
    pub cost_micros: Micros,
}

/// `last_event_id` is read before the list, so applying events with a larger id never skips one.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RunList {
    pub runs: Vec<RunSummary>,
    pub last_event_id: Option<EventId>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RunView {
    pub id: RunId,
    pub parent_run_id: Option<RunId>,
    /// The `delegate` call that started this run, so a running child nests under its call.
    pub parent_call_id: Option<ToolCallId>,
    pub agent_id: AgentId,
    pub title: Option<String>,
    pub task: String,
    pub status: RunStatus,
    pub outcome: Option<RunOutcome>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub cost_micros: Micros,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ToolCallView {
    pub id: ToolCallId,
    pub run_id: RunId,
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

/// One task: its runs in start order (the root first), calls in request order, and the cards
/// still pending. `last_event_id` is read first, as in `RunList`.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RunDetail {
    pub root_run_id: RunId,
    pub runs: Vec<RunView>,
    pub tool_calls: Vec<ToolCallView>,
    pub pending_approvals: Vec<ApprovalView>,
    /// The latest plan recorded in the task.
    pub plan: Option<Vec<PlanStep>>,
    pub taint_sources: Vec<TaintSource>,
    pub cost_micros: Micros,
    pub last_event_id: Option<EventId>,
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
struct ToolCallRow {
    id: String,
    run_id: String,
    agent_id: String,
    tool: String,
    args: String,
    action: Option<String>,
    decision: Option<String>,
    rules: String,
    phase: String,
    status: Option<String>,
    result_preview: Option<String>,
    child_run_id: Option<String>,
    duration_ms: Option<i64>,
    requested_at: String,
    finished_at: Option<String>,
}

impl ToolCallRow {
    fn view(self) -> Result<ToolCallView, ReadError> {
        Ok(ToolCallView {
            id: from_text(&self.id).map_err(corrupt)?,
            run_id: from_text(&self.run_id).map_err(corrupt)?,
            agent_id: from_text(&self.agent_id).map_err(corrupt)?,
            tool: self.tool,
            args: serde_json::from_str(&self.args).map_err(corrupt)?,
            action: json_opt(self.action.as_deref())?,
            decision: text_opt(self.decision.as_deref())?,
            rules: serde_json::from_str(&self.rules).map_err(corrupt)?,
            phase: from_text(&self.phase).map_err(corrupt)?,
            status: text_opt(self.status.as_deref())?,
            result_preview: self.result_preview,
            child_run_id: text_opt(self.child_run_id.as_deref())?,
            duration_ms: self.duration_ms,
            requested_at: parse_timestamp(&self.requested_at).map_err(corrupt)?,
            finished_at: time_opt(self.finished_at.as_deref())?,
        })
    }
}

#[derive(Deserialize)]
struct PlanData {
    steps: Vec<PlanStep>,
}

/// Root runs, newest first, each with its task's counts and cost, in one query.
pub async fn list(pool: &SqlitePool, limit: u32) -> Result<RunList, ReadError> {
    let pending = to_text(&ApprovalStatus::Pending).map_err(corrupt)?;
    let refused = to_text(&CallStatus::Refused).map_err(corrupt)?;
    let denied = to_text(&CallStatus::DeniedHuman).map_err(corrupt)?;
    let mut tx = pool.begin().await?;
    let last_event_id = events::last_id(&mut *tx).await?;
    let rows = sqlx::query!(
        r#"SELECT r.id, r.agent_id, r.title, r.task, r.status, r.outcome, r.started_at, r.finished_at,
                  (SELECT COUNT(*) FROM approvals a
                     JOIN tool_calls t ON t.id = a.tool_call_id
                     JOIN runs c ON c.id = t.run_id
                    WHERE c.root_run_id = r.id AND a.status = ?1) AS "pending!: i64",
                  (SELECT json_group_array(DISTINCT c.agent_id) FROM runs c
                    WHERE c.root_run_id = r.id) AS "agents!: String",
                  (SELECT COUNT(*) FROM tool_calls t
                     JOIN runs c ON c.id = t.run_id
                    WHERE c.root_run_id = r.id AND t.status IN (?2, ?3)) AS "refusals!: i64",
                  (SELECT COALESCE(SUM(json_extract(e.data, '$.cost')), 0) FROM run_events e
                    WHERE e.root_run_id = r.id AND e.type = ?4) AS "cost!: i64"
           FROM runs r
           WHERE r.parent_run_id IS NULL
           ORDER BY r.started_at DESC, r.id DESC
           LIMIT ?5"#,
        pending,
        refused,
        denied,
        LLM_RESPONDED,
        limit
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let runs = rows
        .into_iter()
        .map(|row| {
            Ok(RunSummary {
                id: from_text(&row.id).map_err(corrupt)?,
                agent_id: from_text(&row.agent_id).map_err(corrupt)?,
                title: row.title,
                task: row.task,
                status: from_text(&row.status).map_err(corrupt)?,
                outcome: json_opt(row.outcome.as_deref())?,
                started_at: parse_timestamp(&row.started_at).map_err(corrupt)?,
                finished_at: time_opt(row.finished_at.as_deref())?,
                pending_approvals: count(row.pending)?,
                agents: serde_json::from_str(&row.agents).map_err(corrupt)?,
                refusals: count(row.refusals)?,
                cost_micros: Micros(row.cost),
            })
        })
        .collect::<Result<Vec<_>, ReadError>>()?;
    Ok(RunList {
        runs,
        last_event_id,
    })
}

/// The task whose root run is `root`, or `None` when no root run has this id.
pub async fn detail(pool: &SqlitePool, root: RunId) -> Result<Option<RunDetail>, ReadError> {
    let root_text = to_text(&root).map_err(corrupt)?;
    let mut tx = pool.begin().await?;
    let last_event_id = events::last_id(&mut *tx).await?;

    let run_rows = sqlx::query!(
        r#"SELECT r.id, r.parent_run_id, r.parent_call_id, r.agent_id, r.title, r.task, r.status, r.outcome,
                  r.taint_sources, r.started_at, r.finished_at,
                  (SELECT COALESCE(SUM(json_extract(e.data, '$.cost')), 0) FROM run_events e
                    WHERE e.root_run_id = ?1 AND e.run_id = r.id AND e.type = ?2) AS "cost!: i64"
           FROM runs r
           WHERE r.root_run_id = ?1
           ORDER BY r.started_at, r.id"#,
        root_text,
        LLM_RESPONDED
    )
    .fetch_all(&mut *tx)
    .await?;
    // A child run's id matches no root_run_id, so only a root id finds rows.
    let Some(root_row) = run_rows.iter().find(|row| row.id == root_text) else {
        return Ok(None);
    };
    let taint_sources = json_opt(root_row.taint_sources.as_deref())?.unwrap_or_default();

    let call_rows = sqlx::query_as!(
        ToolCallRow,
        r#"SELECT t.id, t.run_id, t.agent_id, t.tool, t.args, t.action, t.decision, t.rules,
                  t.phase, t.status, t.result_preview, t.child_run_id, t.duration_ms,
                  t.requested_at, t.finished_at
           FROM tool_calls t JOIN runs r ON r.id = t.run_id
           WHERE r.root_run_id = ?1
           ORDER BY t.requested_at, t.id"#,
        root_text
    )
    .fetch_all(&mut *tx)
    .await?;

    let plan = sqlx::query_scalar!(
        "SELECT data FROM run_events WHERE root_run_id = ?1 AND type = ?2 ORDER BY id DESC LIMIT 1",
        root_text,
        PLAN_RECORDED
    )
    .fetch_optional(&mut *tx)
    .await?;
    let pending_approvals =
        approvals::views(&mut tx, Some(ApprovalStatus::Pending), Some(root)).await?;
    tx.commit().await?;

    let runs = run_rows
        .into_iter()
        .map(|row| {
            Ok(RunView {
                id: from_text(&row.id).map_err(corrupt)?,
                parent_run_id: text_opt(row.parent_run_id.as_deref())?,
                parent_call_id: text_opt(row.parent_call_id.as_deref())?,
                agent_id: from_text(&row.agent_id).map_err(corrupt)?,
                title: row.title,
                task: row.task,
                status: from_text(&row.status).map_err(corrupt)?,
                outcome: json_opt(row.outcome.as_deref())?,
                started_at: parse_timestamp(&row.started_at).map_err(corrupt)?,
                finished_at: time_opt(row.finished_at.as_deref())?,
                cost_micros: Micros(row.cost),
            })
        })
        .collect::<Result<Vec<_>, ReadError>>()?;
    let tool_calls = call_rows
        .into_iter()
        .map(ToolCallRow::view)
        .collect::<Result<Vec<_>, ReadError>>()?;
    let plan = json_opt::<PlanData>(plan.as_deref())?.map(|data| data.steps);
    let cost_micros = runs
        .iter()
        .fold(Micros(0), |sum, run| sum.saturating_add(run.cost_micros));

    Ok(Some(RunDetail {
        root_run_id: root,
        runs,
        tool_calls,
        pending_approvals,
        plan,
        taint_sources,
        cost_micros,
        last_event_id,
    }))
}

pub async fn tool_calls(
    pool: &SqlitePool,
    filter: ToolCallFilter,
    limit: u32,
) -> Result<ToolCallList, ReadError> {
    let run = filter
        .run_id
        .as_ref()
        .map(to_text)
        .transpose()
        .map_err(corrupt)?;
    let agent = filter
        .agent_id
        .as_ref()
        .map(to_text)
        .transpose()
        .map_err(corrupt)?;
    let status = filter
        .status
        .as_ref()
        .map(to_text)
        .transpose()
        .map_err(corrupt)?;
    let mut tx = pool.begin().await?;
    let last_event_id = events::last_id(&mut *tx).await?;
    let rows = sqlx::query_as!(
        ToolCallRow,
        r#"SELECT id, run_id, agent_id, tool, args, action, decision, rules, phase, status,
                  result_preview, child_run_id, duration_ms, requested_at, finished_at
           FROM tool_calls
           WHERE (?1 IS NULL OR run_id = ?1) AND (?2 IS NULL OR agent_id = ?2)
             AND (?3 IS NULL OR tool = ?3) AND (?4 IS NULL OR status = ?4)
           ORDER BY requested_at DESC, id DESC
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

/// Whether a root run has this id, for telling an unknown task from a finished one.
pub async fn root_exists(pool: &SqlitePool, root: RunId) -> Result<bool, ReadError> {
    let root = to_text(&root).map_err(corrupt)?;
    let found = sqlx::query_scalar!(
        r#"SELECT 1 AS "found!: i64" FROM runs WHERE id = ?1 AND parent_run_id IS NULL"#,
        root
    )
    .fetch_optional(pool)
    .await?;
    Ok(found.is_some())
}
