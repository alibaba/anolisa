use std::io;
use std::process::ExitCode;

use asc_cli::{
    Cli, InputError,
    output::{render_policy, render_scan_code},
};

fn main() -> ExitCode {
    let cli = match Cli::parse_from(std::env::args_os()) {
        Ok(cli) => cli,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            return if error.print().is_ok() {
                ExitCode::from(code)
            } else {
                ExitCode::FAILURE
            };
        }
    };
    match run(&cli) {
        Ok(code) => ExitCode::from(code),
        Err(error @ RunError::Input(InputError::EmptyCode)) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("agent-sec-cli: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<u8, RunError> {
    let request = cli.request().map_err(RunError::Input)?;
    let response =
        asc_daemon_client::call(&cli.socket, &request, cli.timeout()).map_err(RunError::Client)?;
    if cli.is_scan_code() {
        render_scan_code(
            &response,
            &mut io::stdout().lock(),
            &mut io::stderr().lock(),
        )
        .map_err(RunError::Output)
    } else {
        render_policy(
            &response,
            &mut io::stdout().lock(),
            &mut io::stderr().lock(),
        )
        .map_err(RunError::Output)
    }
}

#[derive(Debug, thiserror::Error)]
enum RunError {
    #[error(transparent)]
    Input(#[from] InputError),
    #[error(transparent)]
    Client(#[from] asc_daemon_client::ClientError),
    #[error(transparent)]
    Output(#[from] io::Error),
}
