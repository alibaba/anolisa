use std::fmt;

use serde::{Deserialize, Serialize};

/// A bounded non-empty identifier carried across process boundaries.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourceId(String);

impl ResourceId {
    /// Builds an identifier accepted by the policy control plane.
    ///
    /// # Errors
    /// Returns an error for empty, oversized, or unsafe identifiers.
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        if value.is_empty() || value.len() > 128 {
            return Err(IdentifierError);
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
        {
            return Err(IdentifierError);
        }
        Ok(Self(value))
    }

    /// Returns the wire representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ResourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Invalid identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("identifier must be 1..=128 ASCII letters, digits, '.', ':', '_' or '-'")]
pub struct IdentifierError;
