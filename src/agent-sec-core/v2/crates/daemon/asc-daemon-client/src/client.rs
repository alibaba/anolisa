use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use asc_daemon_protocol::{BearerAuth, DaemonRequest, DaemonResponse, MAX_FRAME_BYTES};
use opentelemetry::Context;
use serde::Serialize;

use crate::ClientError;
use crate::credential::ManagementCredential;
use crate::trace;

/// Authenticated synchronous client for one system daemon socket.
pub struct DaemonClient {
    socket: PathBuf,
    credential: ManagementCredential,
    io_timeout: Duration,
}

impl DaemonClient {
    /// Creates a client without starting a daemon or opening daemon-owned state.
    ///
    /// # Errors
    /// Returns a stable credential error when the token file is absent or invalid.
    pub fn from_token_file(
        socket: impl Into<PathBuf>,
        token_file: &Path,
        io_timeout: Duration,
    ) -> Result<Self, ClientError> {
        if io_timeout.is_zero() {
            return Err(ClientError::Timeout);
        }
        Ok(Self {
            socket: socket.into(),
            credential: ManagementCredential::load(token_file)?,
            io_timeout,
        })
    }

    /// Sends one authenticated method request and preserves all daemon response layers.
    ///
    /// # Errors
    /// Returns only pre-response credential, transport, timeout, framing, or decoding errors.
    pub fn call<P: Serialize>(
        &self,
        method: &str,
        params: &P,
        context: &Context,
    ) -> Result<DaemonResponse, ClientError> {
        let params = serde_json::to_value(params).map_err(|_| ClientError::RequestSerialization)?;
        let carrier = trace::inject(context);
        let request = DaemonRequest {
            method: method.to_owned(),
            params,
            auth: Some(BearerAuth {
                scheme: "bearer".to_owned(),
                token: self.credential.token().to_owned(),
            }),
            traceparent: carrier.traceparent,
            tracestate: carrier.tracestate,
        };
        let mut frame =
            serde_json::to_vec(&request).map_err(|_| ClientError::RequestSerialization)?;
        frame.push(b'\n');
        if frame.len() > MAX_FRAME_BYTES {
            return Err(ClientError::RequestTooLarge);
        }

        let mut stream =
            UnixStream::connect(&self.socket).map_err(|_| ClientError::DaemonUnavailable)?;
        stream
            .set_read_timeout(Some(self.io_timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.io_timeout)))
            .map_err(|error| map_io_error(&error))?;
        stream
            .write_all(&frame)
            .map_err(|error| map_io_error(&error))?;
        stream.flush().map_err(|error| map_io_error(&error))?;

        let mut response_frame = Vec::new();
        let reader = BufReader::new(&mut stream);
        reader
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut response_frame)
            .map_err(|error| map_io_error(&error))?;
        if response_frame.len() > MAX_FRAME_BYTES {
            return Err(ClientError::ResponseTooLarge);
        }
        if response_frame.last() != Some(&b'\n') {
            return Err(ClientError::Protocol);
        }
        response_frame.pop();
        serde_json::from_slice(&response_frame).map_err(|_| ClientError::Protocol)
    }
}

fn map_io_error(error: &std::io::Error) -> ClientError {
    if matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        ClientError::Timeout
    } else {
        ClientError::Transport
    }
}
