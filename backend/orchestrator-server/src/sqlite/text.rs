//! Ids and enums are stored as their serde text form, so the database and the API agree.
//! A value that serializes to a JSON string is stored bare; anything else as JSON.

use serde::Serialize;
use serde_json::Value;

pub fn to_text<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    Ok(match serde_json::to_value(value)? {
        Value::String(text) => text,
        other => other.to_string(),
    })
}
