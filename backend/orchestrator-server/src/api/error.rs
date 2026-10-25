//! One error shape for every handler, with a code the UI can switch on.
//! Storage failures reach the client as `internal`; the detail stays in the server log.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use ts_rs::TS;

use crate::sqlite::agents::AgentError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ErrorCode {
    InvalidInput,
    NotFound,
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
    status: StatusCode,
    detail: ErrorDetail,
}

impl ApiError {
    fn new(status: StatusCode, code: ErrorCode, message: String, field: Option<String>) -> Self {
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
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::InvalidInput,
            message.to_string(),
            Some(field.to_string()),
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
            AgentError::Invalid(invalid) => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::InvalidInput,
                invalid.message,
                Some(invalid.field),
            ),
            AgentError::NotFound => Self::not_found(),
            AgentError::Storage(_) | AgentError::Corrupt(_) => {
                tracing::error!(%error, "agent storage failed");
                Self::internal()
            }
        }
    }
}

/// A JSON body whose structural errors (wrong type, missing field, unknown enum value)
/// carry the failing path as `field`; rules on valid values are checked later, in core.
pub struct ApiJson<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let is_json = request
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json"));
        if !is_json {
            return Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                ErrorCode::InvalidInput,
                "expected an application/json body".to_string(),
                None,
            ));
        }
        let bytes = Bytes::from_request(request, state)
            .await
            .map_err(|rejection| {
                ApiError::new(
                    rejection.status(),
                    ErrorCode::InvalidInput,
                    rejection.body_text(),
                    None,
                )
            })?;
        parse_json(&bytes).map(ApiJson)
    }
}

fn parse_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    let invalid = |message: String, field: Option<String>| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::InvalidInput,
            message,
            field,
        )
    };
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        // A rejected map key has no name in the path ("tools.?"); the message names it.
        let path = error.path().to_string();
        let path = path.trim_end_matches(".?");
        invalid(
            error.inner().to_string(),
            (path != ".").then(|| path.to_string()),
        )
    })?;
    deserializer
        .end()
        .map_err(|error| invalid(error.to_string(), None))?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use orchestrator_core::agents::AgentInput;

    use super::*;

    const BODY: &str = r#"{"role":"specialist","slug":"reader","name":"R","description":"d",
        "system_prompt":"p","model":"grok-4.3","icon":"i","colour":"c","tools":{TOOLS},"delegate_targets":[]}"#;

    #[test]
    fn unknown_tool_key_names_its_path() {
        let body = BODY.replace("TOOLS", r#""teleport":"allow""#);
        let error = parse_json::<AgentInput>(body.as_bytes()).expect_err("rejected");
        assert_eq!(error.detail.field.as_deref(), Some("tools"));
        assert!(
            error.detail.message.contains("teleport"),
            "{}",
            error.detail.message
        );
    }

    #[test]
    fn unknown_model_names_its_path() {
        let body = BODY.replace("TOOLS", "").replace("grok-4.3", "gpt-9");
        let error = parse_json::<AgentInput>(body.as_bytes()).expect_err("rejected");
        assert_eq!(error.detail.field.as_deref(), Some("model"));
    }
}
