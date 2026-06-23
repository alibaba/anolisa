//! Hermes framework driver.
//!
//! Hermes manages plugins and skills under `$HERMES_HOME` (default
//! `~/.hermes`). `enable` places the plugin by copying the resource root
//! into `$HERMES_HOME/plugins/<plugin_id>/` via
//! [`AdapterOps::copy_tree`](super::driver::AdapterOps::copy_tree), then
//! runs `hermes plugins enable <plugin_id>`. `disable` runs
//! `hermes plugins disable <plugin_id>`, then removes the placed plugin
//! directory and any delivered skill directories through
//! [`AdapterOps::remove_tree`](super::driver::AdapterOps::remove_tree).
//! Skill directories discovered under `<resource_root>/skills/` are
//! copied into `$HERMES_HOME/skills/` during enable. Status uses the
//! read-only `hermes plugins list`. All CLI and filesystem operations go
//! through the Manager's [`AdapterOps`](super::driver::AdapterOps) —
//! the driver never performs direct IO.
//!
//! The CLI env contract: `HERMES_BIN` overrides the executable (used by
//! tests to point at a fake CLI); `HERMES_HOME` overrides the home
//! directory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::AdapterError;
use super::claim::{
    AdapterClaim, CLAIM_SCHEMA_VERSION, ClaimResource, ClaimResourceKind, ClaimStatus,
    DRIVER_SCHEMA_VERSION, DriverPayload, HermesClaim, validate_plugin_id,
};
use super::driver::{
    AdapterBundle, AdapterCondition, AdapterConditionKind, AdapterStatusReport, AdapterSummary,
    ClaimResourceRef, ConditionStatus, DetectResult, DisableReport, DriverCtx, DriverPlan,
    FrameworkCommand, FrameworkDriver, HostEnv, find_binary_in_path,
};

/// Default timeout for a Hermes CLI invocation.
const CLI_TIMEOUT: Duration = Duration::from_secs(60);

/// Resource ids used in Hermes receipts. Stable strings referenced from
/// the [`HermesClaim`] payload and condition reports.
const RES_HOME: &str = "hermes_home";
const RES_PLUGIN: &str = "hermes_plugin";

/// Hermes driver. Stateless; all per-operation context arrives via
/// [`DriverCtx`].
pub struct HermesDriver;

impl HermesDriver {
    /// Construct the driver.
    pub fn new() -> Self {
        Self
    }
}

