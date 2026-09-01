use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use asc_daemon_protocol::{DaemonRequest, DaemonResponse, MAX_FRAME_BYTES};
use asc_policy_runtime::PolicyAdapter;
use tracing::{info, warn};
use uuid::Uuid;

use crate::handler;
use crate::state::AppState;
use crate::telemetry;

const SERIALIZATION_FAILURE_RESPONSE: &[u8] = br#"{"request_id":"unknown","ok":false,"data":{},"stdout":"","stderr":"daemon response serialization failed","exit_code":1,"error":{"code":"internal_error","message":"daemon response serialization failed"}}"#;
const CONNECTION_IO_TIMEOUT: Duration = Duration::from_secs(5);
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One listener together with ownership evidence for its filesystem socket inode.
pub struct BoundSocket {
    listener: UnixListener,
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl Drop for BoundSocket {
    fn drop(&mut self) {
        let Ok(metadata) = fs::symlink_metadata(&self.path) else {
            return;
        };
        if metadata.file_type().is_socket()
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
            && let Err(problem) = fs::remove_file(&self.path)
        {
            warn!(
                path = %self.path.display(),
                error = %problem,
                "failed to remove owned daemon socket"
            );
        }
    }
}

/// Binds a new UDS without deleting or replacing any pre-existing path.
///
/// # Errors
/// Returns an I/O error for an unsafe or already-used path.
pub fn bind_socket(path: &Path) -> Result<BoundSocket, std::io::Error> {
    let path = absolute_path(path)?;
    let listener = UnixListener::bind(&path)?;
    let metadata = fs::symlink_metadata(&path)?;
    let socket = BoundSocket {
        listener,
        path,
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o660))?;
    Ok(socket)
}

/// Serves connections until the listener fails.
///
/// # Errors
/// Individual accept and connection failures are logged and isolated from the daemon lifecycle.
pub fn serve<A>(
    socket: &BoundSocket,
    state: &AppState<A>,
    shutdown: &AtomicBool,
) -> Result<(), std::io::Error>
where
    A: PolicyAdapter + 'static,
{
    socket.listener.set_nonblocking(true)?;
    while !shutdown.load(Ordering::Relaxed) {
        match socket.listener.accept() {
            Ok((mut stream, _)) => {
                if let Err(problem) = handle_connection(&mut stream, state) {
                    warn!(error = %problem, "connection handling failed; continuing");
                }
            }
            Err(problem) if problem.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(problem) if problem.kind() == std::io::ErrorKind::Interrupted => {}
            Err(problem) => {
                warn!(error = %problem, "socket accept failed; retrying");
                thread::sleep(ACCEPT_ERROR_BACKOFF);
            }
        }
    }
    Ok(())
}

