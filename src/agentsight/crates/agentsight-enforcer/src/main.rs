//! Standalone AgentSight enforcement daemon.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "actplane")]
use agentsight_enforcer::ActPlaneBackend;
use agentsight_enforcer::EnforcerService;
#[cfg(all(feature = "mock-backend", not(feature = "actplane")))]
use agentsight_enforcer::MockBackend;

fn main() -> anyhow::Result<()> {
    let socket_path = std::env::var("AGENTSIGHT_ENFORCER_SOCKET")
        .unwrap_or_else(|_| "/run/agentsight/enforcer.sock".into());
    run(socket_path)
}

fn termination_flag() -> anyhow::Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    let signal_stop = Arc::clone(&stop);
    ctrlc::set_handler(move || signal_stop.store(true, Ordering::Release))?;
    Ok(stop)
}

#[cfg(feature = "actplane")]
fn run(socket_path: String) -> anyhow::Result<()> {
    let stop = termination_flag()?;
    let service = EnforcerService::bind(socket_path, Arc::new(ActPlaneBackend::open()?), None)?;
    service.serve_until(stop.as_ref())?;
    Ok(())
}

#[cfg(all(feature = "mock-backend", not(feature = "actplane")))]
fn run(socket_path: String) -> anyhow::Result<()> {
    eprintln!("agentsight-enforcer is using the mock backend; kernel operations are not enforced");
    let stop = termination_flag()?;
    let service = EnforcerService::bind(socket_path, Arc::new(MockBackend::new()), None)?;
    service.serve_until(stop.as_ref())?;
    Ok(())
}

#[cfg(not(any(feature = "mock-backend", feature = "actplane")))]
compile_error!("agentsight-enforcer requires the mock-backend or actplane feature");
