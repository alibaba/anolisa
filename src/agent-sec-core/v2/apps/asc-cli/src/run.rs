use std::io::Write;
use std::time::Duration;

use asc_daemon_client::DaemonClient;
use asc_daemon_protocol::DaemonResponse;
use opentelemetry::trace::{Span as _, TraceContextExt as _, Tracer as _, TracerProvider as _};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::trace::SdkTracerProvider;

use crate::args::Cli;
use crate::error::CliError;
use crate::request;

/// Executes one parsed CLI request through the configured daemon.
///
/// # Errors
/// Returns stable client, daemon, protocol, domain, or output failures.
pub fn execute(cli: Cli, output: &mut dyn Write) -> Result<(), CliError> {
    let socket = cli.socket.clone();
    let token_file = cli.token_file.clone();
    let timeout = Duration::from_millis(cli.timeout_ms);
    let request = request::from_cli(cli)?;
    let client = DaemonClient::from_token_file(socket, &token_file, timeout)?;

    let provider = SdkTracerProvider::builder().build();
    let tracer = provider.tracer("asc-cli");
    let mut span = tracer.start("asc-cli.daemon.request");
    span.set_attribute(KeyValue::new("rpc.method", request.method));
    let context = Context::current_with_span(span);
    let response = client.call(request.method, &request.params, &context);
    drop(context);
    let _ = provider.shutdown();

    render(response?, output)
}

fn render(response: DaemonResponse, output: &mut dyn Write) -> Result<(), CliError> {
    if !response.ok {
        let error = response.error.ok_or(CliError::Protocol)?;
        return Err(CliError::Daemon {
            code: error.code,
            message: error.message,
        });
    }
    if response.exit_code != 0 {
        let error = response
            .data
            .get("error")
            .and_then(serde_json::Value::as_object)
            .ok_or(CliError::Protocol)?;
        let code = error
            .get("code")
            .and_then(serde_json::Value::as_str)
            .ok_or(CliError::Protocol)?;
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .ok_or(CliError::Protocol)?;
        return Err(CliError::Rejected {
            code: code.to_owned(),
            message: message.to_owned(),
        });
    }
    serde_json::to_writer_pretty(&mut *output, &response.data).map_err(|_| CliError::Output)?;
    writeln!(output).map_err(|_| CliError::Output)
}
