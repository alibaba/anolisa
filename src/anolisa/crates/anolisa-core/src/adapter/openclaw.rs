//! OpenClaw framework driver.
//!
//! OpenClaw loads plugins from a CLI-managed registry, not from a dropped
//! directory, so `enable` runs `openclaw plugins install <resource_root>`
//! and `disable` runs `openclaw plugins uninstall <plugin_id>`. Status is
//! the read-only `openclaw plugins list`. All three go through the
//! Manager's [`run_framework_cli`](super::driver::AdapterOps::run_framework_cli)
//! helper (timeout, output
//! truncation, central log) — the driver only builds argv arrays from
//! validated data.
//!
//! The CLI env contract mirrors `openclaw/scripts/install.sh`: unset
//! `OPENCLAW_HOME`, set `OPENCLAW_STATE_DIR` to the resolved home, and
//! prepend the standard bin dirs to `PATH`. `OPENCLAW_BIN` overrides the
//! executable (used by tests to point at a fake CLI).

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::AdapterError;
use super::claim::{
    AdapterClaim, CLAIM_SCHEMA_VERSION, ClaimResource, ClaimResourceKind, ClaimStatus,
    DRIVER_SCHEMA_VERSION, DriverPayload, OpenClawClaim, validate_plugin_id,
};
use super::driver::{
    AdapterBundle, AdapterCondition, AdapterConditionKind, AdapterStatusReport, AdapterSummary,
    ClaimResourceRef, ConditionStatus, DetectResult, DisableReport, DriverCtx, DriverPlan,
    FrameworkCommand, FrameworkDriver, HostEnv, find_binary_in_path,
};

/// Default timeout for an OpenClaw CLI invocation.
const CLI_TIMEOUT: Duration = Duration::from_secs(60);

/// Resource ids used in OpenClaw receipts. Stable strings referenced from
/// the [`OpenClawClaim`] payload and condition reports.
const RES_STATE_DIR: &str = "openclaw_state_dir";
const RES_PLUGIN: &str = "openclaw_plugin";

/// OpenClaw driver. Stateless; all per-operation context arrives via
/// [`DriverCtx`].
pub struct OpenClawDriver;

impl OpenClawDriver {
    /// Construct the driver.
    pub fn new() -> Self {
        Self
    }
}

