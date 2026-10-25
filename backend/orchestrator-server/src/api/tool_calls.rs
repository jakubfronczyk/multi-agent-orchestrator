//! `GET /api/tool-calls`: the audit log of every call, filtered by run, agent, tool or status.

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use super::AppState;
use super::error::ApiError;
use crate::sqlite::runs::{self, RunReadError, ToolCallFilter, ToolCallList};
use crate::sqlite::text::from_text;

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 200;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    run_id: Option<String>,
    agent_id: Option<String>,
    tool: Option<String>,
    status: Option<String>,
    limit: Option<u32>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ToolCallList>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(ApiError::invalid("limit", "use 1 to 200"));
    }
    let filter = ToolCallFilter {
        run_id: query
            .run_id
            .map(|id| {
                id.parse()
                    .map_err(|_| ApiError::invalid("run_id", "not a run id"))
            })
            .transpose()?,
        agent_id: query
            .agent_id
            .map(|id| {
                id.parse()
                    .map_err(|_| ApiError::invalid("agent_id", "not an agent id"))
            })
            .transpose()?,
        tool: query.tool,
        status: query
            .status
            .map(|raw| {
                from_text(&raw).map_err(|_| ApiError::invalid("status", "unknown call status"))
            })
            .transpose()?,
    };
    Ok(Json(
        runs::tool_calls(&state.pool, filter, limit)
            .await
            .map_err(read_error)?,
    ))
}

fn read_error(error: RunReadError) -> ApiError {
    tracing::error!(%error, "tool call read failed");
    ApiError::internal()
}
