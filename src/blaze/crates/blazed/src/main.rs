// SPDX-License-Identifier: Apache-2.0
//! `blazed` binary entry point.
//!
//! blazed is daemon-only. All sandbox management operations are
//! exposed via the HTTP API; this binary only handles daemon lifecycle.

mod api;
mod checkpoint_store;
mod cli;
mod daemon;
mod error;
#[cfg(feature = "test-failpoints")]
mod failpoint;
#[cfg(not(feature = "test-failpoints"))]
#[path = "failpoint_disabled.rs"]
mod failpoint;
mod file_provider;
mod guest;
mod metrics;
mod sandbox;
mod spawner;
mod state;
mod state_store;

use std::future::Future;
use std::num::NonZeroUsize;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::cli::{Cli, Command, DaemonAction};
use crate::error::Result;

const MIN_RUNTIME_WORKERS: usize = 2;
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

fn main() -> ExitCode {
    init_tracing();
    failpoint::announce();

    let cli = Cli::parse();
    let worker_threads = runtime_worker_threads(std::thread::available_parallelism().ok());
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("blazed: build async runtime: {error}");
            return ExitCode::from(1);
        }
    };
    run_with_bounded_runtime(runtime, run_cli(cli), RUNTIME_SHUTDOWN_TIMEOUT)
}

fn run_with_bounded_runtime<F>(
    runtime: tokio::runtime::Runtime,
    future: F,
    shutdown_timeout: Duration,
) -> F::Output
where
    F: Future,
{
    let output = runtime.block_on(future);
    runtime.shutdown_timeout(shutdown_timeout);
    output
}

async fn run_cli(cli: Cli) -> ExitCode {
    let outcome = run(cli).await;
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("blazed: {err}");
            ExitCode::from(1)
        }
    }
}

fn runtime_worker_threads(available: Option<NonZeroUsize>) -> usize {
    available
        .map_or(MIN_RUNTIME_WORKERS, NonZeroUsize::get)
        .max(MIN_RUNTIME_WORKERS)
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Daemon(action) => match action {
            DaemonAction::Start { config } => daemon::run(&config).await,
            DaemonAction::Reload { socket } => {
                println!("Sending reload signal to daemon at {}", socket.display());
                // In v0.1 just print guidance; actual signal delivery deferred.
                println!("  hint: kill -HUP $(pidof blazed)");
                Ok(())
            }
            DaemonAction::Doctor { config } => {
                let config_path = config.unwrap_or_else(|| "/etc/anolisa/blaze/config.toml".into());
                println!("blazed doctor");
                println!("  config : {}", config_path.display());
                match blaze_core::config::DaemonConfig::load(&config_path) {
                    Ok(_) => println!("  config parse : ok"),
                    Err(e) => println!("  config parse : FAIL ({e})"),
                }
                Ok(())
            }
        },
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let layer = fmt::layer()
        .json()
        .with_target(true)
        .with_current_span(false);
    tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .init();
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    use super::*;

    const RUNTIME_SHUTDOWN_CHILD: &str = "BLAZED_RUNTIME_SHUTDOWN_CHILD";

    #[test]
    fn runtime_keeps_a_control_worker_on_single_cpu_hosts() {
        assert_eq!(
            runtime_worker_threads(NonZeroUsize::new(1)),
            MIN_RUNTIME_WORKERS
        );
    }

    #[test]
    fn runtime_uses_available_parallelism_above_the_minimum() {
        assert_eq!(runtime_worker_threads(NonZeroUsize::new(8)), 8);
    }

    #[test]
    fn runtime_shutdown_timeout_bounds_non_yielding_task() {
        if std::env::var_os(RUNTIME_SHUTDOWN_CHILD).is_some() {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(MIN_RUNTIME_WORKERS)
                .enable_all()
                .build()
                .expect("test runtime");
            let (started_tx, started_rx) = mpsc::sync_channel(0);

            run_with_bounded_runtime(
                runtime,
                async move {
                    tokio::spawn(async move {
                        started_tx.send(()).expect("report task start");
                        loop {
                            std::hint::spin_loop();
                        }
                    });
                    started_rx
                        .recv_timeout(Duration::from_secs(1))
                        .expect("non-yielding task did not start");
                },
                Duration::from_millis(50),
            );
            println!("runtime-shutdown-child-complete");
            return;
        }

        let mut child = Command::new(std::env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "tests::runtime_shutdown_timeout_bounds_non_yielding_task",
                "--nocapture",
            ])
            .env(RUNTIME_SHUTDOWN_CHILD, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn runtime-shutdown child");

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().expect("poll runtime-shutdown child") {
                let mut stdout = String::new();
                child
                    .stdout
                    .take()
                    .expect("runtime-shutdown child stdout")
                    .read_to_string(&mut stdout)
                    .expect("read runtime-shutdown child stdout");
                let mut stderr = String::new();
                child
                    .stderr
                    .take()
                    .expect("runtime-shutdown child stderr")
                    .read_to_string(&mut stderr)
                    .expect("read runtime-shutdown child stderr");
                assert!(
                    status.success(),
                    "runtime-shutdown child failed: {status}; stdout={stdout}; stderr={stderr}"
                );
                assert!(
                    stdout.contains("runtime-shutdown-child-complete"),
                    "child test did not exercise runtime teardown; stdout={stdout}; stderr={stderr}"
                );
                break;
            }
            if Instant::now() >= deadline {
                child.kill().expect("kill stuck runtime-shutdown child");
                child.wait().expect("reap stuck runtime-shutdown child");
                panic!("runtime teardown exceeded the test deadline");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}
