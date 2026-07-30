//! Bounded synchronous client for the local enforcer socket.

use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agentsight_enforcement_protocol::{
    ApplyPolicy, Binding, Command, FrameReader, HealthStatus, ProtocolError, Request, Response,
    ResponseBody, ViolationEvent, read_frame, write_frame,
};
use thiserror::Error;
use uuid::Uuid;

/// Client-side failures for the enforcement trust boundary.
#[derive(Debug, Error)]
pub enum EnforcementError {
    /// Connecting to or configuring the UDS failed.
    #[error("enforcer I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// A bounded protocol frame was invalid.
    #[error("enforcer protocol failed: {0}")]
    Protocol(#[from] ProtocolError),
    /// The enforcer rejected a valid request.
    #[error("enforcer rejected request ({code}): {message}")]
    Remote {
        /// Stable machine-readable code.
        code: String,
        /// Sanitized remote detail.
        message: String,
    },
    /// A response did not correlate to its request.
    #[error("enforcer response id {actual} does not match request {expected}")]
    ResponseMismatch {
        /// Original request identifier.
        expected: Uuid,
        /// Response identifier supplied by the peer.
        actual: Uuid,
    },
    /// The response payload was valid but wrong for the operation.
    #[error("unexpected enforcer response: {0}")]
    UnexpectedResponse(String),
    /// A subscription connection closed between events.
    #[error("enforcer subscription disconnected")]
    Disconnected,
}

/// Local synchronous enforcement client.
#[derive(Clone)]
pub struct EnforcementClient {
    socket_path: PathBuf,
    timeout: Duration,
}

impl EnforcementClient {
    /// Creates a client with a five-second ordinary request timeout.
    pub fn new(socket_path: impl AsRef<Path>) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            timeout: Duration::from_secs(5),
        }
    }

    /// Overrides the ordinary request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Returns the configured socket path.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Queries backend readiness.
    ///
    /// # Errors
    ///
    /// Returns a transport, protocol, remote, or response-shape error.
    pub fn health(&self) -> Result<HealthStatus, EnforcementError> {
        match self.call(Command::Health)? {
            ResponseBody::Health(status) => Ok(status),
            body => Err(unexpected("health", &body)),
        }
    }

    /// Applies one desired binding.
    ///
    /// # Errors
    ///
    /// Returns a transport, protocol, remote, or response-shape error.
    pub fn apply(
        &self,
        request: ApplyPolicy,
        required_subscription_id: Uuid,
    ) -> Result<Binding, EnforcementError> {
        match self.call(Command::ApplyPolicyLeased {
            request,
            required_subscription_id,
        })? {
            ResponseBody::Applied(binding) => Ok(binding),
            body => Err(unexpected("apply", &body)),
        }
    }

    /// Detaches one binding after enforcer acknowledgement.
    ///
    /// # Errors
    ///
    /// Returns a transport, protocol, remote, or response-shape error.
    pub fn detach(&self, binding_id: Uuid) -> Result<(), EnforcementError> {
        match self.call(Command::DetachAgent { binding_id })? {
            ResponseBody::Detached => Ok(()),
            body => Err(unexpected("detach", &body)),
        }
    }

    /// Lists backend bindings.
    ///
    /// # Errors
    ///
    /// Returns a transport, protocol, remote, or response-shape error.
    pub fn bindings(&self) -> Result<Vec<Binding>, EnforcementError> {
        match self.call(Command::ListBindings)? {
            ResponseBody::Bindings(bindings) => Ok(bindings),
            body => Err(unexpected("list bindings", &body)),
        }
    }

    /// Opens an independent best-effort violation observer.
    ///
    /// # Errors
    ///
    /// Returns a transport, protocol, remote, or response-shape error.
    pub fn subscribe(&self) -> Result<ViolationSubscription, EnforcementError> {
        let subscription_id = Uuid::new_v4();
        self.open_subscription(
            Command::SubscribeViolations { subscription_id },
            subscription_id,
        )
    }

    /// Opens the required evidence stream used to authorize policy applies.
    ///
    /// # Errors
    ///
    /// Returns a transport, protocol, remote, or response-shape error.
    pub fn subscribe_required(&self) -> Result<ViolationSubscription, EnforcementError> {
        let subscription_id = Uuid::new_v4();
        self.open_subscription(
            Command::SubscribeRequiredViolations { subscription_id },
            subscription_id,
        )
    }

    fn open_subscription(
        &self,
        command: Command,
        subscription_id: Uuid,
    ) -> Result<ViolationSubscription, EnforcementError> {
        let request = Request::new(command);
        let mut stream = self.connect()?;
        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
        write_frame(&mut stream, &request)?;
        let mut reader = BufReader::new(stream);
        let response = read_required_response(&mut reader, request.request_id)?;
        match response {
            ResponseBody::Subscribed => Ok(ViolationSubscription {
                reader: FrameReader::new(reader),
                request_id: request.request_id,
                subscription_id,
            }),
            body => Err(unexpected("subscribe", &body)),
        }
    }

    fn call(&self, command: Command) -> Result<ResponseBody, EnforcementError> {
        let request = Request::new(command);
        let mut stream = self.connect()?;
        write_frame(&mut stream, &request)?;
        read_required_response(&mut BufReader::new(stream), request.request_id)
    }

    fn connect(&self) -> Result<UnixStream, EnforcementError> {
        let stream = UnixStream::connect(&self.socket_path)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        Ok(stream)
    }
}

/// One long-lived violation subscription.
pub struct ViolationSubscription {
    reader: FrameReader<BufReader<UnixStream>>,
    request_id: Uuid,
    subscription_id: Uuid,
}

impl ViolationSubscription {
    /// Returns this stream's generation identity.
    pub fn subscription_id(&self) -> Uuid {
        self.subscription_id
    }

    /// Waits briefly for the next violation.
    ///
    /// `Ok(None)` means the bounded read interval elapsed, allowing callers to
    /// observe shutdown state. EOF is returned as a disconnect error.
    ///
    /// # Errors
    ///
    /// Returns a protocol, remote, disconnect, or response-shape error.
    pub fn next_event(&mut self) -> Result<Option<ViolationEvent>, EnforcementError> {
        let response: Response = match self.reader.read_frame() {
            Ok(Some(response)) => response,
            Ok(None) => return Err(EnforcementError::Disconnected),
            Err(ProtocolError::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        let body = response_body(response, self.request_id)?;
        match body {
            ResponseBody::Violation(event) => Ok(Some(event)),
            body => Err(unexpected("violation subscription", &body)),
        }
    }
}

fn read_required_response(
    reader: &mut BufReader<UnixStream>,
    request_id: Uuid,
) -> Result<ResponseBody, EnforcementError> {
    let response: Response = read_frame(reader)?.ok_or(EnforcementError::Disconnected)?;
    response_body(response, request_id)
}

fn response_body(response: Response, request_id: Uuid) -> Result<ResponseBody, EnforcementError> {
    if response.request_id != request_id {
        return Err(EnforcementError::ResponseMismatch {
            expected: request_id,
            actual: response.request_id,
        });
    }
    response.result.map_err(|error| EnforcementError::Remote {
        code: error.code,
        message: error.message,
    })
}

fn unexpected(operation: &str, body: &ResponseBody) -> EnforcementError {
    EnforcementError::UnexpectedResponse(format!("{operation} returned {body:?}"))
}
