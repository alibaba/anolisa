//! OpenClaw framework driver.
//!
//! OpenClaw plugin adapters use the CLI-managed registry: `enable` runs
//! `openclaw plugins install <resource_root>` and `disable` runs
//! `openclaw plugins uninstall <plugin_id>`. Skill-only adapters
//! (`adapter_type = "skill_bundle"`) skip registry operations and only
//! copy declared skills into the OpenClaw skills directory. Status is the
//! read-only `openclaw plugins list` for plugin adapters. All CLI and
//! filesystem operations go through the Manager's helpers — the driver
//! only builds argv arrays from validated data.
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

        let plugin_id = if ctx.is_skill_bundle() {
            None
        } else {
            let manifest_file = ctx
                .declared_bundle_entry
                .as_deref()
                .unwrap_or("openclaw.plugin.json");
            ctx.declared_plugin_id
                .clone()
                .or(read_plugin_manifest_id(root, manifest_file)?)
                .or_else(|| Some(ctx.component.clone()))
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
        let home = require_home(ctx)?;
        let mut actions = Vec::new();
        let mut register_command = None;
        if !ctx.is_skill_bundle() {
            let plugin_id = require_plugin_id(bundle)?;
            validate_plugin_id(&plugin_id)?;
            let cmd = build_install_cmd(&bundle.resource_root, &home, ctx.user_home.as_deref());
            register_command = Some(display_command(&cmd));
            actions.push(format!(
                "register openclaw plugin '{plugin_id}' from {}",
                bundle.resource_root.display()
            ));
        }

        for skill in &ctx.declared_skills {
            let src_display = match skill.source {
                Some(ref s) => s.display().to_string(),
                None => format!("{}/skills/{}", bundle.resource_root.display(), skill.name,),
            };
            actions.push(format!(
                "deliver openclaw skill '{}' from {} to {}/skills/{}",
                skill.name,
                src_display,
                home.display(),
                skill.name,
            ));
        }
        if !ctx.is_skill_bundle() {
            for (i, cfg) in ctx.declared_config.iter().enumerate() {
                actions.push(format!(
                    "set openclaw config [{i}] {} = {}",
                    cfg.key,
                    config_value_display(&cfg.value)
                ));
            }
        }

        Ok(DriverPlan {
            framework: self.name().to_string(),
            component: ctx.component.clone(),
            actions,
            register_command,
        })
    }

    fn prepare_enable(
        &self,
        bundle: &AdapterBundle,
        ctx: &DriverCtx,
    ) -> Result<AdapterClaim, AdapterError> {
        let home = require_home(ctx)?;
        let mut resources = vec![ClaimResource {
            id: RES_STATE_DIR.to_string(),
            purpose: "openclaw_state_dir".to_string(),
            kind: ClaimResourceKind::ExternalPath { path: home.clone() },
        }];
        let plugin_id = if ctx.is_skill_bundle() {
            None
        } else {
            let plugin_id = require_plugin_id(bundle)?;
            validate_plugin_id(&plugin_id)?;
            resources.push(ClaimResource {
                id: RES_PLUGIN.to_string(),
                purpose: "openclaw_plugin".to_string(),
                kind: ClaimResourceKind::FrameworkPlugin {
                    framework: self.name().to_string(),
                    plugin_id: plugin_id.clone(),
                },
            });
            Some(plugin_id)
        };

        let mut skill_resources = Vec::new();
        for skill in &ctx.declared_skills {
            let res_id = format!("openclaw_skill_{}", skill.name);
            resources.push(ClaimResource {
                id: res_id.clone(),
                purpose: "openclaw_skill".to_string(),
                kind: ClaimResourceKind::ExternalPath {
                    path: home.join("skills").join(&skill.name),
                },
            });
            skill_resources.push(res_id);
        }

        let mut config_resources = Vec::new();
        if !ctx.is_skill_bundle() {
            for (i, cfg) in ctx.declared_config.iter().enumerate() {
                let res_id = format!("openclaw_config_{i}");
                resources.push(ClaimResource {
                    id: res_id.clone(),
                    purpose: "openclaw_config".to_string(),
                    kind: ClaimResourceKind::FrameworkConfig {
                        framework: self.name().to_string(),
                        key: cfg.key.clone(),
                    },
                });
                config_resources.push(res_id);
            }
        }

        Ok(AdapterClaim {
            claim_schema: CLAIM_SCHEMA_VERSION,
            component: ctx.component.clone(),
            framework: self.name().to_string(),
            plugin_id,
            adapter_type: ctx.adapter_type.clone(),
            enabled_at: now_iso8601(),
            resource_root: bundle.resource_root.clone(),
            bundle_digest: bundle.digest.clone(),
            driver_schema: DRIVER_SCHEMA_VERSION,
            status: ClaimStatus::Enabled,
            resources,
            driver_payload: DriverPayload::OpenClaw(OpenClawClaim {
                state_dir_resource: RES_STATE_DIR.to_string(),
                plugin_resource: if ctx.is_skill_bundle() {
                    String::new()
                } else {
                    RES_PLUGIN.to_string()
                },
                skill_resources,
                config_resources,
            }),
        })
    }

    fn apply_enable(&self, claim: &AdapterClaim, ctx: &DriverCtx) -> Result<(), AdapterError> {
        let home = require_home(ctx)?;

        if !ctx.is_skill_bundle() {
            let plugin_id = claim_plugin_id(claim).ok_or_else(|| AdapterError::BundleInvalid {
                root: claim.resource_root.clone(),
                reason: "openclaw receipt has no plugin id".to_string(),
            })?;
            validate_plugin_id(&plugin_id)?;

            let cmd = build_install_cmd(&claim.resource_root, &home, ctx.user_home.as_deref());
            let program = cmd.program.clone();
            let output = ctx.ops.run_framework_cli(cmd)?;
            if !output.success() {
                return Err(AdapterError::FrameworkCli {
                    program,
                    reason: cli_failure_reason("plugins install", &output),
                });
            }
        }

        for skill in &ctx.declared_skills {
            let src = skill
                .source
                .clone()
                .unwrap_or_else(|| ctx.resource_root.join("skills").join(&skill.name));
            let dst = home.join("skills").join(&skill.name);
            ctx.ops.copy_tree(&src, &dst)?;
        }

        if !ctx.is_skill_bundle() {
            for cfg in &ctx.declared_config {
                let cmd =
                    build_config_set_cmd(&cfg.key, &cfg.value, &home, ctx.user_home.as_deref());
                let program = cmd.program.clone();
                let output = ctx.ops.run_framework_cli(cmd)?;
                if !output.success() {
                    return Err(AdapterError::FrameworkCli {
                        program,
                        reason: cli_failure_reason("config set", &output),
                    });
                }
            }
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

        // 3. Plugin still registered? Skill-only receipts have no plugin
        //    registry entry by design, so status does not require one.
        let plugin_registered = if claim.is_skill_bundle() {
            conditions.push(AdapterCondition {
                kind: AdapterConditionKind::VerificationSupported,
                status: bool_status(detect.detected),
                reason: Some("skill_bundle has no plugin registry entry".to_string()),
                resource: None,
            });
            ConditionStatus::True
        } else {
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
            plugin_registered
        };

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
        let home = require_home(ctx)?;
        let mut messages = Vec::new();
        let mut cleanup_complete = true;

        if let Some(plugin_id) = claim_plugin_id(claim) {
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

            let cmd = build_uninstall_cmd(&plugin_id, &home, ctx.user_home.as_deref());
            let output = ctx.ops.run_framework_cli(cmd)?;
            if output.success() {
                messages.push(format!("unregistered openclaw plugin '{plugin_id}'"));
            } else {
                return Ok(DisableReport {
                    cleanup_complete: false,
                    messages: vec![format!(
                        "openclaw plugin uninstall failed: {}",
                        cli_failure_reason("plugins uninstall", &output)
                    )],
                });
            }
        } else {
            messages.push("receipt records no plugin to unregister".to_string());
        }

        let skill_resources = claim_skill_resources(claim);
        for skill_name in &skill_resources {
            let skill_dir = home.join("skills").join(skill_name);
            match ctx.ops.remove_tree(&skill_dir) {
                Ok(true) => messages.push(format!(
                    "removed openclaw skill dir {}",
                    skill_dir.display()
                )),
                Ok(false) => {} // already gone, idempotent
                Err(err) => {
                    messages.push(format!(
                        "failed to remove skill dir {}: {err}",
                        skill_dir.display()
                    ));
                    cleanup_complete = false;
                }
            }
        }

        // 3. Config entries are NOT reversed on disable (framework-wide
        //    config should persist).
        let config_count = claim_config_count(claim);
        if config_count > 0 {
            messages.push(format!(
                "{config_count} openclaw config entries left in place (not reversed on disable)"
            ));
        }

        Ok(DisableReport {
            cleanup_complete,
            messages,
        })
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
fn read_plugin_manifest_id(root: &Path, filename: &str) -> Result<Option<String>, AdapterError> {
    #[derive(serde::Deserialize)]
    struct PluginManifest {
        id: Option<String>,
    }

    let path = root.join(filename);
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

/// True when `plugin_id` appears in the `plugins list` output.
///
/// Handles three output shapes:
/// 1. Plain text — each line has whitespace-delimited tokens.
/// 2. Rich table without wrapping — tokens appear between │ delimiters.
/// 3. Rich table with wrapping — a cell value is split across
///    consecutive physical lines within the same column.
///
/// ANSI escape codes are stripped before any matching.
fn list_contains_plugin(stdout: &str, plugin_id: &str) -> bool {
    let stripped = strip_ansi(stdout);

    // Fast path: exact whitespace-token match on lines that are NOT
    // table data lines. Table data lines (containing │/┃/║) must go
    // through the table parser, because a wrapped cell fragment can
    // look like a complete token on a single physical line.
    if stripped.lines().any(|line| {
        !line.contains(|c: char| is_cell_delimiter(c))
            && line.split_whitespace().any(|tok| tok == plugin_id)
    }) {
        return true;
    }

    // Table-aware path: parse rows, concatenate wrapped cell text per
    // column, then search each concatenated cell.
    table_contains_token(&stripped, plugin_id)
}

/// Strip ANSI escape sequences (CSI and OSC) from `s`.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    // CSI: consume until a final byte (0x40..=0x7E).
                    for c in chars.by_ref() {
                        if matches!(c, '\x40'..='\x7e') {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    // OSC: consume until BEL or ST.
                    for c in chars.by_ref() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn is_cell_delimiter(c: char) -> bool {
    matches!(c, '│' | '┃' | '║')
}

fn is_border_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && trimmed.chars().all(|c| {
            is_cell_delimiter(c)
                || matches!(
                    c,
                    '─' | '━'
                        | '═'
                        | '┌'
                        | '┐'
                        | '└'
                        | '┘'
                        | '├'
                        | '┤'
                        | '┬'
                        | '┴'
                        | '┼'
                        | '┏'
                        | '┓'
                        | '┗'
                        | '┛'
                        | '┣'
                        | '┫'
                        | '┳'
                        | '┻'
                        | '╋'
                        | '┡'
                        | '┩'
                        | '╇'
                        | '╔'
                        | '╗'
                        | '╚'
                        | '╝'
                        | '╠'
                        | '╣'
                        | '╦'
                        | '╩'
                        | '╬'
                        | ' '
                )
        })
}

/// Extract cell text from a line delimited by │/┃/║. Returns `None`
/// when the line has no cell delimiters (not a table data line).
fn extract_cells(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.contains(|c: char| is_cell_delimiter(c)) {
        return None;
    }
    let parts: Vec<&str> = trimmed.split(|c: char| is_cell_delimiter(c)).collect();
    if parts.len() < 3 {
        return None;
    }
    // Skip the empty segments before the first and after the last │.
    let cells: Vec<String> = parts[1..parts.len() - 1]
        .iter()
        .map(|cell| cell.trim().to_string())
        .collect();
    Some(cells)
}

/// Parse rich-table output into logical rows (merging physical
/// continuation lines), then check whether any cell in any row
/// matches `token` as a whitespace-delimited word.
///
/// A continuation line is detected by the *last* column being empty.
/// In `plugins list` tables the last column is typically Status
/// (`enabled`/`disabled`), which is always populated on the first
/// physical line of a row but empty on continuation lines. This
/// correctly handles the case where *both* Name and ID wrap.
fn table_contains_token(text: &str, token: &str) -> bool {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut current: Option<Vec<String>> = None;

    for line in text.lines() {
        if is_border_line(line) {
            if let Some(cells) = current.take() {
                rows.push(cells);
            }
            continue;
        }

        if let Some(cells) = extract_cells(line) {
            let is_continuation = current.is_some() && cells.last().is_some_and(|c| c.is_empty());

            if is_continuation {
                if let Some(cur) = current.as_mut() {
                    for (i, cell) in cells.into_iter().enumerate() {
                        if i < cur.len() && !cell.is_empty() {
                            cur[i].push_str(&cell);
                        }
                    }
                }
            } else {
                if let Some(prev) = current.take() {
                    rows.push(prev);
                }
                current = Some(cells);
            }
        }
    }
    if let Some(cells) = current {
        rows.push(cells);
    }

    rows.iter().any(|row| {
        row.iter().any(|cell| {
            let trimmed = cell.trim();
            trimmed == token || trimmed.split_whitespace().any(|t| t == token)
        })
    })
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

/// Build `openclaw config set <key> <value>`.
fn build_config_set_cmd(
    key: &str,
    value: &toml::Value,
    home: &Path,
    user_home: Option<&Path>,
) -> FrameworkCommand {
    base_cmd(
        vec![
            "config".to_string(),
            "set".to_string(),
            key.to_string(),
            config_value_to_cli_string(value),
        ],
        home,
        user_home,
    )
}

/// Convert a TOML value to a string suitable for the `openclaw config set`
/// CLI argument. Strings are passed bare (no quotes); other types use the
/// TOML display representation.
fn config_value_to_cli_string(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        other => other.to_string(),
    }
}

/// Human-readable display of a config value for plan output.
fn config_value_display(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    }
}

/// Extract skill names from a claim's `skill_resources` by parsing the
/// resource ids. Each id has the form `openclaw_skill_<name>`, and we
/// extract `<name>` as the directory name under `<home>/skills/`.
fn claim_skill_resources(claim: &AdapterClaim) -> Vec<String> {
    let payload = match &claim.driver_payload {
        DriverPayload::OpenClaw(oc) => oc,
        _ => return Vec::new(),
    };
    payload
        .skill_resources
        .iter()
        .filter_map(|id| id.strip_prefix("openclaw_skill_"))
        .map(str::to_string)
        .collect()
}

/// Count config resources in a claim's OpenClaw payload.
fn claim_config_count(claim: &AdapterClaim) -> usize {
    match &claim.driver_payload {
        DriverPayload::OpenClaw(oc) => oc.config_resources.len(),
        _ => 0,
    }
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
    fn list_contains_plugin_strips_ansi() {
        let ansi_output = "\x1b[1m\x1b[32magent-sec\x1b[0m\nother\n";
        assert!(list_contains_plugin(ansi_output, "agent-sec"));
        assert!(!list_contains_plugin(ansi_output, "not-here"));
    }

    #[test]
    fn list_contains_plugin_rich_table_no_wrap() {
        let table = "\
┏━━━━━━━━━━━━━━━━━━━━┳━━━━━━━━━━━━━┳━━━━━━━━━┓
┃ Name               ┃ ID          ┃ Status  ┃
┡━━━━━━━━━━━━━━━━━━━━╇━━━━━━━━━━━━━╇━━━━━━━━━┩
│ Agent Security     │ agent-sec   │ enabled │
└────────────────────┴─────────────┴─────────┘
";
        assert!(list_contains_plugin(table, "agent-sec"));
        assert!(!list_contains_plugin(table, "not-here"));
        assert!(!list_contains_plugin(table, "agent-sec-extra"));
    }

    #[test]
    fn list_contains_plugin_rich_table_wrapped() {
        let table = "\
┏━━━━━━━━━━━━━━━━━┳━━━━━━━━━━━━━━━━━━━┳━━━━━━━━━┓
┃ Name            ┃ ID                ┃ Status  ┃
┡━━━━━━━━━━━━━━━━━╇━━━━━━━━━━━━━━━━━━━╇━━━━━━━━━┩
│ Agent Security  │ agent-sec-core-op │ enabled │
│                 │ enclaw-plugin     │         │
└─────────────────┴───────────────────┴─────────┘
";
        assert!(list_contains_plugin(
            table,
            "agent-sec-core-openclaw-plugin"
        ));
        assert!(!list_contains_plugin(table, "agent-sec"));
    }

    #[test]
    fn list_contains_plugin_rich_table_ansi_wrapped() {
        let table = "\
\x1b[1m┏━━━━━━━━━━━━━━━━━┳━━━━━━━━━━━━━━━━━━━┳━━━━━━━━━┓\x1b[0m
\x1b[1m┃\x1b[0m Name            \x1b[1m┃\x1b[0m ID                \x1b[1m┃\x1b[0m Status  \x1b[1m┃\x1b[0m
\x1b[1m┡━━━━━━━━━━━━━━━━━╇━━━━━━━━━━━━━━━━━━━╇━━━━━━━━━┩\x1b[0m
│ Agent Security  │ agent-sec-core-op │ enabled │
│                 │ enclaw-plugin     │         │
\x1b[1m└─────────────────┴───────────────────┴─────────┘\x1b[0m
";
        assert!(list_contains_plugin(
            table,
            "agent-sec-core-openclaw-plugin"
        ));
    }

    #[test]
    fn strip_ansi_removes_sgr_and_osc() {
        assert_eq!(strip_ansi("\x1b[1mbold\x1b[0m"), "bold");
        assert_eq!(strip_ansi("\x1b[32mgreen\x1b[0m text"), "green text");
        assert_eq!(strip_ansi("no escapes here"), "no escapes here");
    }

    #[test]
    fn is_border_line_identifies_borders() {
        assert!(is_border_line("┏━━━━━━━━━━━━━━━━━┳━━━━━━━━━┓"));
        assert!(is_border_line("├──────┼──────────┤"));
        assert!(is_border_line("└──────┴──────────┘"));
        assert!(!is_border_line("│ agent-sec │ enabled │"));
        assert!(!is_border_line("plain text"));
        assert!(!is_border_line(""));
    }

    #[test]
    fn extract_cells_splits_data_line() {
        let cells = extract_cells("│ agent-sec   │ enabled │").unwrap();
        assert_eq!(cells, vec!["agent-sec", "enabled"]);
    }

    #[test]
    fn extract_cells_returns_none_for_plain_text() {
        assert!(extract_cells("plain text").is_none());
    }

    #[test]
    fn list_contains_plugin_rich_table_name_and_id_both_wrap() {
        let table = "\
┏━━━━━━━━━━━━━━━━━┳━━━━━━━━━━━━━━━━━━━┳━━━━━━━━━┓
┃ Name            ┃ ID                ┃ Status  ┃
┡━━━━━━━━━━━━━━━━━╇━━━━━━━━━━━━━━━━━━━╇━━━━━━━━━┩
│ Agent Security  │ agent-sec-core-op │ enabled │
│ Core Plugin     │ enclaw-plugin     │         │
└─────────────────┴───────────────────┴─────────┘
";
        assert!(list_contains_plugin(
            table,
            "agent-sec-core-openclaw-plugin"
        ));
        assert!(!list_contains_plugin(table, "agent-sec-core-op"));
    }

    #[test]
    fn list_contains_plugin_no_false_positive_across_rows() {
        let table = "\
┏━━━━━━━━━━━━━━━━━┳━━━━━━━━━━━━━━━━━━━┳━━━━━━━━━┓
┃ Name            ┃ ID                ┃ Status  ┃
┡━━━━━━━━━━━━━━━━━╇━━━━━━━━━━━━━━━━━━━╇━━━━━━━━━┩
│ Plugin A        │ agent-sec-core-op │ enabled │
│ Plugin B        │ enclaw-plugin     │ enabled │
└─────────────────┴───────────────────┴─────────┘
";
        assert!(
            !list_contains_plugin(table, "agent-sec-core-openclaw-plugin"),
            "must not merge IDs from independent rows into a false match"
        );
        assert!(list_contains_plugin(table, "agent-sec-core-op"));
        assert!(list_contains_plugin(table, "enclaw-plugin"));
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
            read_plugin_manifest_id(dir.path(), "openclaw.plugin.json").expect("read"),
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

    // -- config set cmd -------------------------------------------------

    #[test]
    fn config_set_cmd_string_value() {
        let cmd = build_config_set_cmd(
            "plugins.entries.sec.enabled",
            &toml::Value::String("true".to_string()),
            Path::new("/home/u/.openclaw"),
            Some(Path::new("/home/u")),
        );
        assert_eq!(cmd.program, "openclaw");
        assert_eq!(
            cmd.args,
            vec!["config", "set", "plugins.entries.sec.enabled", "true"]
        );
    }

    #[test]
    fn config_set_cmd_boolean_value() {
        let cmd = build_config_set_cmd(
            "debug.enabled",
            &toml::Value::Boolean(true),
            Path::new("/home/u/.openclaw"),
            Some(Path::new("/home/u")),
        );
        assert_eq!(cmd.args, vec!["config", "set", "debug.enabled", "true"]);
    }

    #[test]
    fn config_set_cmd_integer_value() {
        let cmd = build_config_set_cmd(
            "limits.max_plugins",
            &toml::Value::Integer(42),
            Path::new("/home/u/.openclaw"),
            Some(Path::new("/home/u")),
        );
        assert_eq!(cmd.args, vec!["config", "set", "limits.max_plugins", "42"]);
    }

    #[test]
    fn config_value_to_cli_string_covers_types() {
        assert_eq!(
            config_value_to_cli_string(&toml::Value::String("hello".into())),
            "hello"
        );
        assert_eq!(config_value_to_cli_string(&toml::Value::Integer(7)), "7");
        assert_eq!(config_value_to_cli_string(&toml::Value::Float(2.5)), "2.5");
        assert_eq!(
            config_value_to_cli_string(&toml::Value::Boolean(false)),
            "false"
        );
    }

    // -- claim_skill_resources / claim_config_count ---------------------

    #[test]
    fn claim_skill_resources_extracts_names() {
        let claim = AdapterClaim {
            claim_schema: CLAIM_SCHEMA_VERSION,
            component: "test".to_string(),
            framework: "openclaw".to_string(),
            plugin_id: None,
            adapter_type: None,
            enabled_at: "2026-01-01T00:00:00Z".to_string(),
            resource_root: PathBuf::from("/tmp"),
            bundle_digest: None,
            driver_schema: DRIVER_SCHEMA_VERSION,
            status: ClaimStatus::Enabled,
            resources: Vec::new(),
            driver_payload: DriverPayload::OpenClaw(OpenClawClaim {
                state_dir_resource: "s".to_string(),
                plugin_resource: "p".to_string(),
                skill_resources: vec![
                    "openclaw_skill_sec-audit".to_string(),
                    "openclaw_skill_cred-scan".to_string(),
                ],
                config_resources: vec!["openclaw_config_0".to_string()],
            }),
        };
        let skills = claim_skill_resources(&claim);
        assert_eq!(skills, vec!["sec-audit", "cred-scan"]);
        assert_eq!(claim_config_count(&claim), 1);
    }

    #[test]
    fn skill_bundle_plan_and_claim_skip_plugin_registration() {
        use crate::adapter::driver::{AdapterOps, CliOutput, DeclaredSkill};

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

        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("marker"), b"x").expect("write");
        let layout = anolisa_platform::fs_layout::FsLayout::user(PathBuf::from("/tmp/test-home"));
        let ops = StubOps;
        let ctx = DriverCtx {
            component: "os-skills".to_string(),
            framework: "openclaw".to_string(),
            layout: &layout,
            resource_root: dir.path().to_path_buf(),
            user_home: Some(PathBuf::from("/tmp/test-home")),
            declared_plugin_id: None,
            adapter_type: Some("skill_bundle".to_string()),
            declared_skills: vec![DeclaredSkill {
                name: "install-openclaw".to_string(),
                source: Some(PathBuf::from("/usr/share/anolisa/skills/install-openclaw")),
            }],
            declared_config: Vec::new(),
            declared_bundle_entry: None,
            dry_run: true,
            ops: &ops,
        };
        let driver = OpenClawDriver::new();
        let bundle = driver.read_bundle(&ctx).expect("read bundle");
        assert!(bundle.plugin_id.is_none());

        let plan = driver.plan_enable(&bundle, &ctx).expect("plan");
        assert!(plan.register_command.is_none());
        assert!(
            plan.actions
                .iter()
                .all(|action| !action.contains("register openclaw plugin")),
        );

        let claim = driver.prepare_enable(&bundle, &ctx).expect("claim");
        assert!(claim.plugin_id.is_none());
        assert_eq!(claim.adapter_type.as_deref(), Some("skill_bundle"));
        assert!(
            claim.resources.iter().all(|resource| !matches!(
                resource.kind,
                ClaimResourceKind::FrameworkPlugin { .. }
            )),
        );
        assert_eq!(claim_skill_resources(&claim), vec!["install-openclaw"]);

        let previous_bin = std::env::var_os("OPENCLAW_BIN");
        // SAFETY: test-only; restored before the test returns.
        unsafe {
            std::env::set_var("OPENCLAW_BIN", "/bin/sh");
        }
        let report = driver.status(&claim, &ctx).expect("status");
        // SAFETY: test-only; restore the process environment.
        unsafe {
            if let Some(value) = previous_bin {
                std::env::set_var("OPENCLAW_BIN", value);
            } else {
                std::env::remove_var("OPENCLAW_BIN");
            }
        }

        assert_eq!(report.summary, AdapterSummary::Healthy);
        assert!(
            report
                .conditions
                .iter()
                .all(|condition| condition.kind != AdapterConditionKind::PluginRegistered),
            "skill_bundle status must not require plugin registration"
        );
        assert!(report.conditions.iter().any(|condition| {
            condition.kind == AdapterConditionKind::VerificationSupported
                && condition.status == ConditionStatus::True
        }));
    }
}