impl Default for HermesDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkDriver for HermesDriver {
    fn name(&self) -> &'static str {
        "hermes"
    }

    fn detect(&self, env: &HostEnv) -> DetectResult {
        match find_binary_in_path(&hermes_bin()) {
            Some(path) => DetectResult {
                detected: true,
                reason: format!("hermes CLI found at {}", path.display()),
            },
            None => {
                // The CLI is what enable/disable need; a bare home dir is
                // not sufficient. Report not-detected but mention the home
                // so a user understands the framework is partially present.
                let home_note = hermes_home(env.user_home.as_deref())
                    .filter(|h| h.exists())
                    .map(|h| format!(" (home {} exists but CLI is not on PATH)", h.display()))
                    .unwrap_or_default();
                DetectResult {
                    detected: false,
                    reason: format!("hermes CLI not found on PATH{home_note}"),
                }
            }
        }
    }

    fn allowed_external_roots(&self, ctx: &DriverCtx) -> Vec<PathBuf> {
        // The only external root Hermes writes is its own home dir.
        hermes_home(ctx.user_home.as_deref()).into_iter().collect()
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

        let plugin_id = match ctx.declared_plugin_id.clone().filter(|id| !id.is_empty()) {
            Some(id) => Some(id),
            None => read_plugin_manifest_id(root, ctx.declared_bundle_entry.as_deref())?
                .or_else(|| Some(ctx.component.clone())),
        };

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
        let enable_cmd = build_enable_cmd(&plugin_id);

        let mut actions = vec![
            format!(
                "place hermes plugin '{plugin_id}' from {} to {}/plugins/{plugin_id}",
                bundle.resource_root.display(),
                home.display(),
            ),
            format!("enable hermes plugin '{plugin_id}'"),
        ];

        for skill in &ctx.declared_skills {
            let src_display = match skill.source {
                Some(ref s) => s.display().to_string(),
                None => format!("{}/skills/{}", bundle.resource_root.display(), skill.name,),
            };
            actions.push(format!(
                "deliver skill '{}' from {} to {}/skills/{}",
                skill.name,
                src_display,
                home.display(),
                skill.name,
            ));
        }

        Ok(DriverPlan {
            framework: self.name().to_string(),
            component: ctx.component.clone(),
            actions,
            register_command: Some(display_command(&enable_cmd)),
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

        let mut resources = vec![
            ClaimResource {
                id: RES_HOME.to_string(),
                purpose: "hermes_home".to_string(),
                kind: ClaimResourceKind::ExternalPath { path: home.clone() },
            },
            ClaimResource {
                id: RES_PLUGIN.to_string(),
                purpose: "hermes_plugin_dir".to_string(),
                kind: ClaimResourceKind::ExternalPath {
                    path: home.join("plugins").join(&plugin_id),
                },
            },
        ];

        let mut skill_resource_ids = Vec::new();
        for skill in &ctx.declared_skills {
            let res_id = format!("hermes_skill_{}", skill.name);
            resources.push(ClaimResource {
                id: res_id.clone(),
                purpose: "hermes_skill".to_string(),
                kind: ClaimResourceKind::ExternalPath {
                    path: home.join("skills").join(&skill.name),
                },
            });
            skill_resource_ids.push(res_id);
        }

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
            driver_payload: DriverPayload::Hermes(HermesClaim {
                home_resource: RES_HOME.to_string(),
                plugin_resource: RES_PLUGIN.to_string(),
                skill_resources: skill_resource_ids,
            }),
        })
    }

    fn apply_enable(&self, claim: &AdapterClaim, ctx: &DriverCtx) -> Result<(), AdapterError> {
        let plugin_id = claim_plugin_id(claim).ok_or_else(|| AdapterError::BundleInvalid {
            root: claim.resource_root.clone(),
            reason: "hermes receipt has no plugin id".to_string(),
        })?;
        validate_plugin_id(&plugin_id)?;
        let home = require_home(ctx)?;

        // 1. Place the plugin: copy the resource root into
        //    $HERMES_HOME/plugins/<plugin_id>/, excluding the `skills/`
        //    subtree (delivered separately to $HERMES_HOME/skills/).
        let plugin_dest = home.join("plugins").join(&plugin_id);
        copy_bundle_excluding_skills(&claim.resource_root, &plugin_dest, ctx)?;

        // 2. Enable the placed plugin via the Hermes CLI.
        let enable_cmd = build_enable_cmd(&plugin_id);
        let program = enable_cmd.program.clone();
        let output = ctx.ops.run_framework_cli(enable_cmd)?;
        if !output.success() {
            return Err(AdapterError::FrameworkCli {
                program,
                reason: cli_failure_reason("plugins enable", &output),
            });
        }

        // 3. Deliver skills through the Manager's controlled IO.
        for skill in &ctx.declared_skills {
            let src = skill
                .source
                .clone()
                .unwrap_or_else(|| claim.resource_root.join("skills").join(&skill.name));
            let dst = home.join("skills").join(&skill.name);
            ctx.ops.copy_tree(&src, &dst)?;
        }

        Ok(())
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
                    reason: Some("hermes CLI unavailable".to_string()),
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

        if find_binary_in_path(&hermes_bin()).is_none() {
            return Ok(DisableReport {
                cleanup_complete: false,
                messages: vec![
                    "hermes CLI not found on PATH; receipt kept so cleanup can be retried"
                        .to_string(),
                ],
            });
        }

        let mut messages = Vec::new();
        let mut cleanup_complete = true;

        let home = require_home(ctx)?;

        // 1. Disable the plugin (ignore failure — might already be disabled).
        let disable_cmd = build_disable_cmd(&plugin_id);
        let _ = ctx.ops.run_framework_cli(disable_cmd);

        // 2. Remove the placed plugin directory through controlled IO.
        let plugin_dir = home.join("plugins").join(&plugin_id);
        match ctx.ops.remove_tree(&plugin_dir) {
            Ok(true) => messages.push(format!(
                "removed hermes plugin directory {}",
                plugin_dir.display()
            )),
            Ok(false) => messages.push(format!(
                "hermes plugin directory {} already absent",
                plugin_dir.display()
            )),
            Err(err) => {
                cleanup_complete = false;
                messages.push(format!(
                    "failed to remove hermes plugin directory {}: {err}",
                    plugin_dir.display()
                ));
            }
        }

        // 3. Remove skill directories through Manager IO.
        if let DriverPayload::Hermes(ref hermes) = claim.driver_payload {
            for skill_res_id in &hermes.skill_resources {
                if let Some(resource) = claim.resource(skill_res_id) {
                    if let ClaimResourceKind::ExternalPath { path } = &resource.kind {
                        match ctx.ops.remove_tree(path) {
                            Ok(true) => {
                                messages.push(format!("removed skill dir {}", path.display()));
                            }
                            Ok(false) => {} // already gone
                            Err(err) => {
                                cleanup_complete = false;
                                messages.push(format!(
                                    "failed to remove skill dir {}: {err}",
                                    path.display()
                                ));
                            }
                        }
                    }
                }
            }
        }

        Ok(DisableReport {
            cleanup_complete,
            messages,
        })
    }
}

