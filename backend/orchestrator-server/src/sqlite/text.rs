//! Ids, enums and timestamps are stored as text in one fixed form, so the database and the API agree.
//! A value whose serde form is not a bare string is an error here; structured values are stored as JSON.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde::ser::Error as _;
use serde_json::Value;

pub fn to_text<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    match serde_json::to_value(value)? {
        Value::String(text) => Ok(text),
        other => Err(serde_json::Error::custom(format!(
            "{other} is not a text value"
        ))),
    }
}

pub fn from_text<T: DeserializeOwned>(text: &str) -> Result<T, serde_json::Error> {
    serde_json::from_value(Value::String(text.to_string()))
}

/// RFC 3339 in UTC with microseconds, the precision every stored time uses.
pub fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Micros, true)
}

pub fn parse_timestamp(text: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
    DateTime::parse_from_rfc3339(text).map(|time| time.with_timezone(&Utc))
}
