//! On-demand environment health collectors: provider readiness,
//! configuration, hooks, PTY support, and permissions.
//!
//! Unlike the Linux host-resource collectors in `collectors.rs`, these checks
//! are cross-platform, synchronous, side-effect free, and infallible. Each
//! collector records facts, marks its check as done, and (when something needs
//! attention) attaches a `Warning` finding carrying a short remediation via
//! `detail_id`. One collector failing never blocks the others.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::CoshConfig;

use super::builder::HealthReportBuilder;
use super::model::{
    HealthFactCategory, HealthFactSource, HealthFactValue, HealthFinding, HealthFindingCategory,
    HealthMessageId, HealthSeverity,
};

/// Run every environment collector into `builder`. Collectors are independent;
/// each contributes facts/findings without depending on the others.
pub(crate) fn run_env_collectors(
    builder: &mut HealthReportBuilder,
    config: &CoshConfig,
    cwd: &Path,
    elapsed_ms: u128,
) {
    collect_provider(builder, config, elapsed_ms);
    collect_config(builder, config, elapsed_ms);
    collect_hooks(builder, config, cwd, elapsed_ms);
    collect_pty(builder, elapsed_ms);
    collect_permissions(builder, elapsed_ms);
}

fn env_finding(
    id: &str,
    title_id: HealthMessageId,
    detail_id: HealthMessageId,
    detail_args: BTreeMap<String, String>,
    evidence_fact_ids: Vec<String>,
) -> HealthFinding {
    HealthFinding {
        id: id.to_string(),
        severity: HealthSeverity::Warning,
        category: HealthFindingCategory::Observation,
        title_id,
        detail_id: Some(detail_id),
        detail_args,
        evidence_fact_ids,
        suggested_try_ids: Vec::new(),
    }
}

// ─── Provider readiness (static, no network) ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderReadiness {
    Ready,
    MissingCredentials,
    UnknownAdapter,
}

/// Pure classification: given the configured adapter and whether credentials
/// are present, decide readiness. No I/O.
pub(crate) fn classify_provider(adapter: &str, has_credentials: bool) -> ProviderReadiness {
    match adapter {
        "" => ProviderReadiness::UnknownAdapter,
        "fake" => ProviderReadiness::Ready,
        "cosh-core" | "claude" | "co" | "qwen" => {
            if has_credentials {
                ProviderReadiness::Ready
            } else {
                ProviderReadiness::MissingCredentials
            }
        }
        _ => {
            if has_credentials {
                ProviderReadiness::Ready
            } else {
                ProviderReadiness::UnknownAdapter
            }
        }
    }
}

fn env_non_empty(key: &str) -> bool {
    std::env::var(key)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn binary_on_path(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
}

fn provider_credentials_present(adapter: &str) -> bool {
    if adapter == "fake" {
        return true;
    }
    const CRED_ENVS: &[&str] = &[
        "DASHSCOPE_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "ALIBABA_CLOUD_ACCESS_KEY_ID",
        "ALIBABA_CLOUD_SECURITY_TOKEN",
        "ALIBABA_CLOUD_ECS_METADATA",
    ];
    if CRED_ENVS.iter().any(|key| env_non_empty(key)) {
        return true;
    }
    binary_on_path(adapter)
}

fn collect_provider(builder: &mut HealthReportBuilder, config: &CoshConfig, elapsed_ms: u128) {
    let adapter = config.adapter_default.trim();
    let has_credentials = provider_credentials_present(adapter);
    builder
        .add_fact(
            HealthFactCategory::Provider,
            "provider.adapter",
            HealthFactValue::String(adapter.to_string()),
            None,
            HealthFactSource::Derived,
            elapsed_ms,
        )
        .add_fact(
            HealthFactCategory::Provider,
            "provider.credentials_present",
            HealthFactValue::Bool(has_credentials),
            None,
            HealthFactSource::Derived,
            elapsed_ms,
        )
        .add_check_done("provider");

    if classify_provider(adapter, has_credentials) != ProviderReadiness::Ready {
        let mut args = BTreeMap::new();
        args.insert(
            "adapter".to_string(),
            if adapter.is_empty() {
                "unknown".to_string()
            } else {
                adapter.to_string()
            },
        );
        builder.add_finding(env_finding(
            "env-provider",
            HealthMessageId::HealthFindingProviderUnconfigured,
            HealthMessageId::HealthRemediationProvider,
            args,
            vec!["provider.adapter".to_string()],
        ));
    }
}

// ─── Configuration ────────────────────────────────────────────────────────

fn collect_config(builder: &mut HealthReportBuilder, config: &CoshConfig, elapsed_ms: u128) {
    let home_present = std::env::var_os("HOME")
        .map(|home| !home.is_empty())
        .unwrap_or(false);
    builder
        .add_fact(
            HealthFactCategory::Config,
            "config.home_present",
            HealthFactValue::Bool(home_present),
            None,
            HealthFactSource::Derived,
            elapsed_ms,
        )
        .add_fact(
            HealthFactCategory::Config,
            "config.language",
            HealthFactValue::String(config.language.clone()),
            None,
            HealthFactSource::Derived,
            elapsed_ms,
        )
        .add_fact(
            HealthFactCategory::Config,
            "config.adapter_default",
            HealthFactValue::String(config.adapter_default.clone()),
            None,
            HealthFactSource::Derived,
            elapsed_ms,
        )
        .add_fact(
            HealthFactCategory::Config,
            "config.approval_mode",
            HealthFactValue::String(config.approval_mode.clone()),
            None,
            HealthFactSource::Derived,
            elapsed_ms,
        )
        .add_check_done("config");

    if !home_present {
        builder.add_finding(env_finding(
            "env-config",
            HealthMessageId::HealthFindingConfigUnavailable,
            HealthMessageId::HealthRemediationConfig,
            BTreeMap::new(),
            vec!["config.home_present".to_string()],
        ));
    }
}

// ─── Hooks ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HooksReadiness {
    Ok,
    ProjectUntrusted,
}

