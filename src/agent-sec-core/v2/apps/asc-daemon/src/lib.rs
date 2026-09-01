//! UDS adapter and composition helpers for the PAP daemon slice.

#![forbid(unsafe_code)]

mod auth;
mod handler;
mod state;
mod telemetry;
pub mod testing;
mod transport;
mod worker;

pub use asc_daemon_protocol::MAX_FRAME_BYTES;
pub use auth::{AuthFileError, PrepareAuthError, TokenVerifier, prepare_auth};
pub use state::AppState;
pub use transport::{BoundSocket, bind_socket, serve};
