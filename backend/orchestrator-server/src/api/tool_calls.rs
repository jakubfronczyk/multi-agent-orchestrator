//! `GET /api/tool-calls`: the audit log of every call, filtered by run, agent, tool or status.

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use super::AppState;
use super::error::ApiError;
use super::query;
use crate::sqlite::tool_calls::{self, ToolCallFilter, ToolCallList};

const DEFAULT_LIMIT: u32 = 100;

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
    let limit = query::limit(query.limit, DEFAULT_LIMIT)?;
    let filter = ToolCallFilter {
        run_id: query::id("run_id", query.run_id)?,
        agent_id: query::id("agent_id", query.agent_id)?,
        tool: query.tool,
        status: query::value("status", query.status)?,
    };
    Ok(Json(
        tool_calls::tool_calls(&state.pool, filter, limit).await?,
    ))
}
