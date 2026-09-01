use std::fs;
use std::path::Path;

use crate::ClientError;

pub(crate) struct ManagementCredential {
    token: String,
}

impl ManagementCredential {
    pub(crate) fn load(path: &Path) -> Result<Self, ClientError> {
        let token = fs::read_to_string(path).map_err(|_| ClientError::CredentialUnavailable)?;
        let token = token.trim().to_owned();
        if !(32..=256).contains(&token.len()) {
            return Err(ClientError::CredentialUnavailable);
        }
        Ok(Self { token })
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}
