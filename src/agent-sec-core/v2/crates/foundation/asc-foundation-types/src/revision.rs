use serde::{Deserialize, Serialize};

/// Positive immutable revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Revision(u64);

impl Revision {
    /// Builds a positive revision.
    ///
    /// # Errors
    /// Revision zero is invalid.
    pub const fn new(value: u64) -> Result<Self, RevisionError> {
        if value == 0 {
            Err(RevisionError)
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the numeric revision.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Invalid revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("revision must be greater than zero")]
pub struct RevisionError;
