//! `/api/runs`: start a task (202 with its root run id; progress arrives on SSE), list tasks,
//! read one task, follow up on a finished one, cancel one. A task is addressed by its root run id.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use orchestrator_core::engine::{FollowUpError, FollowUpRequest, StartError, StartRequest};
use orchestrator_core::ids::{AgentId, RunId};
use orchestrator_core::money::Micros;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::AppState;
use super::error::ApiError;
use super::json::ApiJson;
use super::query;
use crate::llm::S1_DECISION;
use crate::sqlite::runs::{self, RunDetail, RunList};
use crate::tools::workspace::EntryKind;

const DEFAULT_LIMIT: u32 = 50;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    limit: Option<u32>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, optional_fields)]
pub struct StartRunRequest {
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
    if request.scenario.as_deref() == Some("launder") {
        require_s1_decision(&state).await?;
    }
    let run_id = state
        .engine
        .start(StartRequest {
            agent_id: request.agent_id,
            task: request.task,
            title: request.title,
            budget: request.budget_micros,
            scenario: request.scenario,
            draft_only: request.draft_only.unwrap_or(false),
        })
        .await
        .map_err(start_error)?;
    Ok((StatusCode::ACCEPTED, Json(StartRunResponse { run_id })))
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

pub(super) fn start_error(error: StartError) -> ApiError {
    match error {
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
    let run_id = state
        .engine
        .follow_up(
            root,
            FollowUpRequest {
                text: body.text,
                scenario: body.scenario,
                draft_only: body.draft_only.unwrap_or(false),
            },
        )
        .await
        .map_err(follow_up_error)?;
    Ok((StatusCode::ACCEPTED, Json(StartRunResponse { run_id })))
}

fn follow_up_error(error: FollowUpError) -> ApiError {
    match error {
        FollowUpError::EmptyText => ApiError::invalid("text", "must not be empty"),
        FollowUpError::UnknownTask => ApiError::not_found(),
        FollowUpError::Running => ApiError::conflict("this task is still running"),
        FollowUpError::NotLead => {
            ApiError::conflict("only a task the Lead started takes follow-ups")
        }
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
    Ok(Json(runs::list(&state.pool, limit).await?))
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
