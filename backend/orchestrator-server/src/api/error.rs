//! One error shape for every handler, with a code the UI can switch on.
//! Storage failures reach the client as `internal`; the detail stays in the server log.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use ts_rs::TS;

use crate::sqlite::agents::AgentError;
use crate::sqlite::read::ReadError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ErrorCode {
    InvalidInput,
    /// The request came from another site or through a non-loopback host name.
    Forbidden,
    NotFound,
    Conflict,
    Internal,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ErrorDetail {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub field: Option<String>,
}

#[derive(Debug)]
pub struct ApiError {
    pub(super) status: StatusCode,
    pub(super) detail: ErrorDetail,
}

impl ApiError {
    pub(super) fn new(
        status: StatusCode,
        code: ErrorCode,
        message: String,
        field: Option<String>,
    ) -> Self {
        Self {
            status,
            detail: ErrorDetail {
                code,
                message,
                field,
            },
        }
    }

    pub fn invalid(field: &str, message: &str) -> Self {
        Self::unprocessable(message.to_string(), Some(field.to_string()))
    }

    pub(super) fn unprocessable(message: String, field: Option<String>) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::InvalidInput,
            message,
            field,
        )
    }

    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            "internal error".to_string(),
            None,
        )
    }

    pub fn conflict(message: &str) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            message.to_string(),
            None,
        )
    }

    pub fn forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
            "requests must come from this app on this machine".to_string(),
            None,
        )
    }

    pub fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
            "not found".to_string(),
            None,
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(ErrorBody { error: self.detail })).into_response()
    }
}

impl From<AgentError> for ApiError {
    fn from(error: AgentError) -> Self {
        match error {
            AgentError::Invalid(invalid) => {
                Self::unprocessable(invalid.message, Some(invalid.field))
            }
            AgentError::NotFound => Self::not_found(),
            AgentError::Storage(_) | AgentError::Corrupt(_) => {
                tracing::error!(%error, "agent storage failed");
                Self::internal()
            }
        }
    }
}

/// A failed read is the server's fault: the detail is logged, the client sees `internal`.
impl From<ReadError> for ApiError {
    fn from(error: ReadError) -> Self {
        tracing::error!(%error, "read failed");
        ApiError::internal()
    }
}
