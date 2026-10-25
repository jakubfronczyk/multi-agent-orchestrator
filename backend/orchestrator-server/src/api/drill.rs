//! `POST /api/drill` starts the seeded security drill as a task led by the Lead;
//! `GET /api/drill/{run_id}/report` lists every exit the task tried and who decided it.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use orchestrator_core::engine::StartRequest;
use orchestrator_core::ids::{RunId, TeamId};
use serde::Deserialize;

use super::error::ApiError;
use super::runs::{StartRunResponse, missing_slugs, resolve_team, start_in};
use super::{AppState, query};
use crate::config;
use crate::llm::DRILL;
use crate::sqlite::drill::{self, DrillReport};

#[derive(Debug, Deserialize)]
pub struct StartQuery {
    team_id: Option<String>,
}

/// Always scripted: it plays a model that obeys the planted note, so the gates are what stop it.
/// It runs on the named team, the first team by default, if its Lead reaches the script's agents.
pub async fn start(
    State(state): State<AppState>,
    Query(query): Query<StartQuery>,
) -> Result<(StatusCode, Json<StartRunResponse>), ApiError> {
    let team = query::id::<TeamId>("team_id", query.team_id)?;
    let team = resolve_team(&state, team, None).await?;
    let missing = missing_slugs(&state, team, DRILL).await?;
    if !missing.is_empty() {
        let message = format!(
            "this team's Lead cannot hand work to {}; the drill needs them",
            missing.join(", ")
        );
        return Err(ApiError::conflict(&message));
    }
    let request = StartRequest {
        team_id: Some(team),
        agent_id: None,
        task: config::DRILL_TASK.to_string(),
        title: Some(config::DRILL_TITLE.to_string()),
        budget: None,
        scenario: Some(DRILL.to_string()),
        draft_only: false,
    };
    let run_id = start_in(&state, team, request).await?;
    Ok((StatusCode::ACCEPTED, Json(StartRunResponse { run_id })))
}

pub async fn report(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DrillReport>, ApiError> {
    let root: RunId = query::path_id(&id)?;
    drill::report(&state.pool, root)
        .await?
        .map(Json)
        .ok_or_else(ApiError::not_found)
}
