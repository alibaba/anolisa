//! Command parsing and input preparation, separate from transport and rendering.

mod commands;
pub mod output;

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;
use std::time::Duration;

use asc_daemon_protocol::DaemonRequest;
use asc_foundation_types::{DAEMON_SOCKET_ENV, daemon_socket_path_from_env};
use clap::Parser;
use commands::Command;

/// Parsed invocation for one daemon-backed CLI command.
#[derive(Debug)]
pub struct Cli {
    /// Absolute endpoint of an already-running daemon.
    pub socket: PathBuf,
    timeout_ms: u32,
    command: Command,
}

#[derive(Debug, Parser)]
#[command(
    name = "agent-sec-cli",
    version,
    about = "Manage Policy, Scope and Binding through asc-daemon"
)]
struct Arguments {
    /// Absolute endpoint of an already-running daemon.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Total connect/write/read deadline in milliseconds; requests are never retried.
    #[arg(long, global = true, default_value_t = 5000, value_parser = clap::value_parser!(u32).range(1..))]
    timeout_ms: u32,
    #[command(subcommand)]
    command: Command,
}

impl Cli {
    /// Parses argv including its executable name, preserving OS-native file paths.
    ///
    /// # Errors
    /// Returns clap help/version outcomes or usage errors; never connects to a daemon.
    pub fn parse_from<I, T>(arguments: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let socket_env = std::env::var_os(DAEMON_SOCKET_ENV);
        Self::parse_from_with_socket_env(arguments, socket_env.as_deref())
    }

    fn parse_from_with_socket_env<I, T>(
        arguments: I,
        socket_env: Option<&OsStr>,
    ) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let argv: Vec<OsString> = arguments.into_iter().map(Into::into).collect();
        let arguments = Arguments::try_parse_from(&argv)?;
        // Clap propagates global values across subcommands using last-wins.
        // After successful parsing, a standalone --option token cannot be a
        // value: these commands do not accept hyphen values or positional tails.
        for option in ["--socket", "--timeout-ms"] {
            let count = argv
                .iter()
                .skip(1)
                .filter(|argument| {
                    argument.as_bytes().split(|byte| *byte == b'=').next()
                        == Some(option.as_bytes())
                })
                .count();
            if count > 1 {
                return Err(clap::Error::raw(
                    clap::error::ErrorKind::ArgumentConflict,
                    format!("{option} may be specified only once"),
                ));
            }
        }
        let socket = match arguments.socket {
            Some(socket) => socket,
            None => daemon_socket_path_from_env(socket_env).map_err(|error| {
                clap::Error::raw(clap::error::ErrorKind::ValueValidation, error.to_string())
            })?,
        };
        if !socket.is_absolute() {
            return Err(clap::Error::raw(
                clap::error::ErrorKind::ValueValidation,
                "--socket must be an absolute path",
            ));
        }
        Ok(Self {
            socket,
            timeout_ms: arguments.timeout_ms,
            command: arguments.command,
        })
    }

    /// Returns the single call deadline duration.
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(u64::from(self.timeout_ms))
    }

    /// Delegates to the selected command to construct a typed daemon request.
    ///
    /// # Errors
    /// Returns a file read, template decode, or request encoding error.
    pub fn request(&self) -> Result<DaemonRequest, InputError> {
        self.command.request()
    }

    /// Whether this invocation uses the V1-compatible scan-code projection.
    pub const fn is_scan_code(&self) -> bool {
        self.command.is_scan_code()
    }
}

/// Local input failures, reported as execution failures rather than daemon errors.
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    /// The V1-compatible scan-code command received no non-whitespace source.
    #[error("Error: --code is required (use --code '<source>')")]
    EmptyCode,
    /// Template file access failed.
    #[error("cannot read Policy template: {0}")]
    Read(#[from] std::io::Error),
    /// Bound input before parsing or constructing a request.
    #[error("Policy template exceeds the 4194304-byte input limit")]
    TooLarge,
    /// Invalid authoring JSON or request serialization.
    #[error("invalid Policy request input: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_socket_is_used_when_the_option_is_omitted() {
        let cli = Cli::parse_from_with_socket_env(
            ["agent-sec-cli", "scan-code", "--code", "echo hello"],
            Some(OsStr::new("/run/agent-sec-core/daemon.sock")),
        )
        .expect("deployment endpoint parses");
        assert_eq!(cli.socket, PathBuf::from("/run/agent-sec-core/daemon.sock"));
    }

    #[test]
    fn explicit_socket_overrides_the_environment_socket() {
        let cli = Cli::parse_from_with_socket_env(
            [
                "agent-sec-cli",
                "--socket",
                "/run/explicit.sock",
                "scan-code",
                "--code",
                "echo hello",
            ],
            Some(OsStr::new("/run/agent-sec-core/daemon.sock")),
        )
        .expect("explicit endpoint parses");
        assert_eq!(cli.socket, PathBuf::from("/run/explicit.sock"));
    }

    #[test]
    fn missing_or_relative_environment_socket_is_a_usage_error() {
        for socket_env in [None, Some(OsStr::new("relative"))] {
            let error = Cli::parse_from_with_socket_env(
                ["agent-sec-cli", "scan-code", "--code", "echo hello"],
                socket_env,
            )
            .expect_err("invalid deployment endpoint must fail before connecting");
            assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        }
    }
}