impl Default for OpenClawDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkDriver for OpenClawDriver {
    fn name(&self) -> &'static str {
        "openclaw"
    }

    fn detect(&self, env: &HostEnv) -> DetectResult {
        match find_binary_in_path(&openclaw_bin()) {
            Some(path) => DetectResult {
                detected: true,
                reason: format!("openclaw CLI found at {}", path.display()),
            },
            None => {
                // The CLI is what enable/disable need; a bare home dir is
                // not sufficient. Report not-detected but mention the home
                // so a user understands the framework is partially present.
                let home_note = openclaw_home(env.user_home.as_deref())
                    .filter(|h| h.exists())
                    .map(|h| format!(" (home {} exists but CLI is not on PATH)", h.display()))
                    .unwrap_or_default();
                DetectResult {
                    detected: false,
                    reason: format!("openclaw CLI not found on PATH{home_note}"),
                }
            }
        }
    }

    fn allowed_external_roots(&self, ctx: &DriverCtx) -> Vec<PathBuf> {
        // The only external root OpenClaw writes is its own home/state dir.
        openclaw_home(ctx.user_home.as_deref())
            .into_iter()
            .collect()
    }

    fn read_bundle(&self, ctx: &DriverCtx) -> Result<AdapterBundle, AdapterError> {
        let root = &ctx.resource_root;
        if !root.is_dir() {
            return Err(AdapterError::BundleInvalid {
                root: root.clone(),
                reason: "resource root does not exist or is not a directory".to_string(),
            });
        }
        let is_empty = root
            .read_dir()
            .map_err(|source| AdapterError::Io {
                path: root.clone(),
                source,
            })?
            .next()
            .is_none();
        if is_empty {
            return Err(AdapterError::BundleInvalid {
                root: root.clone(),
                reason: "resource root is empty".to_string(),
            });
        }

        let plugin_id = ctx
            .declared_plugin_id
            .clone()
            .or(read_plugin_manifest_id(root)?)
            .or_else(|| Some(ctx.component.clone()));

        Ok(AdapterBundle {
            resource_root: root.clone(),
            digest: digest_tree(root),
            plugin_id,
        })
    }

    fn plan_enable(
        &self,
        bundle: &AdapterBundle,
        ctx: &DriverCtx,
    ) -> Result<DriverPlan, AdapterError> {
        let plugin_id = require_plugin_id(bundle)?;
        validate_plugin_id(&plugin_id)?;
        let home = require_home(ctx)?;
        let cmd = build_install_cmd(&bundle.resource_root, &home, ctx.user_home.as_deref());
        Ok(DriverPlan {
            framework: self.name().to_string(),
            component: ctx.component.clone(),
            actions: vec![format!(
                "register openclaw plugin '{plugin_id}' from {}",
                bundle.resource_root.display()
            )],
            register_command: Some(display_command(&cmd)),
        })
    }

    fn prepare_enable(
        &self,
        bundle: &AdapterBundle,
        ctx: &DriverCtx,
    ) -> Result<AdapterClaim, AdapterError> {
        let plugin_id = require_plugin_id(bundle)?;
        validate_plugin_id(&plugin_id)?;
        let home = require_home(ctx)?;

        let resources = vec![
            ClaimResource {
                id: RES_STATE_DIR.to_string(),
                purpose: "openclaw_state_dir".to_string(),
                kind: ClaimResourceKind::ExternalPath { path: home.clone() },
            },
            ClaimResource {
                id: RES_PLUGIN.to_string(),
                purpose: "openclaw_plugin".to_string(),
                kind: ClaimResourceKind::FrameworkPlugin {
                    framework: self.name().to_string(),
                    plugin_id: plugin_id.clone(),
                },
            },
        ];

        Ok(AdapterClaim {
            claim_schema: CLAIM_SCHEMA_VERSION,
            component: ctx.component.clone(),
            framework: self.name().to_string(),
            plugin_id: Some(plugin_id),
            enabled_at: now_iso8601(),
            resource_root: bundle.resource_root.clone(),
            bundle_digest: bundle.digest.clone(),
            driver_schema: DRIVER_SCHEMA_VERSION,
            status: ClaimStatus::Enabled,
            resources,
            driver_payload: DriverPayload::OpenClaw(OpenClawClaim {
                state_dir_resource: RES_STATE_DIR.to_string(),
                plugin_resource: RES_PLUGIN.to_string(),
            }),
        })
    }

    fn apply_enable(&self, claim: &AdapterClaim, ctx: &DriverCtx) -> Result<(), AdapterError> {
        let plugin_id = claim_plugin_id(claim).ok_or_else(|| AdapterError::BundleInvalid {
            root: claim.resource_root.clone(),
            reason: "openclaw receipt has no plugin id".to_string(),
        })?;
        validate_plugin_id(&plugin_id)?;
        let home = require_home(ctx)?;

        let cmd = build_install_cmd(&claim.resource_root, &home, ctx.user_home.as_deref());
        let program = cmd.program.clone();
        let output = ctx.ops.run_framework_cli(cmd)?;
        if output.success() {
            Ok(())
        } else {
            Err(AdapterError::FrameworkCli {
                program,
                reason: cli_failure_reason("plugins install", &output),
            })
        }
    }

    fn status(
        &self,
        claim: &AdapterClaim,
        ctx: &DriverCtx,
    ) -> Result<AdapterStatusReport, AdapterError> {
        let mut conditions = Vec::new();

        // 1. Framework detectable?
        let detect = self.detect(&HostEnv {
            user_home: ctx.user_home.clone(),
        });
        conditions.push(AdapterCondition {
            kind: AdapterConditionKind::FrameworkDetected,
            status: bool_status(detect.detected),
            reason: Some(detect.reason.clone()),
            resource: None,
        });

        // 2. Resource bundle still matches the enable-time digest?
        conditions.push(self.bundle_match_condition(claim));

        // 3. Plugin still registered? Requires the CLI for a read-only
        //    `plugins list`.
        let plugin_id = claim_plugin_id(claim);
        let (plugin_cond, verify_cond, plugin_registered) = if !detect.detected {
            (
                AdapterCondition {
                    kind: AdapterConditionKind::PluginRegistered,
                    status: ConditionStatus::Unknown,
                    reason: Some("framework not detected; cannot verify".to_string()),
                    resource: plugin_id.as_ref().map(|_| ClaimResourceRef {
                        id: RES_PLUGIN.to_string(),
                    }),
                },
                AdapterCondition {
                    kind: AdapterConditionKind::VerificationSupported,
                    status: ConditionStatus::False,
                    reason: Some("openclaw CLI unavailable".to_string()),
                    resource: None,
                },
                ConditionStatus::Unknown,
            )
        } else if let Some(pid) = &plugin_id {
            self.plugin_registered_condition(pid, ctx)
        } else {
            (
                AdapterCondition {
                    kind: AdapterConditionKind::PluginRegistered,
                    status: ConditionStatus::Unknown,
                    reason: Some("receipt has no plugin id".to_string()),
                    resource: None,
                },
                AdapterCondition {
                    kind: AdapterConditionKind::VerificationSupported,
                    status: ConditionStatus::True,
                    reason: None,
                    resource: None,
                },
                ConditionStatus::Unknown,
            )
        };
        conditions.push(plugin_cond);
        conditions.push(verify_cond);

        let summary = summarize(claim.status, detect.detected, plugin_registered);
        Ok(AdapterStatusReport {
            summary,
            conditions,
        })
    }

    fn disable(
        &self,
        claim: &AdapterClaim,
        ctx: &DriverCtx,
    ) -> Result<DisableReport, AdapterError> {
        let plugin_id = match claim_plugin_id(claim) {
            Some(p) => p,
            None => {
                // No plugin recorded: nothing to unregister.
                return Ok(DisableReport {
                    cleanup_complete: true,
                    messages: vec!["receipt records no plugin to unregister".to_string()],
                });
            }
        };
        validate_plugin_id(&plugin_id)?;

        if find_binary_in_path(&openclaw_bin()).is_none() {
            return Ok(DisableReport {
                cleanup_complete: false,
                messages: vec![
                    "openclaw CLI not found on PATH; receipt kept so cleanup can be retried"
                        .to_string(),
                ],
            });
        }

        let home = require_home(ctx)?;
        let cmd = build_uninstall_cmd(&plugin_id, &home, ctx.user_home.as_deref());
        let output = ctx.ops.run_framework_cli(cmd)?;
        if output.success() {
            Ok(DisableReport {
                cleanup_complete: true,
                messages: vec![format!("unregistered openclaw plugin '{plugin_id}'")],
            })
        } else {
            // Keep the receipt (Manager marks cleanup_failed) so disable can
            // be retried — do NOT delete anything else.
            Ok(DisableReport {
                cleanup_complete: false,
                messages: vec![format!(
                    "openclaw plugin uninstall failed: {}",
                    cli_failure_reason("plugins uninstall", &output)
                )],
            })
        }
    }
}

