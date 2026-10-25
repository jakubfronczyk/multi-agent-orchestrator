//! Query-string values for the list handlers. Each one that does not parse is a 422 naming
//! its field, so a typo never silently widens a filter.

use std::str::FromStr;

use serde::de::DeserializeOwned;

use super::error::ApiError;
use crate::sqlite::text::from_text;

const MAX_LIMIT: u32 = 200;

/// `default` when absent; 1 to 200 otherwise.
pub fn limit(raw: Option<u32>, default: u32) -> Result<u32, ApiError> {
    let limit = raw.unwrap_or(default);
    if (1..=MAX_LIMIT).contains(&limit) {
        Ok(limit)
    } else {
        Err(ApiError::invalid("limit", "use 1 to 200"))
    }
}

/// A malformed id in the path cannot name anything, so it is a 404 like an unknown one.
pub fn path_id<T: FromStr>(raw: &str) -> Result<T, ApiError> {
    raw.parse().map_err(|_| ApiError::not_found())
}

/// An id, parsed through its `FromStr`.
pub fn id<T: FromStr>(field: &str, raw: Option<String>) -> Result<Option<T>, ApiError> {
    raw.map(|raw| {
        raw.parse()
            .map_err(|_| ApiError::invalid(field, "not a valid id"))
    })
    .transpose()
}

/// `true` or `false`; absent is false.
pub fn flag(field: &str, raw: Option<String>) -> Result<bool, ApiError> {
    match raw.as_deref() {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err(ApiError::invalid(field, "use true or false")),
    }
}

/// An enum value in its stored text form.
pub fn value<T: DeserializeOwned>(field: &str, raw: Option<String>) -> Result<Option<T>, ApiError> {
    raw.map(|raw| from_text(&raw).map_err(|_| ApiError::invalid(field, "unknown value")))
        .transpose()
}
