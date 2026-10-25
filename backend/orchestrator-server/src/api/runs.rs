//! `/api/runs`: start a task (202 with its root run id; progress arrives on SSE), list tasks,
//! read one task, follow up on a finished one, cancel one. A task is addressed by its root run id.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use orchestrator_core::agents::Role;
use orchestrator_core::engine::{FollowUpError, FollowUpRequest, StartError, StartRequest};
use orchestrator_core::ids::{AgentId, RunId, TeamId};
use orchestrator_core::money::Micros;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::AppState;
use super::error::ApiError;
use super::json::ApiJson;
use super::query;
use crate::llm::{LAUNDER, S1_DECISION, scenario_slugs};
use crate::sqlite::agents::{self, AgentError};
use crate::sqlite::runs::{self, RunDetail, RunList};
use crate::sqlite::teams;
use crate::tools::workspace::EntryKind;

const DEFAULT_LIMIT: u32 = 50;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    limit: Option<u32>,
    team_id: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, optional_fields)]
pub struct StartRunRequest {
    /// The task's team; with `agent_id` it must be that agent's. Without either, the first team.
    pub team_id: Option<TeamId>,
    /// The root agent; the team's Lead when absent.
    pub agent_id: Option<AgentId>,
    pub task: String,
    pub title: Option<String>,
    pub budget_micros: Option<Micros>,
    /// Which fixed replies to play; a task with a scenario always uses the scripted provider.
    pub scenario: Option<String>,
    /// Refuses every save, note, fetch and send in the first turn, with no card.
    pub draft_only: Option<bool>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct StartRunResponse {
    pub run_id: RunId,
}

pub async fn start(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<StartRunRequest>,
) -> Result<(StatusCode, Json<StartRunResponse>), ApiError> {
    let team = resolve_team(&state, request.team_id, request.agent_id).await?;
    if let Some(scenario) = request.scenario.as_deref() {
        let missing = missing_slugs(&state, team, scenario).await?;
        if !missing.is_empty() {
            let message = format!(
                "this team has no {} for the {scenario} demo",
                missing.join(", ")
            );
            return Err(ApiError::conflict(&message));
        }
    }
    if request.scenario.as_deref() == Some(LAUNDER) {
        require_s1_decision(&state).await?;
    }
    let start = StartRequest {
        team_id: Some(team),
        agent_id: request.agent_id,
        task: request.task,
        title: request.title,
        budget: request.budget_micros,
        scenario: request.scenario,
        draft_only: request.draft_only.unwrap_or(false),
    };
    let run_id = start_in(&state, team, start).await?;
    Ok((StatusCode::ACCEPTED, Json(StartRunResponse { run_id })))
}

/// The team a task runs in: the agent's when one is named (and `team_id`, if given, must match),
/// else `team_id`, else the first live team.
pub(super) async fn resolve_team(
    state: &AppState,
    team: Option<TeamId>,
    agent: Option<AgentId>,
) -> Result<TeamId, ApiError> {
    if let Some(agent) = agent {
        let record = match agents::get(&state.pool, agent).await {
            Err(AgentError::NotFound) => {
                return Err(ApiError::invalid("agent_id", "no live agent has this id"));
            }
            other => other?,
        };
        let own = record.spec.team_id();
        if team.is_some_and(|team| team != own) {
            return Err(ApiError::invalid("agent_id", "not on this team"));
        }
        return Ok(own);
    }
    let mut conn = state.pool.acquire().await.map_err(storage)?;
    let found = match team {
        Some(team) => teams::is_live(&mut conn, team)
            .await
            .map_err(storage)?
            .then_some(team),
        None => teams::first_live(&mut conn).await.map_err(storage)?,
    };
    found.ok_or_else(|| ApiError::invalid("team_id", "no live team has this id"))
}

/// The slugs a scripted scenario needs that the team's Lead cannot hand work to.
pub(super) async fn missing_slugs(
    state: &AppState,
    team: TeamId,
    scenario: &str,
) -> Result<Vec<&'static str>, ApiError> {
    let needed = scenario_slugs(scenario);
    if needed.is_empty() {
        return Ok(Vec::new());
    }
    let lead = agents::list(&state.pool, Some(team))
        .await?
        .into_iter()
        .find(|agent| agent.spec.role() == Role::Lead);
    let targets: Vec<&str> = lead
        .as_ref()
        .map(|lead| {
            lead.spec
                .delegate_targets()
                .iter()
                .map(|t| t.as_str())
                .collect()
        })
        .unwrap_or_default();
    Ok(needed
        .iter()
        .copied()
        .filter(|slug| !targets.contains(slug))
        .collect())
}

