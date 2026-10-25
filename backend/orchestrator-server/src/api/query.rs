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

/// An id, parsed through its `FromStr`.
pub fn id<T: FromStr>(field: &str, raw: Option<String>) -> Result<Option<T>, ApiError> {
    raw.map(|raw| {
        raw.parse()
            .map_err(|_| ApiError::invalid(field, "not a valid id"))
    })
    .transpose()
}

/// An enum value in its stored text form.
pub fn value<T: DeserializeOwned>(field: &str, raw: Option<String>) -> Result<Option<T>, ApiError> {
    raw.map(|raw| from_text(&raw).map_err(|_| ApiError::invalid(field, "unknown value")))
        .transpose()
}
