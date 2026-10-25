//! Ids and enums are stored as their serde text form, so the database and the API agree.
//! A value that serializes to a JSON string is stored bare; anything else as JSON.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub fn to_text<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    Ok(match serde_json::to_value(value)? {
        Value::String(text) => text,
        other => other.to_string(),
    })
}

/// The inverse of `to_text`: a bare string first, then JSON.
pub fn from_text<T: DeserializeOwned>(text: &str) -> Result<T, serde_json::Error> {
    serde_json::from_value(Value::String(text.to_string())).or_else(|_| serde_json::from_str(text))
}
