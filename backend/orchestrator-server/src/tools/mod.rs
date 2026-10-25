//! The server's tools. Each gets its database pool, paths or HTTP client through its
//! constructor; core sees only the `Tool` trait and the `ToolSpec`.

use std::time::Duration;

use orchestrator_core::tools::{MAY_ASK, ToolError, ToolName, ToolSpec, schema_for};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub mod memory;

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

/// Storage errors reach the model as a fixed message, so no stored text leaks through one.
pub(crate) fn storage_failed(error: impl std::fmt::Display) -> ToolError {
    tracing::error!(%error, "tool storage failed");
    ToolError::Failed("storage failed".to_string())
}
