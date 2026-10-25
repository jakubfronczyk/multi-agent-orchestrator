//! The server's tools. Each gets its database pool, paths or HTTP client through its
//! constructor; core sees only the `Tool` trait and the `ToolSpec`.

use std::time::Duration;

use orchestrator_core::policy::{Trust, untrusted};
use orchestrator_core::tools::{MAY_ASK, ToolError, ToolName, ToolSpec, schema_for};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::sqlite::text::from_text;

pub mod files;
pub mod memory;
mod sandbox;

/// The one argument parser every tool uses in both `check` and `run`.
pub(crate) fn parse<T: DeserializeOwned>(args: &Value) -> Result<T, ToolError> {
    serde_json::from_value(args.clone()).map_err(|error| ToolError::InvalidArgs(error.to_string()))
}

/// The schema comes from the argument struct; its field docs are what the model reads.
pub(crate) fn spec<T: JsonSchema>(name: ToolName, purpose: &str, timeout: Duration) -> ToolSpec {
    ToolSpec {
        name,
        description: format!("{purpose} {MAY_ASK}"),
        schema: schema_for::<T>(),
        timeout: Some(timeout),
    }
}

/// Stored text goes back as data unless its stored trust says trusted. A missing row or an
/// unknown value counts as untrusted, so a bad row fails closed.
pub(crate) fn as_stored(source: &str, content: &str, trust: Option<&str>) -> String {
    if is_trusted(trust) {
        content.to_string()
    } else {
        untrusted(source, content)
    }
}

pub(crate) fn is_trusted(trust: Option<&str>) -> bool {
    match trust.map(from_text::<Trust>) {
        Some(Ok(Trust::Trusted)) => true,
        Some(Ok(Trust::Untrusted)) | Some(Err(_)) | None => false,
    }
}

/// Storage errors reach the model as a fixed message, so no stored text leaks through one.
pub(crate) fn storage_failed(error: impl std::fmt::Display) -> ToolError {
    tracing::error!(%error, "tool storage failed");
    ToolError::Failed("storage failed".to_string())
}
