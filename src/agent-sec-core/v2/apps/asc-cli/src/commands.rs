//! Top-level command registration and request dispatch.

mod binding;
mod common;
mod policy;
mod scan_code;
mod scope;

use asc_daemon_protocol::DaemonRequest;
use clap::Subcommand;

use self::binding::BindingCommand;
use self::policy::PolicyCommand;
use self::scan_code::ScanCodeCommand;
use self::scope::ScopeCommand;
use crate::InputError;

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Manage authored Policy templates.
    #[command(subcommand)]
    Policy(PolicyCommand),
    /// Manage PID or cgroup Scope selectors.
    #[command(subcommand)]
    Scope(ScopeCommand),
    /// Manage Binding desired state; acceptance does not imply enforcement.
    #[command(subcommand)]
    Binding(BindingCommand),
    /// Scan code for security issues.
    ScanCode(ScanCodeCommand),
}

impl Command {
    pub(crate) fn request(&self) -> Result<DaemonRequest, InputError> {
        match self {
            Self::Policy(command) => command.request(),
            Self::Scope(command) => command.request(),
            Self::Binding(command) => command.request(),
            Self::ScanCode(command) => command.request(),
        }
    }

    pub(crate) const fn is_scan_code(&self) -> bool {
        matches!(self, Self::ScanCode(_))
    }
}
