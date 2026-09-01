use std::sync::{Arc, mpsc};
use std::thread;

use asc_daemon_core::PolicyService;
use asc_persistence_sqlite::SqlitePolicyStore;
use asc_policy_runtime::PolicyAdapter;
use tracing::{error, info};

pub(crate) fn start<A>(policy: Arc<PolicyService<SqlitePolicyStore, A>>) -> mpsc::Sender<()>
where
    A: PolicyAdapter + 'static,
{
    let (signal, receiver) = mpsc::channel();
    thread::spawn(move || run(policy, receiver));
    signal
}

#[allow(clippy::needless_pass_by_value)] // the spawned thread owns both handles
fn run<A>(policy: Arc<PolicyService<SqlitePolicyStore, A>>, receiver: mpsc::Receiver<()>)
where
    A: PolicyAdapter,
{
    while receiver.recv().is_ok() {
        loop {
            match policy.dispatch_once() {
                Ok(Some(operation)) => info!(
                    operation_id = %operation.operation_id,
                    binding_id = %operation.binding_id,
                    state = ?operation.state,
                    "policy Adapter dispatch finished"
                ),
                Ok(None) => break,
                Err(problem) => {
                    error!(
                        error_code = "policy_dispatch_persistence",
                        "outbox dispatch failed: {problem}"
                    );
                    break;
                }
            }
        }
    }
}