impl HermesDriver {
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

    /// Run `hermes plugins list` and decide whether `plugin_id` is still
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
        let cmd = build_list_cmd();
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

/// `HERMES_BIN` override, else `hermes`.
fn hermes_bin() -> String {
    std::env::var("HERMES_BIN")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "hermes".to_string())
}

/// Resolve the Hermes home directory: `HERMES_HOME`, else
/// `<user_home>/.hermes`. Trailing slashes are trimmed.
fn hermes_home(user_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("HERMES_HOME") {
        let s = h.to_string_lossy();
        let trimmed = s.trim_end_matches('/');
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    user_home.map(|h| h.join(".hermes"))
}

/// Build a bare Hermes command with the given args. Hermes has a simpler
/// env contract than OpenClaw: no state-dir rewriting, no PATH prepend.
fn base_cmd(args: Vec<String>) -> FrameworkCommand {
    FrameworkCommand {
        program: hermes_bin(),
        args,
        env_set: Vec::new(),
        env_remove: Vec::new(),
        path_prepend: Vec::new(),
        timeout: CLI_TIMEOUT,
    }
}

/// Build `hermes plugins enable <plugin_id>`.
fn build_enable_cmd(plugin_id: &str) -> FrameworkCommand {
    base_cmd(vec![
        "plugins".to_string(),
        "enable".to_string(),
        plugin_id.to_string(),
    ])
}

/// Build `hermes plugins disable <plugin_id>`.
fn build_disable_cmd(plugin_id: &str) -> FrameworkCommand {
    base_cmd(vec![
        "plugins".to_string(),
        "disable".to_string(),
        plugin_id.to_string(),
    ])
}

/// Build the read-only `hermes plugins list`.
fn build_list_cmd() -> FrameworkCommand {
    base_cmd(vec!["plugins".to_string(), "list".to_string()])
}

/// Plugin id declared by a Hermes-native manifest, when present.
///
/// When `declared_entry` is given, uses only that file; otherwise tries
/// `hermes.plugin.json` then `hermes.manifest.yaml`. Falls back to `None`.
///
/// File format is determined by extension: `.yaml`/`.yml` are parsed with
/// a simple line scan for `id: <value>`; `.json` (and the default
/// fallback `hermes.plugin.json`) are parsed as JSON.
fn read_plugin_manifest_id(
    root: &Path,
    declared_entry: Option<&str>,
) -> Result<Option<String>, AdapterError> {
    if let Some(entry) = declared_entry {
        let lower = entry.to_ascii_lowercase();
        if lower.ends_with(".yaml") || lower.ends_with(".yml") {
            return read_yaml_id(root, entry);
        }
        return read_json_id(root, entry);
    }

    // No declared entry: try JSON then YAML fallback.
    if let Some(id) = read_json_id(root, "hermes.plugin.json")? {
        return Ok(Some(id));
    }
    read_yaml_id(root, "hermes.manifest.yaml")
}

/// Read the `id` field from a JSON manifest file.
fn read_json_id(root: &Path, filename: &str) -> Result<Option<String>, AdapterError> {
    #[derive(serde::Deserialize)]
    struct PluginManifest {
        id: Option<String>,
    }

    let path = root.join(filename);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(AdapterError::Io { path, source }),
    };
    let manifest: PluginManifest =
        serde_json::from_slice(&bytes).map_err(|source| AdapterError::BundleInvalid {
            root: root.to_path_buf(),
            reason: format!(
                "failed to parse {} as Hermes plugin manifest: {source}",
                path.display()
            ),
        })?;
    Ok(manifest.id.filter(|id| !id.is_empty()))
}

/// Read the `id` field from a YAML manifest via a minimal line scan.
fn read_yaml_id(root: &Path, filename: &str) -> Result<Option<String>, AdapterError> {
    let path = root.join(filename);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(AdapterError::Io { path, source }),
    };
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("id:") {
            let value = rest.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return Ok(Some(value.to_string()));
            }
        }
    }
    Ok(None)
}

