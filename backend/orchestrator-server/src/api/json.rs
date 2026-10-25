//! The JSON body extractor: structural input errors carry the failing path as `field`.
//! Rules on well-formed values are checked later, in core.

use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use serde::de::DeserializeOwned;

use super::error::{ApiError, ErrorCode};

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
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        // A rejected map key has no name in the path ("tools.?"); the message names it.
        let path = error.path().to_string();
        let path = path.trim_end_matches(".?");
        ApiError::unprocessable(
            error.inner().to_string(),
            (path != ".").then(|| path.to_string()),
        )
    })?;
    deserializer
        .end()
        .map_err(|error| ApiError::unprocessable(error.to_string(), None))?;
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