/// Pure classification for hook readiness. No I/O.
pub(crate) fn classify_hooks(project_present: bool, project_trusted: bool) -> HooksReadiness {
    if project_present && !project_trusted {
        HooksReadiness::ProjectUntrusted
    } else {
        HooksReadiness::Ok
    }
}

fn collect_hooks(
    builder: &mut HealthReportBuilder,
    config: &CoshConfig,
    cwd: &Path,
    elapsed_ms: u128,
) {
    let user_dir_present = user_hooks_dir_present();
    let project_root = project_hook_root(cwd);
    let project_present = project_root.is_some();
    let project_trusted = match &project_root {
        Some(root) => is_trusted_root(root, &config.trusted_project_roots),
        None => true,
    };
    builder
        .add_fact(
            HealthFactCategory::Hooks,
            "hooks.user_dir_present",
            HealthFactValue::Bool(user_dir_present),
            None,
            HealthFactSource::Derived,
            elapsed_ms,
        )
        .add_fact(
            HealthFactCategory::Hooks,
            "hooks.project_present",
            HealthFactValue::Bool(project_present),
            None,
            HealthFactSource::Derived,
            elapsed_ms,
        )
        .add_fact(
            HealthFactCategory::Hooks,
            "hooks.project_trusted",
            HealthFactValue::Bool(project_trusted),
            None,
            HealthFactSource::Derived,
            elapsed_ms,
        )
        .add_check_done("hooks");

    if classify_hooks(project_present, project_trusted) == HooksReadiness::ProjectUntrusted {
        let mut args = BTreeMap::new();
        args.insert(
            "path".to_string(),
            project_root
                .as_ref()
                .map(|root| root.display().to_string())
                .unwrap_or_default(),
        );
        builder.add_finding(env_finding(
            "env-hooks",
            HealthMessageId::HealthFindingHooksUntrusted,
            HealthMessageId::HealthRemediationHooks,
            args,
            vec!["hooks.project_trusted".to_string()],
        ));
    }
}

/// Local reimplementations of the hook path probes so the diagnostics engine
/// stays independent of the binary-only `hooks` module facade.
fn user_hooks_dir_present() -> bool {
    std::env::var_os("HOME")
        .map(|home| {
            PathBuf::from(home)
                .join(".copilot-shell/cosh/hooks")
                .is_dir()
        })
        .unwrap_or(false)
}

fn project_hook_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|candidate| candidate.join(".cosh/hooks").is_dir())
        .map(canonical_root)
}

fn is_trusted_root(root: &Path, trusted_roots: &[PathBuf]) -> bool {
    let root = canonical_root(root);
    trusted_roots
        .iter()
        .any(|trusted| canonical_root(trusted) == root)
}

