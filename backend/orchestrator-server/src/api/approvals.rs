//! `/api/approvals`: list cards by status and answer one. A second answer is a 409.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use orchestrator_core::ids::ApprovalId;
use orchestrator_core::runs::Answer;
use serde::Deserialize;
use ts_rs::TS;

use super::AppState;
use super::error::ApiError;
use super::json::ApiJson;
use super::query;
use crate::sqlite::approvals::{self, ApprovalList};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    status: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AnswerDecision {
    Approve,
    Deny,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, optional_fields)]
pub struct AnswerApprovalRequest {
    pub decision: AnswerDecision,
    pub note: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ApprovalList>, ApiError> {
    let status = query::value("status", query.status)?;
    Ok(Json(approvals::list(&state.pool, status).await?))
}

pub async fn answer(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<AnswerApprovalRequest>,
) -> Result<StatusCode, ApiError> {
    let id: ApprovalId = query::path_id(&id)?;
    let answer = match request.decision {
        AnswerDecision::Approve => Answer::Approve { note: request.note },
        AnswerDecision::Deny => Answer::Deny { note: request.note },
    };
    if state.engine.answer(id, answer) {
        return Ok(StatusCode::NO_CONTENT);
    }
    match approvals::status(&state.pool, id).await? {
        Some(_) => Err(ApiError::conflict(
            "this approval is no longer waiting for an answer",
        )),
        None => Err(ApiError::not_found()),
    }
}