/// Copy the bundle directory into the plugin destination, skipping the
/// `skills/` subdirectory (delivered separately to `$HERMES_HOME/skills/`).
/// All IO goes through `ctx.ops` so the Manager enforces path boundaries.
fn copy_bundle_excluding_skills(
    src: &Path,
    dst: &Path,
    ctx: &DriverCtx,
) -> Result<(), AdapterError> {
    let entries = std::fs::read_dir(src).map_err(|source| AdapterError::Io {
        path: src.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| AdapterError::Io {
            path: src.to_path_buf(),
            source,
        })?;
        let name = entry.file_name();
        if name == "skills" {
            continue;
        }
        let src_child = entry.path();
        let dst_child = dst.join(&name);
        if src_child.is_dir() {
            ctx.ops.copy_tree(&src_child, &dst_child)?;
        } else {
            ctx.ops.copy_file(&src_child, &dst_child)?;
        }
    }
    Ok(())
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
/// `- agent-sec (v1.2)`.
fn list_contains_plugin(stdout: &str, plugin_id: &str) -> bool {
    stdout
        .lines()
        .any(|line| line.split_whitespace().any(|tok| tok == plugin_id))
}

/// Extract the validated plugin id from a claim's resources, falling back
/// to the top-level `plugin_id` field.
fn claim_plugin_id(claim: &AdapterClaim) -> Option<String> {
    // Hermes uses ExternalPath for the plugin dir; the plugin id is in the
    // top-level field.
    claim.plugin_id.clone()
}

/// Plugin id from a bundle, or [`AdapterError::BundleInvalid`] when none
/// is resolvable.
fn require_plugin_id(bundle: &AdapterBundle) -> Result<String, AdapterError> {
    bundle
        .plugin_id
        .clone()
        .ok_or_else(|| AdapterError::BundleInvalid {
            root: bundle.resource_root.clone(),
            reason: "no plugin id declared in manifest and none discoverable".to_string(),
        })
}

/// Hermes home, or [`AdapterError::FrameworkCli`] when `$HOME` is
/// unresolvable (no `user_home`, no `HERMES_HOME`).
fn require_home(ctx: &DriverCtx) -> Result<PathBuf, AdapterError> {
    hermes_home(ctx.user_home.as_deref()).ok_or_else(|| AdapterError::FrameworkCli {
        program: hermes_bin(),
        reason: "cannot resolve Hermes home (no $HOME and no HERMES_HOME)".to_string(),
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

/// Map a bool to a [`ConditionStatus`] (`true` -> `True`, `false` -> `False`).
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
    fn hermes_home_resolution() {
        // SAFETY: test-only; env mutation is acceptable in serial tests.
        unsafe {
            // With env var set.
            std::env::set_var("HERMES_HOME", "/opt/hermes");
            assert_eq!(
                hermes_home(Some(Path::new("/home/alice"))),
                Some(PathBuf::from("/opt/hermes"))
            );
            // Trailing slashes are stripped.
            std::env::set_var("HERMES_HOME", "/opt/hermes///");
            assert_eq!(
                hermes_home(Some(Path::new("/home/alice"))),
                Some(PathBuf::from("/opt/hermes"))
            );
            // Empty env var falls back to user_home.
            std::env::set_var("HERMES_HOME", "");
            assert_eq!(
                hermes_home(Some(Path::new("/home/alice"))),
                Some(PathBuf::from("/home/alice/.hermes"))
            );
            // No env var, no user_home.
            std::env::remove_var("HERMES_HOME");
        }
        assert_eq!(hermes_home(None), None);
        // No env var, with user_home.
        assert_eq!(
            hermes_home(Some(Path::new("/home/bob"))),
            Some(PathBuf::from("/home/bob/.hermes"))
        );
    }

    #[test]
    fn list_contains_plugin_matches_whole_token() {
        assert!(list_contains_plugin("agent-sec\nother\n", "agent-sec"));
        assert!(list_contains_plugin("- agent-sec (v1.2)\n", "agent-sec"));
        assert!(!list_contains_plugin("agent-sec-extra\n", "agent-sec"));
        assert!(!list_contains_plugin("", "agent-sec"));
    }

    #[test]
    fn enable_cmd_shape() {
        let cmd = build_enable_cmd("agent-sec");
        assert_eq!(cmd.program, "hermes");
        assert_eq!(cmd.args, vec!["plugins", "enable", "agent-sec"]);
    }

    #[test]
    fn disable_cmd_shape() {
        let cmd = build_disable_cmd("agent-sec");
        assert_eq!(cmd.args, vec!["plugins", "disable", "agent-sec"]);
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

    // -- review fix coverage: lazy plugin_id, YAML entry, skills allowlist --

    use crate::adapter::claim::{DriverPayload, HermesClaim};
    use crate::adapter::driver::{AdapterOps, CliOutput};

    struct StubOps;

    impl AdapterOps for StubOps {
        fn run_framework_cli(&self, _: FrameworkCommand) -> Result<CliOutput, AdapterError> {
            unimplemented!()
        }
        fn copy_tree(&self, _: &Path, _: &Path) -> Result<(), AdapterError> {
            unimplemented!()
        }
        fn copy_file(&self, _: &Path, _: &Path) -> Result<(), AdapterError> {
            unimplemented!()
        }
        fn remove_tree(&self, _: &Path) -> Result<bool, AdapterError> {
            unimplemented!()
        }
    }

    #[test]
    fn declared_plugin_id_skips_manifest_parse() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Invalid content that would fail if read_plugin_manifest_id parsed it.
        std::fs::write(dir.path().join("hermes.plugin.json"), b"NOT JSON {{").expect("write");

        let driver = HermesDriver::new();
        let ops = StubOps;
        let layout = anolisa_platform::fs_layout::FsLayout::user(PathBuf::from("/tmp/test-home-a"));
        let ctx = DriverCtx {
            component: "test-comp".to_string(),
            framework: "hermes".to_string(),
            layout: &layout,
            resource_root: dir.path().to_path_buf(),
            user_home: Some(PathBuf::from("/tmp/test-home-a")),
            declared_plugin_id: Some("agent-sec".to_string()),
            declared_skills: Vec::new(),
            declared_config: Vec::new(),
            declared_bundle_entry: None,
            dry_run: true,
            ops: &ops,
        };

        let bundle = driver
            .read_bundle(&ctx)
            .expect("must succeed without parsing manifest");
        assert_eq!(bundle.plugin_id.as_deref(), Some("agent-sec"));
    }

    #[test]
    fn yaml_bundle_entry_parses_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("hermes.manifest.yaml"),
            "id: agent-sec\nname: Agent Security\n",
        )
        .expect("write");

        let id = read_plugin_manifest_id(dir.path(), Some("hermes.manifest.yaml"))
            .expect("parse must succeed")
            .expect("id must be present");
        assert_eq!(id, "agent-sec");
    }

    #[test]
    fn only_declared_skills_are_planned_and_claimed() {
        use crate::adapter::driver::DeclaredSkill;
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("skills/sec-audit")).expect("mkdir");
        std::fs::create_dir_all(root.join("skills/undeclared-extra")).expect("mkdir");
        std::fs::write(root.join("dummy.txt"), b"x").expect("write");

        let driver = HermesDriver::new();
        let ops = StubOps;
        let layout = anolisa_platform::fs_layout::FsLayout::user(PathBuf::from("/tmp/test-home-c"));
        let ctx = DriverCtx {
            component: "test-comp".to_string(),
            framework: "hermes".to_string(),
            layout: &layout,
            resource_root: root.to_path_buf(),
            user_home: Some(PathBuf::from("/tmp/test-home-c")),
            declared_plugin_id: Some("test-plugin".to_string()),
            declared_skills: vec![DeclaredSkill {
                name: "sec-audit".to_string(),
                source: None,
            }],
            declared_config: Vec::new(),
            declared_bundle_entry: None,
            dry_run: true,
            ops: &ops,
        };

        let bundle = AdapterBundle {
            resource_root: root.to_path_buf(),
            digest: None,
            plugin_id: Some("test-plugin".to_string()),
        };

        let plan = driver.plan_enable(&bundle, &ctx).expect("plan_enable");
        assert!(
            plan.actions.iter().any(|a| a.contains("sec-audit")),
            "declared skill must appear in plan"
        );
        assert!(
            !plan.actions.iter().any(|a| a.contains("undeclared-extra")),
            "undeclared skill must not appear in plan"
        );

        let claim = driver
            .prepare_enable(&bundle, &ctx)
            .expect("prepare_enable");
        let skill_resources: Vec<&str> = claim
            .resources
            .iter()
            .filter(|r| r.purpose == "hermes_skill")
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(skill_resources, vec!["hermes_skill_sec-audit"]);
        if let DriverPayload::Hermes(HermesClaim {
            ref skill_resources,
            ..
        }) = claim.driver_payload
        {
            assert_eq!(skill_resources, &["hermes_skill_sec-audit"]);
        } else {
            panic!("expected Hermes driver payload");
        }
    }
}
