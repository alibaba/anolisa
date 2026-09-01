use std::sync::{Arc, mpsc};

use asc_daemon_core::PolicyService;
use asc_persistence_sqlite::SqlitePolicyStore;
use asc_policy_runtime::PolicyAdapter;

use crate::auth::TokenVerifier;
use crate::worker;

/// Process application state shared by connection handlers.
pub struct AppState<A> {
    policy: Arc<PolicyService<SqlitePolicyStore, A>>,
    auth: Arc<TokenVerifier>,
    worker_signal: mpsc::Sender<()>,
}

impl<A> Clone for AppState<A> {
    fn clone(&self) -> Self {
        Self {
            policy: Arc::clone(&self.policy),
            auth: Arc::clone(&self.auth),
            worker_signal: self.worker_signal.clone(),
        }
    }
}

impl<A> AppState<A>
where
    A: PolicyAdapter + 'static,
{
    /// Creates state and starts the event-driven Policy outbox worker.
    pub fn new(policy: Arc<PolicyService<SqlitePolicyStore, A>>, auth: Arc<TokenVerifier>) -> Self {
        let worker_signal = worker::start(Arc::clone(&policy));
        let state = Self {
            policy,
            auth,
            worker_signal,
        };
        state.wake_policy_worker();
        state
    }

    pub(crate) fn policy(&self) -> &PolicyService<SqlitePolicyStore, A> {
        &self.policy
    }

    pub(crate) fn auth(&self) -> &TokenVerifier {
        &self.auth
    }

    pub(crate) fn wake_policy_worker(&self) {
        let _ = self.worker_signal.send(());
    }
}
