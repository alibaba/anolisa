use std::sync::Mutex;

use crate::adapter::{AdapterDispatchError, PolicyAdapter};
use crate::model::{AdapterAccepted, AdapterCommand};

/// Recording Adapter for contract and integration tests.
#[derive(Debug, Default)]
pub struct FakePolicyAdapter {
    commands: Mutex<Vec<AdapterCommand>>,
}

impl FakePolicyAdapter {
    /// Returns recorded commands without exposing mutable shared state.
    pub fn commands(&self) -> Vec<AdapterCommand> {
        self.commands
            .lock()
            .map_or_else(|_| Vec::new(), |values| values.clone())
    }
}

impl PolicyAdapter for FakePolicyAdapter {
    fn submit(&self, command: &AdapterCommand) -> Result<AdapterAccepted, AdapterDispatchError> {
        self.commands
            .lock()
            .map_err(|_| AdapterDispatchError::Retryable)?
            .push(command.clone());
        Ok(AdapterAccepted)
    }
}
