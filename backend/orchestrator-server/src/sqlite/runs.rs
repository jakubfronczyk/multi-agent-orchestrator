//! Run reads for the API: the task list and one task's detail, all written by `Store::emit`.
//! Costs are summed from `llm_responded` events, the one record of what a call cost.

use chrono::{DateTime, Utc};
use orchestrator_core::ids::{AgentId, EventId, RunId, ToolCallId};
use orchestrator_core::money::Micros;
use orchestrator_core::policy::TaintSource;
use orchestrator_core::runs::{ApprovalStatus, CallStatus, RunOutcome, RunStatus};
use orchestrator_core::tools::PlanStep;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use ts_rs::TS;

use super::approvals::{self, ApprovalView};
use super::events::{self, LLM_RESPONDED, PLAN_RECORDED};
use super::read::{
    ReadError, count, decode, decode_json, decode_time, encode, json_opt, text_opt, time_opt,
};
use super::tool_calls::{ToolCallRow, ToolCallView};

/// One task, read from its root run; status, outcome and finish are its latest turn's.
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
    /// The scripted replies the root run played; `None` when it ran on the live model.
    pub scenario: Option<String>,
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
    /// The agent's config version this run used; `None` for runs recorded before it existed.
    pub agent_version: Option<i64>,
    pub title: Option<String>,
    /// Plain text, never the model's wrapped form. A delegated run's is the exact message the
    /// child received: `Task: …`, then `Context: …` and `Expected output: …` when given.
    pub task: String,
    /// The taint the task carried when this run was delegated; empty when clean.
    pub task_sources: Vec<TaintSource>,
    /// The run's turn refuses every save, note, fetch and send.
    pub draft_only: bool,
    pub status: RunStatus,
    pub outcome: Option<RunOutcome>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub cost_micros: Micros,
}

/// One task: its runs in start order (the root first), calls in request order, and the cards
/// still pending. `last_event_id` is read first, as in `RunList`.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RunDetail {
    pub root_run_id: RunId,
    pub runs: Vec<RunView>,
    /// The user's messages and the Lead's answers: the root run, then each follow-up.
    pub turns: Vec<Turn>,
    pub tool_calls: Vec<ToolCallView>,
    pub pending_approvals: Vec<ApprovalView>,
    /// The latest plan recorded in the task.
    pub plan: Option<Vec<PlanStep>>,
    pub taint_sources: Vec<TaintSource>,
    pub cost_micros: Micros,
    pub last_event_id: Option<EventId>,
}

/// One top-level run of a task: what the user asked and how the turn ended.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct Turn {
    pub run_id: RunId,
    /// Plain text, as the user sent it.
    pub text: String,
    /// The Lead's final message; `None` until the turn completes, or when it did not.
    pub summary: Option<String>,
    /// `None` while the turn runs.
    pub outcome: Option<RunOutcome>,
    /// The turn refused every save, note, fetch and send.
    pub draft_only: bool,
}

#[derive(Deserialize)]
struct PlanData {
    steps: Vec<PlanStep>,
}

/// Tasks by root run, newest first, each with its counts and cost, in one query. A follow-up
/// has no parent either, so a root is the run whose id is its own root id.
pub async fn list(pool: &SqlitePool, limit: u32) -> Result<RunList, ReadError> {
    let pending = encode(&ApprovalStatus::Pending)?;
    let refused = encode(&CallStatus::Refused)?;
    let denied = encode(&CallStatus::DeniedHuman)?;
    let mut tx = pool.begin().await?;
    let last_event_id = events::last_id(&mut *tx).await?;
    let rows = sqlx::query!(
        r#"SELECT r.id, r.agent_id, r.title, r.task, l.status, l.outcome, r.started_at, l.finished_at,
                  r.scenario,
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
           JOIN runs l ON l.id = (SELECT t.id FROM runs t
                                   WHERE t.root_run_id = r.id AND t.parent_run_id IS NULL
                                   ORDER BY t.started_at DESC, t.id DESC LIMIT 1)
           WHERE r.id = r.root_run_id
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
                id: decode(&row.id)?,
                agent_id: decode(&row.agent_id)?,
                title: row.title,
                task: row.task,
                status: decode(&row.status)?,
                outcome: json_opt(row.outcome.as_deref())?,
                started_at: decode_time(&row.started_at)?,
                finished_at: time_opt(row.finished_at.as_deref())?,
                scenario: row.scenario,
                pending_approvals: count(row.pending)?,
                agents: decode_json(&row.agents)?,
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
    let root_text = encode(&root)?;
    let mut tx = pool.begin().await?;
    let last_event_id = events::last_id(&mut *tx).await?;

    let run_rows = sqlx::query!(
        r#"SELECT r.id, r.parent_run_id, r.parent_call_id, r.agent_id, r.agent_version, r.title, r.task, r.task_sources, r.draft_only, r.status, r.outcome,
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
        r#"SELECT t.id, t.run_id, r.root_run_id, t.agent_id, t.tool, t.args, t.action, t.decision,
                  t.rules, t.phase, t.status, t.result_preview, t.child_run_id, t.duration_ms,
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
                id: decode(&row.id)?,
                parent_run_id: text_opt(row.parent_run_id.as_deref())?,
                parent_call_id: text_opt(row.parent_call_id.as_deref())?,
                agent_id: decode(&row.agent_id)?,
                agent_version: row.agent_version,
                title: row.title,
                task: row.task,
                task_sources: decode_json(&row.task_sources)?,
                draft_only: row.draft_only != 0,
                status: decode(&row.status)?,
                outcome: json_opt(row.outcome.as_deref())?,
                started_at: decode_time(&row.started_at)?,
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
    let turns = turns(&runs);

    Ok(Some(RunDetail {
        root_run_id: root,
        runs,
        turns,
        tool_calls,
        pending_approvals,
        plan,
        taint_sources,
        cost_micros,
        last_event_id,
    }))
}

/// The task's turns: every run without a parent, in start order.
fn turns(runs: &[RunView]) -> Vec<Turn> {
    runs.iter()
        .filter(|run| run.parent_run_id.is_none())
        .map(|run| Turn {
            run_id: run.id,
            text: run.task.clone(),
            summary: match &run.outcome {
                Some(RunOutcome::Completed { output, .. }) => Some(output.clone()),
                _ => None,
            },
            outcome: run.outcome.clone(),
            draft_only: run.draft_only,
        })
        .collect()
}

/// Whether a root run has this id, for telling an unknown task from a finished one.
pub async fn root_exists(pool: &SqlitePool, root: RunId) -> Result<bool, ReadError> {
    let root = encode(&root)?;
    let found = sqlx::query_scalar!(
        r#"SELECT 1 AS "found!: i64" FROM runs WHERE id = ?1 AND root_run_id = ?1"#,
        root
    )
    .fetch_optional(pool)
    .await?;
    Ok(found.is_some())
}
