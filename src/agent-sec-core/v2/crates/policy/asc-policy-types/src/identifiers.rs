//! Strong identifiers, revisions, and content digests.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

fn validate_identifier(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("must not be empty".to_owned());
    }
    if value.len() > 256 {
        return Err("must not exceed 256 bytes".to_owned());
    }
    if !value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "-_.:".contains(character))
    {
        return Err("contains unsupported characters".to_owned());
    }
    Ok(())
}

fn validate_profile_identifier(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("must not be empty".to_owned());
    }
    if value.len() > 256 {
        return Err("must not exceed 256 bytes".to_owned());
    }
    if !value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "-_.:/".contains(character))
    {
        return Err("contains unsupported profile identifier characters".to_owned());
    }
    Ok(())
}

macro_rules! identifier {
    ($name:ident, $doc:literal) => {
        identifier!($name, $doc, validate_identifier);
    };
    ($name:ident, $doc:literal, $validator:path) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Creates a validated identifier.
            ///
            /// # Errors
            /// Returns an error when the value is empty, too long, or contains
            /// a character outside the closed wire alphabet.
            pub fn new(value: impl Into<String>) -> Result<Self, String> {
                Self::try_from(value.into())
            }

            /// Returns the wire value.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = String;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                $validator(&value)?;
                Ok(Self(value))
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::try_from(value).map_err(serde::de::Error::custom)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

identifier!(PolicyId, "Stable policy identifier.");
identifier!(
    ProfileId,
    "Immutable Canonical Policy IR profile identifier.",
    validate_profile_identifier
);
identifier!(RuleId, "Stable rule identifier within one policy revision.");
identifier!(
    ResourceSetId,
    "Stable resource-set identifier within one policy revision."
);
identifier!(ResourceId, "Stable protected-resource identifier.");
identifier!(Label, "Backend-independent sensitive-data label.");
/// Monotonically increasing immutable revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Revision(u64);

impl Revision {
    /// Creates a non-zero revision.
    ///
    /// # Errors
    /// Returns an error when `value` is zero.
    pub const fn new(value: u64) -> Result<Self, &'static str> {
        if value == 0 {
            Err("revision must be greater than zero")
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the numeric revision.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for Revision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Lowercase SHA-256 digest encoded as `sha256:<64 hex characters>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Digest(String);

impl Digest {
    /// Parses and validates a SHA-256 digest.
    ///
    /// # Errors
    /// Returns an error unless the value is `sha256:` followed by exactly 64
    /// lowercase hexadecimal characters.
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        Self::try_from(value.into())
    }

    /// Returns the wire value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Digest {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let Some(hex) = value.strip_prefix("sha256:") else {
            return Err("digest must use the sha256 algorithm prefix".to_owned());
        };
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(
                "digest must contain exactly 64 lowercase hexadecimal characters".to_owned(),
            );
        }
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