/// Serves a bounded number of connections for deterministic integration tests.
///
/// # Errors
/// Returns accept or response I/O errors.
pub(crate) fn serve_n<A>(
    socket: &BoundSocket,
    state: &AppState<A>,
    maximum: usize,
) -> Result<(), std::io::Error>
where
    A: PolicyAdapter + 'static,
{
    for _ in 0..maximum {
        let (mut stream, _) = socket.listener.accept()?;
        if let Err(problem) = handle_connection(&mut stream, state) {
            warn!(error = %problem, "connection handling failed; continuing");
        }
    }
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf, std::io::Error> {
    if path.is_absolute() {
        return Ok(path.to_owned());
    }
    Ok(std::env::current_dir()?.join(path))
}

fn handle_connection<A>(stream: &mut UnixStream, state: &AppState<A>) -> Result<(), std::io::Error>
where
    A: PolicyAdapter + 'static,
{
    // TODO: move accepted streams to a bounded worker pool when the daemon concurrency model is
    // introduced. Deadlines bound head-of-line blocking in this single-threaded foundation slice.
    stream.set_read_timeout(Some(CONNECTION_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(CONNECTION_IO_TIMEOUT))?;
    let mut frame = Vec::new();
    {
        let mut reader = BufReader::new(&mut *stream);
        reader
            .by_ref()
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut frame)?;
    }
    if frame.len() > MAX_FRAME_BYTES {
        return write_response(
            stream,
            &DaemonResponse::daemon_error(
                Uuid::new_v4().to_string(),
                "payload_too_large",
                "request payload exceeds 4 MiB",
            ),
        );
    }
    if frame.last() == Some(&b'\n') {
        frame.pop();
    }
    let request_id = Uuid::new_v4().to_string();
    let request: DaemonRequest = match serde_json::from_slice(&frame) {
        Ok(request) => request,
        Err(_) => {
            return write_response(
                stream,
                &DaemonResponse::daemon_error(
                    request_id,
                    "bad_request",
                    "request must match the daemon envelope",
                ),
            );
        }
    };
    let span = telemetry::request_span(&request, &request_id);
    let _guard = span.enter();
    let response = handler::dispatch(request_id, request, state);
    info!(
        response_ok = response.ok,
        exit_code = response.exit_code,
        "daemon request completed"
    );
    write_response(stream, &response)
}

fn write_response(
    stream: &mut UnixStream,
    response: &DaemonResponse,
) -> Result<(), std::io::Error> {
    let mut writer = BoundedFrameWriter::new(MAX_FRAME_BYTES - 1);
    let serialized = serde_json::to_writer(&mut writer, response);
    let mut bytes = if serialized.is_ok() {
        writer.into_bytes()
    } else if writer.exceeded() {
        serde_json::to_vec(&DaemonResponse::daemon_error(
            response.request_id.clone(),
            "payload_too_large",
            "response payload exceeds 4 MiB",
        ))
        .unwrap_or_else(|_| SERIALIZATION_FAILURE_RESPONSE.to_vec())
    } else {
        SERIALIZATION_FAILURE_RESPONSE.to_vec()
    };
    bytes.push(b'\n');
    stream.write_all(&bytes)?;
    stream.flush()
}

struct BoundedFrameWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl BoundedFrameWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            exceeded: false,
        }
    }

    fn exceeded(&self) -> bool {
        self.exceeded
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for BoundedFrameWriter {
    fn write(&mut self, buffer: &[u8]) -> Result<usize, std::io::Error> {
        let Some(next_len) = self.bytes.len().checked_add(buffer.len()) else {
            self.exceeded = true;
            return Err(std::io::Error::other("response frame limit exceeded"));
        };
        if next_len > self.limit {
            self.exceeded = true;
            return Err(std::io::Error::other("response frame limit exceeded"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> Result<(), std::io::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use asc_daemon_core::PolicyService;
    use asc_persistence_sqlite::SqlitePolicyStore;
    use asc_policy_runtime::testing::FakePolicyAdapter;

    use super::*;
    use crate::{AppState, TokenVerifier, prepare_auth};

    fn unique_directory(suffix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("asc-daemon-{suffix}-{}", Uuid::new_v4()))
    }

    #[test]
    fn owned_socket_is_removed_when_the_guard_drops() {
        let directory = unique_directory("socket-drop");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("daemon.sock");
        let socket = bind_socket(&path).unwrap();
        assert!(path.exists());

        drop(socket);

        assert!(!path.exists());
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn socket_guard_never_removes_a_replacement_inode() {
        let directory = unique_directory("socket-replacement");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("daemon.sock");
        let moved_path = directory.join("owned.sock");
        let socket = bind_socket(&path).unwrap();
        fs::rename(&path, &moved_path).unwrap();
        let replacement = UnixListener::bind(&path).unwrap();

        drop(socket);

        assert!(path.exists());
        drop(replacement);
        fs::remove_file(path).unwrap();
        fs::remove_file(moved_path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn shutdown_flag_stops_accept_and_removes_the_owned_socket() {
        let directory = unique_directory("socket-shutdown");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("daemon.sock");
        let token_path = directory.join("policy-admin.token");
        prepare_auth(&token_path).unwrap();
        let auth = Arc::new(TokenVerifier::load(&token_path).unwrap());
        let policy = Arc::new(PolicyService::new(
            Arc::new(SqlitePolicyStore::memory().unwrap()),
            Arc::new(FakePolicyAdapter::default()),
        ));
        let state = AppState::new(policy, auth);
        let socket = bind_socket(&path).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let server_shutdown = Arc::clone(&shutdown);
        let server = thread::spawn(move || {
            serve(&socket, &state, &server_shutdown).unwrap();
        });

        shutdown.store(true, Ordering::Relaxed);
        server.join().unwrap();

        assert!(!path.exists());
        fs::remove_file(token_path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn response_serialization_stops_at_the_frame_bound() {
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let response = DaemonResponse::success(
            Uuid::new_v4().to_string(),
            serde_json::json!({"large": "x".repeat(MAX_FRAME_BYTES)}),
        );

        write_response(&mut server, &response).unwrap();
        drop(server);
        let mut frame = Vec::new();
        client.read_to_end(&mut frame).unwrap();

        assert!(frame.len() <= MAX_FRAME_BYTES);
        let decoded: DaemonResponse = serde_json::from_slice(&frame).unwrap();
        assert_eq!(decoded.error.unwrap().code, "payload_too_large");
    }
}
