//! Policy output policy; transport does not select presentation or process exit codes.

use std::io::{self, Write};

use asc_daemon_protocol::DaemonResponse;

/// Prints a Policy result to stdout or the complete daemon error to stderr.
///
/// # Errors
/// Returns output encoding or write failures, including a closed output pipe.
pub fn render_policy(
    response: &DaemonResponse,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> io::Result<u8> {
    match response {
        DaemonResponse::Success(success) => {
            serde_json::to_writer_pretty(&mut *stdout, &success.result)?;
            writeln!(stdout)?;
            Ok(0)
        }
        DaemonResponse::Error(error) => {
            serde_json::to_writer(&mut *stderr, error)?;
            writeln!(stderr)?;
            Ok(1)
        }
    }
}
