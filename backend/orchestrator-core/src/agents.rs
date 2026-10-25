//! Agent identity types and the validation error every constructor returns.
//! A `Slug` can only be built through `Slug::new`, so an invalid one never exists.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A field-level validation failure; the API turns it into `{error: {code, message, field}}`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {message}")]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

impl ValidationError {
    pub fn new(field: &str, message: &str) -> Self {
        Self {
            field: field.to_string(),
            message: message.to_string(),
        }
    }
}

/// An agent's fixed identifier, the name the model sees: `[a-z0-9_-]{1,32}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(try_from = "String")]
#[ts(export, type = "string")]
pub struct Slug(String);

impl Slug {
    pub fn new(value: &str) -> Result<Self, ValidationError> {
        let valid_char =
            |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-';
        if value.is_empty() || value.len() > 32 || !value.chars().all(valid_char) {
            return Err(ValidationError::new(
                "slug",
                "use 1 to 32 of a-z, 0-9, _ and -",
            ));
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Slug {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Slug::new(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_accepts_valid_and_rejects_invalid() {
        assert!(Slug::new("reader_2-a").is_ok());
        for bad in ["", "Reader", "has space", "dot.ted", &"a".repeat(33)] {
            assert!(Slug::new(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn slug_deserialize_validates() {
        assert!(serde_json::from_str::<Slug>("\"ok\"").is_ok());
        assert!(serde_json::from_str::<Slug>("\"NOT OK\"").is_err());
    }
}
