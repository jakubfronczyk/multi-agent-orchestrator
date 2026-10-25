//! `Store` over SQLite: update the projections, append the event, commit, then broadcast.
//! One async mutex serializes `emit`, so event ids are monotonic and broadcast in id order.

use async_trait::async_trait;
use chrono::{SubsecRound, Utc};
use orchestrator_core::agents::AgentSpec;
use orchestrator_core::ids::{EventId, RunId, TeamId};
use orchestrator_core::runs::{ApprovalStatus, CallPhase, RunEvent, RunStatus};
use orchestrator_core::store::{RunRef, Store, StoreError};
use serde::Serialize;
use sqlx::sqlite::SqliteQueryResult;
use sqlx::{Sqlite, SqlitePool, Transaction};
use tokio::sync::{Mutex, broadcast};

use super::text::{from_text, timestamp, to_text};
use super::{agents, begin_write, events};
use crate::events::EventEnvelope;

const BROADCAST_CAPACITY: usize = 1024;

pub struct SqliteStore {
    pool: SqlitePool,
    /// A tokio mutex: the guard is held across the transaction's awaits, where a std guard is not `Send`.
    emit_lock: Mutex<()>,
    events: broadcast::Sender<EventEnvelope>,
}

impl SqliteStore {
    pub fn new(pool: SqlitePool) -> Self {
        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            pool,
            emit_lock: Mutex::new(()),
            events,
        }
    }

    /// Committed events in id order; a receiver that lags past the buffer gets `Lagged`.
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.events.subscribe()
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn emit(&self, at: RunRef, event: RunEvent) -> Result<EventId, StoreError> {
        // Held through the send, so broadcast order is id order and a client that reconnects
        // with the last id it saw never skips an earlier one.
        let _guard = self.emit_lock.lock().await;
        let now = Utc::now().trunc_subsecs(6);
        let ts = timestamp(now);

        let mut tx = begin_write(&self.pool).await.map_err(failed)?;
        // Projections first: run_started creates the runs row the event references.
        project(&mut tx, at, &event, &ts).await?;
        let id = insert_event(&mut tx, at, &event, &ts).await?;
        // After the projection, so a root `run_started` finds its own row.
        let root = text(&at.root_run_id)?;
        let team = sqlx::query_scalar!("SELECT team_id FROM runs WHERE id = ?", root)
            .fetch_one(&mut *tx)
            .await
            .map_err(failed)?;
        let team_id: TeamId = from_text(&team).map_err(failed)?;
        tx.commit().await.map_err(failed)?;

        // Only committed events reach subscribers; having no subscribers is not an error.
        let _ = self.events.send(EventEnvelope {
            id,
            root_run_id: at.root_run_id,
            run_id: at.run_id,
            team_id,
            ts: now,
            event,
        });
        Ok(id)
    }

    async fn team(
        &self,
        team: Option<TeamId>,
    ) -> Result<Option<(TeamId, Vec<AgentSpec>)>, StoreError> {
        agents::live_team(&self.pool, team).await.map_err(failed)
    }

    async fn task_events(&self, root: RunId) -> Result<Vec<(RunId, RunEvent)>, StoreError> {
        events::of_task(&self.pool, root).await.map_err(failed)
    }
}

async fn insert_event(
    tx: &mut Transaction<'_, Sqlite>,
    at: RunRef,
    event: &RunEvent,
    ts: &str,
) -> Result<EventId, StoreError> {
    let value = serde_json::to_value(event).map_err(failed)?;
    let kind = value["type"]
        .as_str()
        .ok_or_else(|| failed("event has no type"))?;
    let data = value["data"].to_string();
    let root = text(&at.root_run_id)?;
    let run = text(&at.run_id)?;
    let result = sqlx::query!(
        "INSERT INTO run_events (root_run_id, run_id, ts, type, data) VALUES (?, ?, ?, ?, ?)",
        root,
        run,
        ts,
        kind,
        data
    )
    .execute(&mut **tx)
    .await
    .map_err(failed)?;
    Ok(EventId(result.last_insert_rowid()))
}

