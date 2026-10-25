//! What every API read shares: one error type and the decoders from stored text.
//! A stored value that does not decode is `Corrupt`, never a default.

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;

use super::text::{from_text, parse_timestamp};

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("storage failed: {0}")]
    Storage(#[from] sqlx::Error),
    #[error("stored data is unreadable: {0}")]
    Corrupt(String),
}

pub(crate) fn corrupt(error: impl std::fmt::Display) -> ReadError {
    ReadError::Corrupt(error.to_string())
}

/// A nullable JSON column.
pub(crate) fn json_opt<T: DeserializeOwned>(text: Option<&str>) -> Result<Option<T>, ReadError> {
    text.map(serde_json::from_str).transpose().map_err(corrupt)
}

/// A nullable id or enum column stored through `to_text`.
pub(crate) fn text_opt<T: DeserializeOwned>(text: Option<&str>) -> Result<Option<T>, ReadError> {
    text.map(from_text).transpose().map_err(corrupt)
}

pub(crate) fn time_opt(text: Option<&str>) -> Result<Option<DateTime<Utc>>, ReadError> {
    text.map(parse_timestamp).transpose().map_err(corrupt)
}

pub(crate) fn count(value: i64) -> Result<u32, ReadError> {
    u32::try_from(value).map_err(corrupt)
}
