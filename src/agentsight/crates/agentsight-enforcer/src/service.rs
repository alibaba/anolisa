//! Unix-domain socket service for the enforcement protocol.

use std::fs;
use std::io::BufReader;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use agentsight_enforcement_protocol::{
    Command, PROTOCOL_VERSION, ProtocolError, RemoteError, Request, Response, ResponseBody,
    read_frame, write_frame,
};
use thiserror::Error;

use crate::{BackendError, EnforcementBackend};

/// Failures while binding or serving the local protocol socket.
#[derive(Debug, Error)]
pub enum ServiceError {
    /// Socket or filesystem setup failed.
    #[error("enforcer service I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Local UDS server around one enforcement backend.
pub struct EnforcerService<B: EnforcementBackend> {
    listener: UnixListener,
    backend: Arc<B>,
    socket_path: PathBuf,
    allowed_gid: Option<u32>,
}

impl<B: EnforcementBackend> EnforcerService<B> {
    /// Binds a mode-0660 socket at `socket_path`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the socket cannot be bound, permissioned, or
    /// switched to non-blocking accept mode.
    pub fn bind(
        socket_path: impl AsRef<Path>,
        backend: Arc<B>,
        allowed_gid: Option<u32>,
    ) -> Result<Self, ServiceError> {
        let socket_path = socket_path.as_ref().to_path_buf();
        prepare_socket_path(&socket_path)?;
        let listener = UnixListener::bind(&socket_path)?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o660))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            backend,
            socket_path,
            allowed_gid,
        })
    }

    /// Accepts connections until `stop` is set.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when accepting a connection fails for a reason
    /// other than the non-blocking listener having no pending client.
    pub fn serve_until(self, stop: &AtomicBool) -> Result<(), ServiceError> {
        while !stop.load(Ordering::Acquire) {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let backend = Arc::clone(&self.backend);
                    let allowed_gid = self.allowed_gid;
                    thread::spawn(move || {
                        if let Err(error) = handle_connection(stream, backend, allowed_gid) {
                            eprintln!("agentsight-enforcer connection failed: {error}");
                        }
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(ServiceError::Io(error)),
            }
        }
        Ok(())
    }
}

fn prepare_socket_path(socket_path: &Path) -> Result<(), std::io::Error> {
    let metadata = match fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_socket() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!("refusing to replace non-socket path {socket_path:?}"),
        ));
    }
    match UnixStream::connect(socket_path) {
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!("enforcer is already listening at {socket_path:?}"),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            fs::remove_file(socket_path)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

impl<B: EnforcementBackend> Drop for EnforcerService<B> {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.socket_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("agentsight-enforcer could not remove socket: {error}");
        }
    }
}

fn handle_connection<B: EnforcementBackend>(
    mut stream: UnixStream,
    backend: Arc<B>,
    allowed_gid: Option<u32>,
) -> Result<(), ProtocolError> {
    if !peer_is_authorized(&stream, allowed_gid)? {
        return write_frame(
            &mut stream,
            &error_response(uuid::Uuid::nil(), "unauthorized", "peer is not authorized"),
        );
    }

    let request = match read_frame::<_, Request>(&mut BufReader::new(&stream)) {
        Ok(Some(request)) => request,
        Ok(None) => return Ok(()),
        Err(error) => {
            write_frame(
                &mut stream,
                &error_response(uuid::Uuid::nil(), "invalid_frame", &error.to_string()),
            )?;
            return Ok(());
        }
    };

    if matches!(request.command, Command::SubscribeSecurityEvents) {
        return write_frame(
            &mut stream,
            &error_response(
                request.request_id,
                "unsupported_command",
                "security event subscription is not available",
            ),
        );
    }

    if matches!(request.command, Command::SubscribeViolations) {
        let receiver = backend.subscribe();
        write_frame(
            &mut stream,
            &success_response(request.request_id, ResponseBody::Subscribed),
        )?;
        while let Ok(event) = receiver.recv() {
            write_frame(
                &mut stream,
                &success_response(request.request_id, ResponseBody::Violation(event)),
            )?;
        }
        return Ok(());
    }

    let result = dispatch(&*backend, request.command).map_err(remote_backend_error);
    write_frame(
        &mut stream,
        &Response {
            protocol_version: PROTOCOL_VERSION,
            request_id: request.request_id,
            result,
        },
    )
}

fn dispatch<B: EnforcementBackend>(
    backend: &B,
    command: Command,
) -> Result<ResponseBody, BackendError> {
    match command {
        Command::Health => backend.health().map(ResponseBody::Health),
        Command::ApplyPolicy(request) => backend.apply(request).map(ResponseBody::Applied),
        Command::DetachAgent { binding_id } => {
            backend.detach(binding_id)?;
            Ok(ResponseBody::Detached)
        }
        Command::ListBindings => backend.bindings().map(ResponseBody::Bindings),
        Command::SubscribeViolations => Ok(ResponseBody::Subscribed),
        Command::SubscribeSecurityEvents => Ok(ResponseBody::Subscribed),
    }
}

fn success_response(request_id: uuid::Uuid, body: ResponseBody) -> Response {
    Response {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        result: Ok(body),
    }
}

fn error_response(request_id: uuid::Uuid, code: &str, message: &str) -> Response {
    Response {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        result: Err(RemoteError {
            code: code.into(),
            message: message.into(),
        }),
    }
}

fn remote_backend_error(error: BackendError) -> RemoteError {
    let code = match &error {
        BackendError::BindingConflict(_) => "binding_conflict",
        BackendError::MissingBinding(_) => "missing_binding",
        BackendError::StaleProcess { .. } => "stale_process",
        BackendError::CompileFailure(_) => "compile_failure",
        BackendError::KernelFailure(_) => "kernel_failure",
    };
    RemoteError {
        code: code.into(),
        message: error.to_string(),
    }
}

#[cfg(target_os = "linux")]
fn peer_is_authorized(
    stream: &UnixStream,
    allowed_gid: Option<u32>,
) -> Result<bool, ProtocolError> {
    use std::mem::{size_of, zeroed};
    use std::os::fd::AsRawFd;

    // SAFETY: `ucred` is a plain C struct initialized before the kernel writes
    // exactly `size_of::<ucred>()` bytes through getsockopt.
    let mut credentials: libc::ucred = unsafe { zeroed() };
    let mut length = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the socket FD is valid for this call and both pointers reference
    // writable storage whose size is provided in `length`.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: `geteuid` has no preconditions and does not dereference pointers.
    let service_uid = unsafe { libc::geteuid() };
    Ok(credentials.uid == 0
        || credentials.uid == service_uid
        || allowed_gid.is_some_and(|gid| credentials.gid == gid))
}

#[cfg(not(target_os = "linux"))]
fn peer_is_authorized(
    _stream: &UnixStream,
    _allowed_gid: Option<u32>,
) -> Result<bool, ProtocolError> {
    Ok(true)
}
