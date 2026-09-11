use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use asc_daemon::{Cli, ParseOutcome, ProcessSignals, run_with_shutdown_timeout, serve};
use asc_daemon_core::{PrincipalPolicy, RootManagedPrincipalPolicy};
use asc_daemon_handler::{DaemonDispatcher, JsonRejectionEncoder};
use asc_daemon_service::ShutdownToken;
use asc_pap::PapService;
use asc_pap_repository_memory::ProcessLocalPapRepository;
use asc_policy_engine::PolicyTemplateCompiler;
use asc_policy_runtime::reconciliation::ReconciliationRuntime;

const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

fn main() -> ExitCode {
    match run_with_shutdown_timeout(run(), RUNTIME_SHUTDOWN_TIMEOUT) {
        Ok(exit_code) => exit_code,
        Err(problem) => {
            report_error(&problem);
            ExitCode::FAILURE
        }
    }
}

async fn run() -> ExitCode {
    let outcome = match Cli::parse_from(std::env::args_os()) {
        Ok(outcome) => outcome,
        Err(problem) => {
            eprintln!("agent-sec-daemon: {problem}");
            return ExitCode::from(2);
        }
    };
    let ParseOutcome::Serve(cli) = outcome else {
        let ParseOutcome::Help(help) = outcome else {
            unreachable!("all parse outcomes are covered")
        };
        print!("{help}");
        return ExitCode::SUCCESS;
    };

    let signals = match ProcessSignals::install() {
        Ok(signals) => signals,
        Err(problem) => {
            eprintln!("agent-sec-daemon: {problem}");
            return ExitCode::FAILURE;
        }
    };
    let repository = Arc::new(ProcessLocalPapRepository::default());
    let policy_runtime = match asc_daemon::start_policy_reconciliation(repository.clone()) {
        Ok(runtime) => Some(runtime),
        Err(error) => {
            eprintln!("asc-daemon: reconciliation unavailable; Binding mutations disabled");
            report_error(&error);
            None
        }
    };
    let enqueuer: Arc<dyn asc_pap::BindingReconcileEnqueuer> = policy_runtime.as_ref().map_or_else(
        || {
            Arc::new(asc_daemon::UnavailableReconciliation)
                as Arc<dyn asc_pap::BindingReconcileEnqueuer>
        },
        |runtime| runtime.enqueuer(),
    );
    let pap = PapService::new(repository, Arc::new(PolicyTemplateCompiler))
        .with_reconcile_enqueuer(enqueuer);
    let principal_policy = Arc::new(RootManagedPrincipalPolicy::with_admin_uids(
        cli.policy_admin_uids,
    ));
    let policy_for_handler: Arc<dyn PrincipalPolicy> = principal_policy.clone();
    let dispatcher = Arc::new(DaemonDispatcher::new(pap, policy_for_handler));
    eprintln!("agent-sec-daemon: warning: PAP state is process-local and is lost on restart");

    let shutdown = ShutdownToken::new();
    let health_task = policy_runtime
        .as_ref()
        .map(|runtime| watch_policy_health(runtime.enqueuer()));
    let signal_task = tokio::spawn(signals.request_shutdown(shutdown.clone()));
    let result = serve(
        cli.bootstrap,
        dispatcher,
        Arc::new(JsonRejectionEncoder),
        shutdown,
    )
    .await;
    signal_task.abort();
    if let Some(health_task) = health_task {
        health_task.abort();
    }
    // The UDS service has stopped admission and drained requests. Retain the
    // blocking join task even on timeout; only process exit may cut off calls.
    let drain = tokio::task::spawn_blocking(move || {
        policy_runtime.map_or(Ok(()), ReconciliationRuntime::shutdown)
    });
    if !matches!(
        tokio::time::timeout(Duration::from_secs(30), drain).await,
        Ok(Ok(Ok(())))
    ) {
        eprintln!("asc-daemon: reconciliation drain failed or timed out");
        return ExitCode::FAILURE;
    }

    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(problem) => {
            report_error(&problem);
            ExitCode::FAILURE
        }
    }
}

fn report_error(problem: &dyn std::error::Error) {
    eprintln!("agent-sec-daemon: {problem}");
    let mut source = problem.source();
    while let Some(cause) = source {
        eprintln!("  caused by: {cause}");
        source = cause.source();
    }
}

fn watch_policy_health(
    queue: Arc<asc_policy_runtime::reconciliation::WorkQueue>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut healthy = true;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let current = queue.is_healthy();
            if current != healthy {
                eprintln!(
                    "asc-daemon: reconciliation health {}",
                    if current { "running" } else { "degraded" }
                );
                healthy = current;
            }
            if queue.has_failed() {
                break;
            }
        }
    })
}
