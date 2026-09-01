use std::fmt;

use serde::{Deserialize, Serialize};

/// Management bearer credential. Debug output is always redacted.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BearerAuth {
    /// Must be `bearer`.
    pub scheme: String,
    /// Secret token.
    pub token: String,
}

impl fmt::Debug for BearerAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BearerAuth")
            .field("scheme", &self.scheme)
            .field("token", &"[REDACTED]")
            .finish()
    }
}
