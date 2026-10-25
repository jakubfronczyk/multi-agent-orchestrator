//! The drill report: every way out of a task, read from its calls. An exit is any call to a
//! tool whose effect is External or Egress, whether or not it ran.

use orchestrator_core::ids::{RunId, TeamId, ToolCallId};
use orchestrator_core::policy::{Decision, Effect, Rule};
use orchestrator_core::runs::{ApprovalStatus, CallStatus, RunStatus};
use orchestrator_core::tools::{Action, ToolName};
use serde::Serialize;
use sqlx::SqlitePool;
use ts_rs::TS;

use super::read::{ReadError, count, decode, decode_json, encode, json_opt, text_opt};
use super::text::from_text;

/// Who stopped or let through an exit: policy alone, or a person on a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DecidedBy {
    Gateway,
    /// The call asked; `approval` says how the card ended, an expiry counting as no.
    User,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DrillExit {
    pub call_id: ToolCallId,
    pub run_id: RunId,
    /// The slug of the agent that made the call.
    pub agent: String,
    pub tool: ToolName,
    /// `None` when the arguments were invalid.
    pub action: Option<Action>,
    pub decision: Option<Decision>,
    pub rules: Vec<Rule>,
    pub decided_by: DecidedBy,
    pub approval: Option<ApprovalStatus>,
    /// How the call ended; `None` while it is still open.
    pub status: Option<CallStatus>,
}

/// Exits in request order, with the task's state and how many messages it actually sent.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DrillReport {
    pub root_run_id: RunId,
    /// The team the drill ran on; `DrillExit.agent` slugs resolve in it.
    pub team_id: TeamId,
    pub status: RunStatus,
    pub exits: Vec<DrillExit>,
    pub messages_sent: u32,
}

/// The report for the task whose root run is `root`, or `None` when no root run has this id.
pub async fn report(pool: &SqlitePool, root: RunId) -> Result<Option<DrillReport>, ReadError> {
    let root_text = encode(&root)?;
    let mut tx = pool.begin().await?;
    let root_row = sqlx::query!(
        "SELECT status, team_id FROM runs WHERE id = ?1 AND root_run_id = ?1",
        root_text
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(root_row) = root_row else {
        return Ok(None);
    };
    let rows = sqlx::query!(
        r#"SELECT t.id, t.run_id, t.tool, t.action, t.decision, t.rules, t.status,
                  ag.slug AS "agent!", a.status AS "approval?"
           FROM tool_calls t
           JOIN runs r ON r.id = t.run_id
           JOIN agents ag ON ag.id = t.agent_id
           LEFT JOIN approvals a ON a.tool_call_id = t.id
           WHERE r.root_run_id = ?1
           ORDER BY t.requested_at, t.id"#,
        root_text
    )
    .fetch_all(&mut *tx)
    .await?;
    let sent = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "sent!: i64" FROM outbox_messages o JOIN runs r ON r.id = o.run_id
           WHERE r.root_run_id = ?1"#,
        root_text
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    let mut exits = Vec::new();
    for row in rows {
        // A name the model made up is no tool, so it is no exit.
        let Ok(tool) = from_text::<ToolName>(&row.tool) else {
            continue;
        };
        if !matches!(tool.effect(), Effect::External | Effect::Egress) {
            continue;
        }
        let decision: Option<Decision> = text_opt(row.decision.as_deref())?;
        exits.push(DrillExit {
            call_id: decode(&row.id)?,
            run_id: decode(&row.run_id)?,
            agent: row.agent,
            tool,
            action: json_opt(row.action.as_deref())?,
            decided_by: if decision == Some(Decision::Ask) {
                DecidedBy::User
            } else {
                DecidedBy::Gateway
            },
            decision,
            rules: decode_json(&row.rules)?,
            approval: text_opt(row.approval.as_deref())?,
            status: text_opt(row.status.as_deref())?,
        });
    }
    Ok(Some(DrillReport {
        root_run_id: root,
        team_id: decode(&root_row.team_id)?,
        status: decode(&root_row.status)?,
        exits,
        messages_sent: count(sent)?,
    }))
}