/// Every variant is listed, so a new event type fails to compile until its projection is decided.
async fn project(
    tx: &mut Transaction<'_, Sqlite>,
    at: RunRef,
    event: &RunEvent,
    ts: &str,
) -> Result<(), StoreError> {
    let run_id = text(&at.run_id)?;
    match event {
        RunEvent::RunStarted {
            agent_id,
            team_id,
            parent_run_id,
            parent_call_id,
            task,
            task_sources,
            title,
            budget: _,
            scenario,
            agent_version,
            draft_only,
        } => {
            let task_sources = json(task_sources)?;
            let root = text(&at.root_run_id)?;
            let agent = text(agent_id)?;
            let parent = parent_run_id.as_ref().map(text).transpose()?;
            let parent_call = parent_call_id.as_ref().map(text).transpose()?;
            let status = text(&RunStatus::Running)?;
            let team = text(team_id)?;
            // No run is recorded on a deleted team: the insert touches no row and the emit fails.
            exactly_one(
                sqlx::query!(
                    "INSERT INTO runs (id, root_run_id, team_id, parent_run_id, parent_call_id, agent_id, agent_version, title, task, task_sources, scenario, draft_only, status, started_at)
                     SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
                     WHERE EXISTS (SELECT 1 FROM teams WHERE id = ? AND deleted_at IS NULL)",
                    run_id,
                    root,
                    team,
                    parent,
                    parent_call,
                    agent,
                    agent_version,
                    title,
                    task,
                    task_sources,
                    scenario,
                    draft_only,
                    status,
                    ts,
                    team
                )
                .execute(&mut **tx)
                .await,
            )
        }
        RunEvent::PlanRecorded { .. }
        | RunEvent::LlmRequested { .. }
        | RunEvent::LlmResponded { .. }
        | RunEvent::LlmRetrying { .. } => Ok(()),
        RunEvent::ToolCallRequested {
            call_id,
            tool,
            args,
            action,
            decision,
            reasons,
        } => {
            let id = text(call_id)?;
            let args = json(args)?;
            let action = action.as_ref().map(json).transpose()?;
            let decision = decision.as_ref().map(text).transpose()?;
            let rules = json(reasons)?;
            let phase = text(&CallPhase::Requested)?;
            exactly_one(
                sqlx::query!(
                    "INSERT INTO tool_calls (id, run_id, agent_id, team_id, tool, args, action, decision, rules, phase, requested_at)
                     SELECT ?, id, agent_id, team_id, ?, ?, ?, ?, ?, ?, ? FROM runs WHERE id = ?",
                    id,
                    tool,
                    args,
                    action,
                    decision,
                    rules,
                    phase,
                    ts,
                    run_id
                )
                .execute(&mut **tx)
                .await,
            )
        }
        RunEvent::ApprovalRequested {
            approval_id,
            call_id,
            action: _,
            custody,
            preview,
            expires_at,
        } => {
            let id = text(approval_id)?;
            let call = text(call_id)?;
            let custody = json(custody)?;
            let expires_at = timestamp(*expires_at);
            let pending = text(&ApprovalStatus::Pending)?;
            let waiting = text(&CallPhase::Waiting)?;
            exactly_one(
                sqlx::query!(
                    "INSERT INTO approvals (id, tool_call_id, custody, preview, status, expires_at, created_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                    id,
                    call,
                    custody,
                    preview,
                    pending,
                    expires_at,
                    ts
                )
                .execute(&mut **tx)
                .await,
            )?;
            exactly_one(
                sqlx::query!(
                    "UPDATE tool_calls SET phase = ? WHERE id = ?",
                    waiting,
                    call
                )
                .execute(&mut **tx)
                .await,
            )
        }
        RunEvent::ApprovalResolved {
            approval_id,
            outcome,
            note,
        } => {
            let id = text(approval_id)?;
            let status = text(&ApprovalStatus::from(*outcome))?;
            let pending = text(&ApprovalStatus::Pending)?;
            // Only a pending approval resolves, so a second answer fails instead of overwriting.
            exactly_one(
                sqlx::query!(
                    "UPDATE approvals SET status = ?, note = ?, resolved_at = ? WHERE id = ? AND status = ?",
                    status,
                    note,
                    ts,
                    id,
                    pending
                )
                .execute(&mut **tx)
                .await,
            )
        }
        RunEvent::ToolCallStarted { call_id } => {
            let id = text(call_id)?;
            let running = text(&CallPhase::Running)?;
            exactly_one(
                sqlx::query!("UPDATE tool_calls SET phase = ? WHERE id = ?", running, id)
                    .execute(&mut **tx)
                    .await,
            )
        }
        RunEvent::ToolCallFinished {
            call_id,
            status,
            result_preview,
            duration_ms,
            child_run_id,
            tool: _,
            args: _,
        } => {
            let id = text(call_id)?;
            let phase = text(&CallPhase::Finished)?;
            let status = text(status)?;
            let duration_ms = i64::try_from(*duration_ms).unwrap_or(i64::MAX);
            let child = child_run_id.as_ref().map(text).transpose()?;
            exactly_one(
                sqlx::query!(
                    "UPDATE tool_calls SET phase = ?, status = ?, result_preview = ?, duration_ms = ?, child_run_id = ?,
                     finished_at = ? WHERE id = ?",
                    phase,
                    status,
                    result_preview,
                    duration_ms,
                    child,
                    ts,
                    id
                )
                .execute(&mut **tx)
                .await,
            )
        }
        RunEvent::TaskTainted { source } => {
            let root = text(&at.root_run_id)?;
            let source = json(source)?;
            exactly_one(
                sqlx::query!(
                    "UPDATE runs SET taint_sources = json_insert(COALESCE(taint_sources, '[]'), '$[#]', json(?))
                     WHERE id = ?",
                    source,
                    root
                )
                .execute(&mut **tx)
                .await,
            )
        }
        RunEvent::RunFinished { outcome } => {
            let status = text(&RunStatus::from(outcome))?;
            let outcome = json(outcome)?;
            exactly_one(
                sqlx::query!(
                    "UPDATE runs SET status = ?, outcome = ?, finished_at = ? WHERE id = ?",
                    status,
                    outcome,
                    ts,
                    run_id
                )
                .execute(&mut **tx)
                .await,
            )
        }
    }
}