impl OpenClawDriver {
    /// Build the `ResourceBundleMatches` condition by re-digesting the
    /// resource root and comparing to the enable-time digest.
    fn bundle_match_condition(&self, claim: &AdapterClaim) -> AdapterCondition {
        let kind = AdapterConditionKind::ResourceBundleMatches;
        match (&claim.bundle_digest, digest_tree(&claim.resource_root)) {
            (Some(recorded), Some(current)) if recorded == &current => AdapterCondition {
                kind,
                status: ConditionStatus::True,
                reason: None,
                resource: None,
            },
            (Some(_), Some(_)) => AdapterCondition {
                kind,
                status: ConditionStatus::False,
                reason: Some("resource bundle changed since enable".to_string()),
                resource: None,
            },
            _ => AdapterCondition {
                kind,
                status: ConditionStatus::Unknown,
                reason: Some("no digest recorded or resource root unavailable".to_string()),
                resource: None,
            },
        }
    }

    /// Run `openclaw plugins list` and decide whether `plugin_id` is still
    /// registered. Returns `(plugin_condition, verification_condition,
    /// plugin_registered_status)`.
    fn plugin_registered_condition(
        &self,
        plugin_id: &str,
        ctx: &DriverCtx,
    ) -> (AdapterCondition, AdapterCondition, ConditionStatus) {
        let plugin_ref = Some(ClaimResourceRef {
            id: RES_PLUGIN.to_string(),
        });
        let home = match openclaw_home(ctx.user_home.as_deref()) {
            Some(h) => h,
            None => {
                return (
                    AdapterCondition {
                        kind: AdapterConditionKind::PluginRegistered,
                        status: ConditionStatus::Unknown,
                        reason: Some("cannot resolve openclaw home".to_string()),
                        resource: plugin_ref,
                    },
                    AdapterCondition {
                        kind: AdapterConditionKind::VerificationSupported,
                        status: ConditionStatus::False,
                        reason: Some("openclaw home unresolved".to_string()),
                        resource: None,
                    },
                    ConditionStatus::Unknown,
                );
            }
        };
        let cmd = build_list_cmd(&home, ctx.user_home.as_deref());
        match ctx.ops.run_framework_cli(cmd) {
            Ok(output) if output.success() => {
                let registered = list_contains_plugin(&output.stdout, plugin_id);
                (
                    AdapterCondition {
                        kind: AdapterConditionKind::PluginRegistered,
                        status: bool_status(registered),
                        reason: (!registered)
                            .then(|| "plugin not present in `plugins list`".to_string()),
                        resource: plugin_ref,
                    },
                    AdapterCondition {
                        kind: AdapterConditionKind::VerificationSupported,
                        status: ConditionStatus::True,
                        reason: None,
                        resource: None,
                    },
                    bool_status(registered),
                )
            }
            // The list probe ran but failed, or could not spawn: we cannot
            // verify. Report Unknown, never a faked healthy/absent.
            Ok(_) | Err(_) => (
                AdapterCondition {
                    kind: AdapterConditionKind::PluginRegistered,
                    status: ConditionStatus::Unknown,
                    reason: Some("`plugins list` did not return a usable result".to_string()),
                    resource: plugin_ref,
                },
                AdapterCondition {
                    kind: AdapterConditionKind::VerificationSupported,
                    status: ConditionStatus::False,
                    reason: Some("`plugins list` unavailable".to_string()),
                    resource: None,
                },
                ConditionStatus::Unknown,
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (no spawning) — unit-testable
// ---------------------------------------------------------------------------

/// `OPENCLAW_BIN` override, else `openclaw`.
fn openclaw_bin() -> String {
    std::env::var("OPENCLAW_BIN")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "openclaw".to_string())
}

/// Resolve the OpenClaw home (also the state dir): `OPENCLAW_HOME`, else
/// `<user_home>/.openclaw`. Trailing slashes are trimmed to match the
/// official script.
fn openclaw_home(user_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("OPENCLAW_HOME") {
        let s = h.to_string_lossy();
        let trimmed = s.trim_end_matches('/');
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    user_home.map(|h| h.join(".openclaw"))
}

/// PATH prefix dirs, mirroring `install.sh`:
/// `<user_home>/.local/bin`, `<home>/bin`, `/usr/local/bin`.
fn path_prepend(home: &Path, user_home: Option<&Path>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(uh) = user_home {
        v.push(uh.join(".local/bin"));
    }
    v.push(home.join("bin"));
    v.push(PathBuf::from("/usr/local/bin"));
    v
}

/// Shared env contract for every OpenClaw invocation: unset
/// `OPENCLAW_HOME`, set `OPENCLAW_STATE_DIR` to the home, prepend PATH.
fn base_cmd(args: Vec<String>, home: &Path, user_home: Option<&Path>) -> FrameworkCommand {
    FrameworkCommand {
        program: openclaw_bin(),
        args,
        env_set: vec![(
            "OPENCLAW_STATE_DIR".to_string(),
            home.to_string_lossy().into_owned(),
        )],
        env_remove: vec!["OPENCLAW_HOME".to_string()],
        path_prepend: path_prepend(home, user_home),
        timeout: CLI_TIMEOUT,
    }
}

/// Build `openclaw plugins install <resource_root> --force
/// --dangerously-force-unsafe-install`.
fn build_install_cmd(
    resource_root: &Path,
    home: &Path,
    user_home: Option<&Path>,
) -> FrameworkCommand {
    base_cmd(
        vec![
            "plugins".to_string(),
            "install".to_string(),
            resource_root.to_string_lossy().into_owned(),
            "--force".to_string(),
            "--dangerously-force-unsafe-install".to_string(),
        ],
        home,
        user_home,
    )
}

/// Build `openclaw plugins uninstall <plugin_id> --force`.
///
/// `--force` skips OpenClaw's interactive confirmation — ANOLISA drives
/// the CLI non-interactively. `plugin_id` is validated by the caller.
fn build_uninstall_cmd(plugin_id: &str, home: &Path, user_home: Option<&Path>) -> FrameworkCommand {
    base_cmd(
        vec![
            "plugins".to_string(),
            "uninstall".to_string(),
            plugin_id.to_string(),
            "--force".to_string(),
        ],
        home,
        user_home,
    )
}

/// Build the read-only `openclaw plugins list`.
fn build_list_cmd(home: &Path, user_home: Option<&Path>) -> FrameworkCommand {
    base_cmd(
        vec!["plugins".to_string(), "list".to_string()],
        home,
        user_home,
    )
}

/// Plugin id declared by the OpenClaw-native plugin manifest, when present.
fn read_plugin_manifest_id(root: &Path) -> Result<Option<String>, AdapterError> {
    #[derive(serde::Deserialize)]
    struct PluginManifest {
        id: Option<String>,
    }

    let path = root.join("openclaw.plugin.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(AdapterError::Io { path, source }),
    };
    let manifest: PluginManifest =
        serde_json::from_slice(&bytes).map_err(|source| AdapterError::BundleInvalid {
            root: root.to_path_buf(),
            reason: format!(
                "failed to parse {} as OpenClaw plugin manifest: {source}",
                path.display()
            ),
        })?;
    let id =
        manifest
            .id
            .filter(|id| !id.is_empty())
            .ok_or_else(|| AdapterError::BundleInvalid {
                root: root.to_path_buf(),
                reason: format!("{} does not declare a non-empty id", path.display()),
            })?;
    Ok(Some(id))
}

/// Human-readable form of a command for dry-run/preview output. Display
/// only — never parsed back into an argv.
fn display_command(cmd: &FrameworkCommand) -> String {
    let mut s = String::new();
    for (k, v) in &cmd.env_set {
        s.push_str(&format!("{k}={v} "));
    }
    s.push_str(&cmd.program);
    for a in &cmd.args {
        s.push(' ');
        s.push_str(a);
    }
    s
}

/// True when `plugin_id` appears as a whole whitespace-delimited token on
/// any line of `plugins list` output. Tolerant of decoration like
/// `- tokenless (v1.2)`.
fn list_contains_plugin(stdout: &str, plugin_id: &str) -> bool {
    stdout
        .lines()
        .any(|line| line.split_whitespace().any(|tok| tok == plugin_id))
}

/// Extract the validated plugin id from a claim's `FrameworkPlugin`
/// resource, falling back to the top-level `plugin_id` field.
fn claim_plugin_id(claim: &AdapterClaim) -> Option<String> {
    for res in &claim.resources {
        if let ClaimResourceKind::FrameworkPlugin { plugin_id, .. } = &res.kind {
            return Some(plugin_id.clone());
        }
    }
    claim.plugin_id.clone()
}

/// Plugin id from a bundle, or [`AdapterError::BundleInvalid`] when none is
/// resolvable.
fn require_plugin_id(bundle: &AdapterBundle) -> Result<String, AdapterError> {
    bundle
        .plugin_id
        .clone()
        .ok_or_else(|| AdapterError::BundleInvalid {
            root: bundle.resource_root.clone(),
            reason: "no plugin id declared in manifest and none discoverable".to_string(),
        })
}

/// OpenClaw home, or [`AdapterError::FrameworkCli`] when `$HOME` is
/// unresolvable (no `user_home`, no `OPENCLAW_HOME`).
fn require_home(ctx: &DriverCtx) -> Result<PathBuf, AdapterError> {
    openclaw_home(ctx.user_home.as_deref()).ok_or_else(|| AdapterError::FrameworkCli {
        program: openclaw_bin(),
        reason: "cannot resolve OpenClaw home (no $HOME and no OPENCLAW_HOME)".to_string(),
    })
}

/// Compose a failure reason string from a non-success [`CliOutput`].
fn cli_failure_reason(verb: &str, output: &super::driver::CliOutput) -> String {
    if output.timed_out {
        return format!("'{verb}' timed out");
    }
    let code = output
        .status
        .map(|c| c.to_string())
        .unwrap_or_else(|| "killed".to_string());
    let mut reason = format!("'{verb}' exited with {code}");
    let stderr = output.stderr.trim();
    if !stderr.is_empty() {
        reason.push_str(": ");
        reason.push_str(stderr);
    }
    reason
}

/// Map a bool to a [`ConditionStatus`] (`true`→`True`, `false`→`False`).
fn bool_status(b: bool) -> ConditionStatus {
    if b {
        ConditionStatus::True
    } else {
        ConditionStatus::False
    }
}

/// Roll the framework-detect and plugin-registration signals into a
/// summary, honoring a `cleanup_failed` receipt.
fn summarize(
    claim_status: ClaimStatus,
    framework_detected: bool,
    plugin_registered: ConditionStatus,
) -> AdapterSummary {
    if claim_status == ClaimStatus::CleanupFailed {
        return AdapterSummary::CleanupFailed;
    }
    if !framework_detected {
        return AdapterSummary::Degraded;
    }
    match plugin_registered {
        ConditionStatus::True => AdapterSummary::Healthy,
        ConditionStatus::False => AdapterSummary::Degraded,
        ConditionStatus::Unknown => AdapterSummary::Unknown,
    }
}

/// SHA-256 digest of a directory tree, stable across runs: files are
/// hashed in sorted relative-path order as `path\0len\0bytes`. Returns
/// `None` on any IO error so callers fall back to `Unknown` rather than a
/// wrong verdict.
fn digest_tree(root: &Path) -> Option<String> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(root, &mut files).ok()?;
    files.sort();
    let mut hasher = Sha256::new();
    for path in &files {
        let rel = path.strip_prefix(root).unwrap_or(path);
        let bytes = std::fs::read(path).ok()?;
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update([0u8]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update([0u8]);
        hasher.update(&bytes);
    }
    Some(format!("sha256:{:x}", hasher.finalize()))
}

/// Recursively collect regular-file paths under `dir`. Symlinks are not
/// followed into directories (their link path is recorded as a file).
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// ISO 8601 UTC timestamp, second precision.
fn now_iso8601() -> String {
    use chrono::{SecondsFormat, Utc};
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_contains_plugin_matches_whole_token() {
        assert!(list_contains_plugin("tokenless\nother\n", "tokenless"));
        assert!(list_contains_plugin("- tokenless (v1.2)\n", "tokenless"));
        assert!(!list_contains_plugin("tokenless-extra\n", "tokenless"));
        assert!(!list_contains_plugin("", "tokenless"));
    }

    #[test]
    fn install_cmd_mirrors_script_contract() {
        let cmd = build_install_cmd(
            Path::new("/data/adapters/tokenless/openclaw"),
            Path::new("/home/u/.openclaw"),
            Some(Path::new("/home/u")),
        );
        assert_eq!(cmd.program, "openclaw");
        assert_eq!(
            cmd.args,
            vec![
                "plugins",
                "install",
                "/data/adapters/tokenless/openclaw",
                "--force",
                "--dangerously-force-unsafe-install",
            ]
        );
        assert!(cmd.env_remove.contains(&"OPENCLAW_HOME".to_string()));
        assert_eq!(
            cmd.env_set,
            vec![(
                "OPENCLAW_STATE_DIR".to_string(),
                "/home/u/.openclaw".to_string()
            )]
        );
        assert_eq!(cmd.path_prepend[0], PathBuf::from("/home/u/.local/bin"));
    }

    #[test]
    fn uninstall_cmd_uses_force() {
        let cmd = build_uninstall_cmd(
            "tokenless",
            Path::new("/home/u/.openclaw"),
            Some(Path::new("/home/u")),
        );
        assert_eq!(
            cmd.args,
            vec!["plugins", "uninstall", "tokenless", "--force"]
        );
    }

    #[test]
    fn plugin_manifest_id_is_read_from_real_openclaw_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("openclaw.plugin.json"),
            br#"{"id":"tokenless","name":"Tokenless"}"#,
        )
        .expect("write manifest");

        assert_eq!(
            read_plugin_manifest_id(dir.path()).expect("read"),
            Some("tokenless".to_string())
        );
    }

    #[test]
    fn summarize_prioritizes_cleanup_failed() {
        assert_eq!(
            summarize(ClaimStatus::CleanupFailed, true, ConditionStatus::True),
            AdapterSummary::CleanupFailed
        );
    }

    #[test]
    fn summarize_healthy_only_when_detected_and_registered() {
        assert_eq!(
            summarize(ClaimStatus::Enabled, true, ConditionStatus::True),
            AdapterSummary::Healthy
        );
        assert_eq!(
            summarize(ClaimStatus::Enabled, false, ConditionStatus::True),
            AdapterSummary::Degraded
        );
        assert_eq!(
            summarize(ClaimStatus::Enabled, true, ConditionStatus::False),
            AdapterSummary::Degraded
        );
        assert_eq!(
            summarize(ClaimStatus::Enabled, true, ConditionStatus::Unknown),
            AdapterSummary::Unknown
        );
    }

    #[test]
    fn digest_tree_is_stable_and_detects_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"hello").expect("write");
        std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
        std::fs::write(dir.path().join("sub/b.txt"), b"world").expect("write");

        let d1 = digest_tree(dir.path()).expect("digest");
        let d2 = digest_tree(dir.path()).expect("digest again");
        assert_eq!(d1, d2, "digest must be stable");

        std::fs::write(dir.path().join("sub/b.txt"), b"WORLD").expect("rewrite");
        let d3 = digest_tree(dir.path()).expect("digest after change");
        assert_ne!(d1, d3, "digest must change when a file changes");
    }
}
