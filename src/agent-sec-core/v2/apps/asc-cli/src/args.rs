use std::path::PathBuf;

use clap::{Parser, Subcommand};
use uuid::Uuid;

/// Rust daemon client for `AgentSecCore` management operations.
#[derive(Debug, Parser)]
#[command(name = "asc-cli", version, about)]
pub struct Cli {
    /// Path of the system daemon Unix socket.
    #[arg(long, value_name = "PATH")]
    pub(crate) socket: PathBuf,
    /// Path of the management credential file.
    #[arg(long, value_name = "PATH")]
    pub(crate) token_file: PathBuf,
    /// Per-read and per-write daemon I/O timeout in milliseconds.
    #[arg(
        long,
        default_value = "5000",
        value_parser = positive_u64,
        value_name = "MILLISECONDS"
    )]
    pub(crate) timeout_ms: u64,
    /// Operation to invoke through the daemon.
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Manage Policy Administration Point resources.
    Policy {
        /// Policy resource kind.
        #[command(subcommand)]
        resource: PolicyResource,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum PolicyResource {
    /// Immutable product Policy Templates.
    Template {
        /// Template operation.
        #[command(subcommand)]
        operation: TemplateOperation,
    },
    /// Immutable execution Scopes.
    Scope {
        /// Scope operation.
        #[command(subcommand)]
        operation: ScopeOperation,
    },
    /// Durable Policy Bindings.
    Binding {
        /// Binding operation.
        #[command(subcommand)]
        operation: BindingOperation,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum TemplateOperation {
    /// Create a new Policy or replace one existing Policy's complete desired state.
    Put {
        /// JSON file containing the exact daemon Policy Template put DTO.
        #[arg(long, value_name = "PATH")]
        file: PathBuf,
    },
    /// Fetch one immutable Policy Template revision.
    Get {
        /// Stable Policy Template identity.
        id: Uuid,
        /// Immutable Policy Template revision.
        #[arg(long, value_parser = positive_u64)]
        revision: u64,
    },
    /// List immutable Policy Template revisions.
    List {
        /// Maximum number of resources to return.
        #[arg(long, default_value = "100", value_parser = list_limit)]
        limit: u32,
        /// Number of resources to skip.
        #[arg(long, default_value = "0")]
        offset: u64,
    },
    /// Delete one exact Policy Template revision.
    Delete {
        /// Stable Policy UUID.
        id: Uuid,
        /// Exact immutable Policy Template revision to delete.
        #[arg(long, value_parser = positive_u64)]
        revision: u64,
    },
}

#[derive(Debug, Clone, Copy, Subcommand)]
pub(crate) enum ScopeOperation {
    /// Create a Scope or replace one existing Scope's selector intent.
    Put {
        /// Existing Scope UUID to update. Omit it to create.
        #[arg(long)]
        scope_id: Option<Uuid>,
        /// Select one process by caller-observed PID.
        #[arg(
            long,
            value_parser = positive_u32,
            required_unless_present = "cgroup_id",
            conflicts_with = "cgroup_id"
        )]
        pid: Option<u32>,
        /// Select one cgroup by caller-observed cgroup id.
        #[arg(
            long,
            value_parser = positive_u64,
            required_unless_present = "pid",
            conflicts_with = "pid"
        )]
        cgroup_id: Option<u64>,
    },
    /// Fetch one immutable resource revision.
    Get {
        /// Stable resource identity.
        id: Uuid,
        /// Immutable resource revision.
        #[arg(long, value_parser = positive_u64)]
        revision: u64,
    },
    /// List immutable Scope revisions.
    List {
        /// Maximum number of resources to return.
        #[arg(long, default_value = "100", value_parser = list_limit)]
        limit: u32,
        /// Number of resources to skip.
        #[arg(long, default_value = "0")]
        offset: u64,
    },
    /// Delete one exact Scope revision.
    Delete {
        /// Stable Scope identity.
        id: Uuid,
        /// Exact immutable Scope revision to delete.
        #[arg(long, value_parser = positive_u64)]
        revision: u64,
    },
}

#[derive(Debug, Clone, Copy, Subcommand)]
pub(crate) enum BindingOperation {
    /// Create a Binding or replace one existing Binding's references.
    Put {
        /// Existing Binding UUID to update. Omit it to create.
        #[arg(long)]
        binding_id: Option<Uuid>,
        /// Referenced Policy UUID.
        #[arg(long)]
        policy_id: Uuid,
        /// Referenced immutable Policy revision.
        #[arg(long, value_parser = positive_u64)]
        policy_revision: u64,
        /// Referenced Scope UUID.
        #[arg(long)]
        scope_id: Uuid,
        /// Referenced immutable Scope revision.
        #[arg(long, value_parser = positive_u64)]
        scope_revision: u64,
    },
    /// Fetch one resource by stable identity.
    Get {
        /// Stable resource identity.
        id: Uuid,
    },
    /// List current Binding snapshots.
    List {
        /// Maximum number of resources to return.
        #[arg(long, default_value = "100", value_parser = list_limit)]
        limit: u32,
        /// Number of resources to skip.
        #[arg(long, default_value = "0")]
        offset: u64,
    },
    /// Submit one immutable Binding removal revision.
    Delete {
        /// Stable Binding UUID.
        id: Uuid,
    },
}

fn positive_u64(value: &str) -> Result<u64, String> {
    let value = value
        .parse::<u64>()
        .map_err(|_| "must be a positive integer".to_owned())?;
    if value == 0 {
        return Err("must be greater than zero".to_owned());
    }
    Ok(value)
}

fn positive_u32(value: &str) -> Result<u32, String> {
    let value = value
        .parse::<u32>()
        .map_err(|_| "must be a positive integer".to_owned())?;
    if value == 0 {
        return Err("must be greater than zero".to_owned());
    }
    Ok(value)
}

fn list_limit(value: &str) -> Result<u32, String> {
    let value = value
        .parse::<u32>()
        .map_err(|_| "must be an integer from 1 through 1000".to_owned())?;
    if !(1..=1000).contains(&value) {
        return Err("must be from 1 through 1000".to_owned());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_must_be_positive() {
        let result = Cli::try_parse_from([
            "asc-cli",
            "--socket",
            "/run/daemon.sock",
            "--token-file",
            "/run/token",
            "policy",
            "template",
            "get",
            "6efed5ea-47c9-4b14-8e86-888f2ad88fc7",
            "--revision",
            "0",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn policy_delete_requires_both_id_and_revision() {
        let missing_revision = Cli::try_parse_from([
            "asc-cli",
            "--socket",
            "/run/daemon.sock",
            "--token-file",
            "/run/token",
            "policy",
            "template",
            "delete",
            "6efed5ea-47c9-4b14-8e86-888f2ad88fc7",
        ]);
        assert!(missing_revision.is_err());

        let missing_id = Cli::try_parse_from([
            "asc-cli",
            "--socket",
            "/run/daemon.sock",
            "--token-file",
            "/run/token",
            "policy",
            "template",
            "delete",
            "--revision",
            "1",
        ]);
        assert!(missing_id.is_err());
    }

    #[test]
    fn list_limit_is_bounded_by_the_daemon_contract() {
        for limit in ["0", "1001"] {
            let result = Cli::try_parse_from([
                "asc-cli",
                "--socket",
                "/run/daemon.sock",
                "--token-file",
                "/run/token",
                "policy",
                "template",
                "list",
                "--limit",
                limit,
            ]);
            assert!(result.is_err());
        }
    }
}
