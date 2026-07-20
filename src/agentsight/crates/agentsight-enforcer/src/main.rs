//! Standalone AgentSight enforcement daemon.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use agentsight_enforcer::{EnforcerService, MockBackend};

fn main() -> anyhow::Result<()> {
    let socket_path = std::env::var("AGENTSIGHT_ENFORCER_SOCKET")
        .unwrap_or_else(|_| "/run/agentsight/enforcer.sock".into());
    eprintln!("agentsight-enforcer is using the mock backend; kernel operations are not enforced");
    let service = EnforcerService::bind(socket_path, Arc::new(MockBackend::new()), None)?;
    service.serve_until(&AtomicBool::new(false))?;
    Ok(())
}
