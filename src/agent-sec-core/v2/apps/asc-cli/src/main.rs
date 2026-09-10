use std::io;
use std::process::ExitCode;

use asc_cli::{Cli, output::render_policy};

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
        Err(error) => {
            eprintln!("agent-sec-cli: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<u8, Box<dyn std::error::Error>> {
    let request = cli.request()?;
    let response = asc_daemon_client::call(&cli.socket, &request, cli.timeout())?;
    render_policy(
        &response,
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    )
    .map_err(Into::into)
}
