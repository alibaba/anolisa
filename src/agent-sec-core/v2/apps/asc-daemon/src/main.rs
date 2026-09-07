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
            eprintln!("asc-daemon: {problem}");
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
            eprintln!("asc-daemon: {problem}");
            return ExitCode::FAILURE;
        }
    };
    let repository = Arc::new(ProcessLocalPapRepository::default());
    let pap = PapService::new(repository, Arc::new(PolicyTemplateCompiler));
    let principal_policy = Arc::new(RootManagedPrincipalPolicy::default());
    let policy_for_handler: Arc<dyn PrincipalPolicy> = principal_policy.clone();
    let dispatcher = Arc::new(DaemonDispatcher::new(pap, policy_for_handler));
    eprintln!("asc-daemon: warning: PAP state is process-local and is lost on restart");

    let shutdown = ShutdownToken::new();
    let signal_task = tokio::spawn(signals.request_shutdown(shutdown.clone()));
    let result = serve(
        cli.bootstrap,
        dispatcher,
        Arc::new(JsonRejectionEncoder),
        shutdown,
    )
    .await;
    signal_task.abort();

    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(problem) => {
            report_error(&problem);
            ExitCode::FAILURE
        }
    }
}

fn report_error(problem: &dyn std::error::Error) {
    eprintln!("asc-daemon: {problem}");
    let mut source = problem.source();
    while let Some(cause) = source {
        eprintln!("  caused by: {cause}");
        source = cause.source();
    }
}