/// Starts the task; a storage failure on a team deleted meanwhile reads as that team being gone.
pub(super) async fn start_in(
    state: &AppState,
    team: TeamId,
    request: StartRequest,
) -> Result<RunId, ApiError> {
    match state.engine.start(request).await {
        Ok(run) => Ok(run),
        Err(StartError::Store(error)) => {
            let mut conn = state.pool.acquire().await.map_err(storage)?;
            if teams::is_live(&mut conn, team).await.map_err(storage)? {
                tracing::error!(%error, "could not start the run");
                Err(ApiError::internal())
            } else {
                Err(ApiError::invalid("team_id", "no live team has this id"))
            }
        }
        Err(other) => Err(start_error(other)),
    }
}

fn storage(error: sqlx::Error) -> ApiError {
    tracing::error!(%error, "could not read the team");
    ApiError::internal()
}

/// `launder` indexes the file S1 saves; without it the script would describe a file that is not there.
async fn require_s1_decision(state: &AppState) -> Result<(), ApiError> {
    match state.workspace.kind_of(S1_DECISION).await {
        Ok(Some(EntryKind::File)) => Ok(()),
        Ok(_) => Err(ApiError::conflict("run the s1 scenario first")),
        Err(error) => {
            tracing::error!(%error, "could not check for the s1 decision");
            Err(ApiError::internal())
        }
    }
}

fn start_error(error: StartError) -> ApiError {
    match error {
        StartError::UnknownTeam => ApiError::invalid("team_id", "no live team has this id"),
        StartError::NoLead => ApiError::invalid("agent_id", "the team has no Lead"),
        StartError::UnknownAgent => ApiError::invalid("agent_id", "no live agent has this id"),
        StartError::EmptyTask => ApiError::invalid("task", "must not be empty"),
        StartError::Store(error) => {
            tracing::error!(%error, "could not start the run");
            ApiError::internal()
        }
    }
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, optional_fields)]
pub struct FollowUpBody {
    pub text: String,
    /// Which fixed replies to play; set only by demo prompts, so typed text reaches the live model.
    pub scenario: Option<String>,
    /// Refuses every save, note, fetch and send in this turn, with no card.
    pub draft_only: Option<bool>,
}

/// 202 with the new turn's run id; 409 while any run in the task is live or when its root
/// agent is not the Lead.
pub async fn follow_up(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<FollowUpBody>,
) -> Result<(StatusCode, Json<StartRunResponse>), ApiError> {
    let root: RunId = query::path_id(&id)?;
    let team = runs::team_of(&state.pool, root).await?;
    if let (Some(team), Some(scenario)) = (team, body.scenario.as_deref()) {
        let missing = missing_slugs(&state, team, scenario).await?;
        if !missing.is_empty() {
            let message = format!(
                "this team has no {} for the {scenario} demo",
                missing.join(", ")
            );
            return Err(ApiError::conflict(&message));
        }
    }
    let request = FollowUpRequest {
        text: body.text,
        scenario: body.scenario,
        draft_only: body.draft_only.unwrap_or(false),
    };
    match state.engine.follow_up(root, request).await {
        Ok(run_id) => Ok((StatusCode::ACCEPTED, Json(StartRunResponse { run_id }))),
        // A delete that commits between the engine's team read and its `run_started` makes the
        // emit fail; the team decides whether that is the delete or a real storage error.
        Err(FollowUpError::Store(error)) => {
            let mut conn = state.pool.acquire().await.map_err(storage)?;
            match team {
                Some(team) if !teams::is_live(&mut conn, team).await.map_err(storage)? => {
                    Err(follow_up_error(FollowUpError::TeamDeleted))
                }
                _ => Err(follow_up_error(FollowUpError::Store(error))),
            }
        }
        Err(other) => Err(follow_up_error(other)),
    }
}

fn follow_up_error(error: FollowUpError) -> ApiError {
    match error {
        FollowUpError::EmptyText => ApiError::invalid("text", "must not be empty"),
        FollowUpError::UnknownTask => ApiError::not_found(),
        FollowUpError::Running => ApiError::conflict("this task is still running"),
        FollowUpError::NotLead => {
            ApiError::conflict("only a task the Lead started takes follow-ups")
        }
        FollowUpError::TeamDeleted => ApiError::conflict("this task's team was deleted"),
        FollowUpError::Store(error) => {
            tracing::error!(%error, "could not start the follow-up");
            ApiError::internal()
        }
    }
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<RunList>, ApiError> {
    let limit = query::limit(query.limit, DEFAULT_LIMIT)?;
    let team = query::id::<TeamId>("team_id", query.team_id)?;
    Ok(Json(runs::list(&state.pool, limit, team).await?))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RunDetail>, ApiError> {
    let root: RunId = query::path_id(&id)?;
    runs::detail(&state.pool, root)
        .await?
        .map(Json)
        .ok_or_else(ApiError::not_found)
}

/// 202: the task is being cancelled and its `run_finished` events arrive on SSE.
pub async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let root: RunId = query::path_id(&id)?;
    if state.engine.cancel(root) {
        return Ok(StatusCode::ACCEPTED);
    }
    if runs::root_exists(&state.pool, root).await? {
        Err(ApiError::conflict("this task has already finished"))
    } else {
        Err(ApiError::not_found())
    }
}