fn canonical_root(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

// ─── PTY support ──────────────────────────────────────────────────────────

fn pty_available() -> bool {
    // Static, side-effect-free probe: presence of the PTY multiplexer device.
    Path::new("/dev/ptmx").exists()
}

fn collect_pty(builder: &mut HealthReportBuilder, elapsed_ms: u128) {
    let available = pty_available();
    builder
        .add_fact(
            HealthFactCategory::Pty,
            "pty.ptmx_available",
            HealthFactValue::Bool(available),
            None,
            HealthFactSource::Derived,
            elapsed_ms,
        )
        .add_check_done("pty");

    if !available {
        builder.add_finding(env_finding(
            "env-pty",
            HealthMessageId::HealthFindingPtyUnavailable,
            HealthMessageId::HealthRemediationPty,
            BTreeMap::new(),
            vec!["pty.ptmx_available".to_string()],
        ));
    }
}

// ─── Permissions ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermissionsReadiness {
    Ok,
    Unwritable,
}

/// Pure classification for config-directory writability. No I/O.
pub(crate) fn classify_permissions(writable: bool) -> PermissionsReadiness {
    if writable {
        PermissionsReadiness::Ok
    } else {
        PermissionsReadiness::Unwritable
    }
}

fn config_state_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".copilot-shell"))
}

fn dir_writable(path: &Path) -> bool {
    match std::fs::metadata(path) {
        // Existing directory: writable unless the write bits are cleared.
        Ok(meta) => meta.is_dir() && !meta.permissions().readonly(),
        // Not created yet: cosh-shell will create it on demand -> treat as ok.
        Err(_) => true,
    }
}

fn collect_permissions(builder: &mut HealthReportBuilder, elapsed_ms: u128) {
    let dir = config_state_dir();
    let (path_str, writable) = match &dir {
        Some(path) => (path.display().to_string(), dir_writable(path)),
        None => (String::new(), false),
    };
    builder
        .add_fact(
            HealthFactCategory::Permissions,
            "permissions.state_dir",
            HealthFactValue::String(path_str.clone()),
            None,
            HealthFactSource::Derived,
            elapsed_ms,
        )
        .add_fact(
            HealthFactCategory::Permissions,
            "permissions.state_dir_writable",
            HealthFactValue::Bool(writable),
            None,
            HealthFactSource::Derived,
            elapsed_ms,
        )
        .add_check_done("permissions");

    if classify_permissions(writable) == PermissionsReadiness::Unwritable {
        let mut args = BTreeMap::new();
        args.insert("path".to_string(), path_str);
        builder.add_finding(env_finding(
            "env-permissions",
            HealthMessageId::HealthFindingPermissionsUnwritable,
            HealthMessageId::HealthRemediationPermissions,
            args,
            vec!["permissions.state_dir_writable".to_string()],
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_provider_covers_known_and_unknown_adapters() {
        assert_eq!(classify_provider("fake", false), ProviderReadiness::Ready);
        assert_eq!(
            classify_provider("cosh-core", true),
            ProviderReadiness::Ready
        );
        assert_eq!(
            classify_provider("cosh-core", false),
            ProviderReadiness::MissingCredentials
        );
        assert_eq!(classify_provider("qwen", true), ProviderReadiness::Ready);
        assert_eq!(
            classify_provider("", false),
            ProviderReadiness::UnknownAdapter
        );
        assert_eq!(
            classify_provider("mystery", false),
            ProviderReadiness::UnknownAdapter
        );
        assert_eq!(classify_provider("mystery", true), ProviderReadiness::Ready);
    }

    #[test]
    fn classify_hooks_flags_untrusted_project_only() {
        assert_eq!(classify_hooks(false, true), HooksReadiness::Ok);
        assert_eq!(classify_hooks(true, true), HooksReadiness::Ok);
        assert_eq!(
            classify_hooks(true, false),
            HooksReadiness::ProjectUntrusted
        );
        // No project hooks present -> trusted flag is irrelevant.
        assert_eq!(classify_hooks(false, false), HooksReadiness::Ok);
    }

    #[test]
    fn classify_permissions_flags_unwritable() {
        assert_eq!(classify_permissions(true), PermissionsReadiness::Ok);
        assert_eq!(
            classify_permissions(false),
            PermissionsReadiness::Unwritable
        );
    }

    #[test]
    fn collectors_record_checks_without_panicking() {
        let config = CoshConfig::default();
        let mut builder = HealthReportBuilder::for_started_at(0);
        run_env_collectors(&mut builder, &config, Path::new("/tmp"), 0);
        let report = builder.finish(1);
        for check in ["provider", "config", "hooks", "pty", "permissions"] {
            assert!(
                report.checks_done.iter().any(|done| done == check),
                "missing check {check}: {report:?}"
            );
        }
    }
}