/// A projection that touches no row means the event refers to something that does not exist.
fn exactly_one(result: Result<SqliteQueryResult, sqlx::Error>) -> Result<(), StoreError> {
    match result.map_err(failed)?.rows_affected() {
        1 => Ok(()),
        n => Err(failed(format!("projection touched {n} rows, expected 1"))),
    }
}

fn text<T: Serialize>(value: &T) -> Result<String, StoreError> {
    to_text(value).map_err(failed)
}

fn json<T: Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(failed)
}

fn failed(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> StoreError {
    StoreError::Failed(error.into())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use orchestrator_core::ids::{AgentId, RunId, ToolCallId};

    use super::*;

    /// A team with one Lead, written directly.
    async fn seed_agent(pool: &SqlitePool) -> (TeamId, AgentId) {
        let mut conn = pool.acquire().await.expect("connection");
        let team = crate::sqlite::teams::insert(&mut conn, "t")
            .await
            .expect("team");
        let team_text = to_text(&team).expect("team id");
        let id = AgentId::new();
        let text_id = to_text(&id).expect("id");
        sqlx::query!(
            "INSERT INTO agents (id, team_id, role, slug, name, description, system_prompt, model, max_steps, icon, colour, created_at, updated_at)
             VALUES (?, ?, 'lead', 'lead', 'Lead', 'd', 'p', 'grok-4.3', 12, 'i', 'c', 't', 't')",
            text_id,
            team_text
        )
        .execute(pool)
        .await
        .expect("seed agent");
        (team, id)
    }

    async fn start_root_run(store: &SqliteStore, pool: &SqlitePool) -> RunRef {
        let run = RunId::new();
        let at = RunRef {
            root_run_id: run,
            run_id: run,
        };
        let (team_id, agent_id) = seed_agent(pool).await;
        let event = RunEvent::RunStarted {
            agent_id,
            team_id,
            parent_run_id: None,
            parent_call_id: None,
            task: "t".to_string(),
            task_sources: Vec::new(),
            title: None,
            budget: None,
            scenario: None,
            agent_version: None,
            draft_only: false,
        };
        store.emit(at, event).await.expect("run started");
        at
    }

    async fn event_count(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar!("SELECT COUNT(*) FROM run_events")
            .fetch_one(pool)
            .await
            .expect("count")
    }

    #[sqlx::test]
    async fn concurrent_emits_broadcast_in_increasing_id_order(pool: SqlitePool) {
        let store = Arc::new(SqliteStore::new(pool.clone()));
        let at = start_root_run(&store, &pool).await;
        let mut rx = store.subscribe();

        let handles: Vec<_> = (0..20)
            .map(|step| {
                let store = Arc::clone(&store);
                tokio::spawn(async move { store.emit(at, RunEvent::LlmRequested { step }).await })
            })
            .collect();
        for handle in handles {
            handle.await.expect("join").expect("emit");
        }

        let mut last = 0;
        for _ in 0..20 {
            let envelope = rx.recv().await.expect("broadcast");
            assert!(envelope.id.0 > last, "ids must increase");
            last = envelope.id.0;
        }
    }

    #[sqlx::test]
    async fn failed_projection_writes_and_broadcasts_nothing(pool: SqlitePool) {
        let store = SqliteStore::new(pool.clone());
        let at = start_root_run(&store, &pool).await;
        let mut rx = store.subscribe();
        let before = event_count(&pool).await;

        let unknown_call = RunEvent::ToolCallStarted {
            call_id: ToolCallId::new(),
        };
        assert!(store.emit(at, unknown_call).await.is_err());

        assert_eq!(event_count(&pool).await, before);
        assert!(rx.try_recv().is_err(), "nothing may be broadcast");
    }
}
