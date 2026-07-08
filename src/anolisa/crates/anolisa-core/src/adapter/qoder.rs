//! Qoder (`qodercli`) framework driver.
//!
//! Qoder CLI installs a plugin from a directory whose name becomes the
//! plugin id (`qodercli plugins install <dir>`), then activates it through
//! two entries in `~/.qoder/settings.json`: the plugin's hooks under
//! `.hooks`, and `<plugin>@local` under `.plugins.enabled`. ANOLISA
//! reproduces the legacy install script's behavior entirely in this driver
//! — it never shells out to `scripts/install.sh` / `uninstall.sh`:
//!
//! ```text
//! qodercli plugins install <staging>/<plugin>   # staging = <data>/qoder-plugins
//! # merge our hooks + <plugin>@local into ~/.qoder/settings.json in place
//! ```
//!
//! The plugin bundle lives under a resource directory named `qoder`, but
//! `qodercli` derives the plugin id from the *directory name*, so enable
//! stages a symlink named after the plugin id (`tokenless`) pointing at the
//! resource root and installs from there — mirroring the legacy script's
//! private tempdir. The symlink is install-time only (qodercli copies the
//! plugin into its own cache) and is removed immediately after install.
//!
//! **settings.json is edited in place, never rewritten wholesale.** All
//! reads and writes go through the Manager's controlled
//! [`AdapterOps`](super::driver::AdapterOps); the driver only ever adds or
//! removes ANOLISA-managed entries (hooks whose name starts with
//! `<plugin>-`, and the `<plugin>@local` plugin entry). A settings file that
//! exists but cannot be parsed is left untouched: enable fails closed and
//! disable reports cleanup incomplete, so ANOLISA never clobbers a config it
//! cannot safely merge.
//!
//! `qodercli plugins list` has been observed to omit freshly installed
//! plugins, so `status` does **not** trust it: plugin registration is
//! reported `Unknown` rather than faked healthy. The reliable, CLI-free
//! signal is the presence of our managed entries in `settings.json`, which
//! `status` verifies directly.
//!
//! Env contract: `QODERCLI_BIN` overrides the executable (tests point it at
//! a fake CLI); otherwise the binary is resolved in the legacy order
//! (highest-versioned `~/.qoder/bin/qodercli/qodercli-*`, then the
//! unversioned binary there, then `qodercli` on `PATH`). `XDG_DATA_HOME`
//! relocates the plugin staging base.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};

use super::AdapterError;
use super::claim::{
    AdapterClaim, CLAIM_SCHEMA_VERSION, ClaimResource, ClaimResourceKind, ClaimStatus,
    DRIVER_SCHEMA_VERSION, DriverPayload, QoderClaim, validate_plugin_id,
};
use super::driver::{
    AdapterBundle, AdapterCondition, AdapterConditionKind, AdapterStatusReport, AdapterSummary,
    ClaimResourceRef, ConditionStatus, DetectResult, DisableReport, DriverCtx, DriverPlan,
    FrameworkCommand, FrameworkDriver, HostEnv, find_binary_in_path,
};
use super::util::{bool_status, cli_failure_reason, digest_tree, display_command, now_iso8601};

/// Default timeout for a `qodercli` invocation.
const CLI_TIMEOUT: Duration = Duration::from_secs(60);

/// Qoder-native plugin manifest inside the bundle. The contract may override
/// the entry, but this is the default the legacy layout ships.
const QODER_PLUGIN_MANIFEST: &str = ".qoder-plugin/plugin.json";

/// Hook declarations shipped alongside the plugin manifest, merged into the
/// user's `settings.json` at enable time.
const QODER_HOOKS_FILE: &str = "hooks.json";

/// Placeholder in `hooks.json` for the absolute hook-scripts directory,
/// expanded to `<resource_root>/../common/hooks` before the entries are
/// written into `settings.json` (matching the legacy install script).
const HOOKS_PLACEHOLDER: &str = "${QODER_TOKENLESS_HOOKS}";

/// Resource ids used in Qoder receipts.
const RES_PLUGIN: &str = "qoder_plugin";
const RES_SETTINGS: &str = "qoder_settings";

/// Qoder driver. Stateless; all per-operation context arrives via
/// [`DriverCtx`].
pub struct QoderDriver;

impl QoderDriver {
    /// Construct the driver.
    pub fn new() -> Self {
        Self
    }
}

impl Default for QoderDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkDriver for QoderDriver {
    fn name(&self) -> &'static str {
        "qoder"
    }

    fn detect(&self, env: &HostEnv) -> DetectResult {
        match resolve_qodercli(env.user_home.as_deref()) {
            Some(path) => DetectResult {
                detected: true,
                reason: format!("qodercli found at {}", path.display()),
            },
            None => DetectResult {
                detected: false,
                reason: "qodercli not found (checked $QODERCLI_BIN, ~/.qoder/bin/qodercli, PATH)"
                    .to_string(),
            },
        }
    }

    fn allowed_external_roots(&self, ctx: &DriverCtx) -> Vec<PathBuf> {
        // Two external roots: the user's `~/.qoder` (where settings.json
        // lives) and ANOLISA's own plugin-staging namespace under the data
        // home (where the install-time symlink is created). Neither is
        // derived from receipt contents. `~/.ssh`, `/etc`, etc. fall outside
        // both, so a forged receipt cannot redirect a write there.
        let mut roots = Vec::new();
        if let Some(home) = ctx.user_home.as_deref() {
            roots.push(qoder_home(home));
        }
        if let Some(staging) = plugin_staging_root(ctx.user_home.as_deref()) {
            roots.push(staging);
        }
        roots
    }

    fn read_bundle(&self, ctx: &DriverCtx) -> Result<AdapterBundle, AdapterError> {
        let root = &ctx.resource_root;
        if !root.is_dir() {
            return Err(AdapterError::BundleInvalid {
                root: root.clone(),
                reason: "resource root does not exist or is not a directory".to_string(),
            });
        }
        let manifest = ctx
            .declared_bundle_entry
            .as_deref()
            .unwrap_or(QODER_PLUGIN_MANIFEST);
        if !root.join(manifest).is_file() {
            return Err(AdapterError::BundleInvalid {
                root: root.clone(),
                reason: format!(
                    "qoder plugin manifest '{manifest}' missing (run: make stamp-adapter-templates)"
                ),
            });
        }
        if !root.join(QODER_HOOKS_FILE).is_file() {
            return Err(AdapterError::BundleInvalid {
                root: root.clone(),
                reason: format!("qoder '{QODER_HOOKS_FILE}' missing from resource root"),
            });
        }
        let plugin_id = ctx
            .declared_plugin_id
            .clone()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| ctx.component.clone());
        // Validate the resolved plugin id (including the component-name
        // default) before it can reach an argv or a staging directory name.
        validate_plugin_id(&plugin_id)?;
        Ok(AdapterBundle {
            resource_root: root.clone(),
            digest: digest_tree(root),
            plugin_id: Some(plugin_id),
        })
    }

    fn plan_enable(
        &self,
        bundle: &AdapterBundle,
        ctx: &DriverCtx,
    ) -> Result<DriverPlan, AdapterError> {
        let plugin = plugin_name(bundle, ctx);
        let program =
            qodercli_program(ctx.user_home.as_deref()).unwrap_or_else(|| "qodercli".to_string());
        let staging = staging_symlink(ctx.user_home.as_deref(), &plugin);
        let staging_display = staging
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| format!("<staging>/{plugin}"));
        let settings_display = settings_path(ctx.user_home.as_deref())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~/.qoder/settings.json".to_string());
        let install_cmd = build_install_cmd(
            &program,
            staging.as_deref().unwrap_or_else(|| Path::new("<staging>")),
        );
        let actions = vec![
            format!(
                "stage qoder plugin dir {staging_display} -> {}",
                bundle.resource_root.display()
            ),
            format!("register qoder plugin '{plugin}' via `qodercli plugins install`"),
            format!("merge tokenless hooks into {settings_display}"),
            format!(
                "enable plugin '{}' in qoder settings",
                plugin_entry(&plugin)
            ),
        ];
        Ok(DriverPlan {
            framework: self.name().to_string(),
            component: ctx.component.clone(),
            actions,
            register_command: Some(display_command(&install_cmd)),
        })
    }

    fn prepare_enable(
        &self,
        bundle: &AdapterBundle,
        ctx: &DriverCtx,
    ) -> Result<AdapterClaim, AdapterError> {
        let plugin = plugin_name(bundle, ctx);
        validate_plugin_id(&plugin)?;
        let settings =
            settings_path(ctx.user_home.as_deref()).ok_or_else(|| AdapterError::FrameworkCli {
                program: "qodercli".to_string(),
                reason: "cannot resolve ~/.qoder/settings.json (no home directory)".to_string(),
            })?;
        // Record the hook names we will merge so status can require each one
        // present later without re-reading the bundle.
        let managed_hooks = collect_expected_hook_names(&bundle.resource_root)?;

        let resources = vec![
            ClaimResource {
                id: RES_PLUGIN.to_string(),
                purpose: "qoder_plugin".to_string(),
                kind: ClaimResourceKind::FrameworkPlugin {
                    framework: self.name().to_string(),
                    plugin_id: plugin.clone(),
                },
            },
            ClaimResource {
                id: RES_SETTINGS.to_string(),
                purpose: "qoder_settings".to_string(),
                kind: ClaimResourceKind::ExternalPath { path: settings },
            },
        ];

        Ok(AdapterClaim {
            claim_schema: CLAIM_SCHEMA_VERSION,
            component: ctx.component.clone(),
            framework: self.name().to_string(),
            plugin_id: Some(plugin),
            adapter_type: ctx.adapter_type.clone(),
            enabled_at: now_iso8601(),
            resource_root: bundle.resource_root.clone(),
            bundle_digest: bundle.digest.clone(),
            driver_schema: DRIVER_SCHEMA_VERSION,
            status: ClaimStatus::Enabled,
            resources,
            driver_payload: DriverPayload::Qoder(QoderClaim {
                plugin_resource: RES_PLUGIN.to_string(),
                settings_resource: RES_SETTINGS.to_string(),
                managed_hooks,
            }),
        })
    }

    fn apply_enable(&self, claim: &AdapterClaim, ctx: &DriverCtx) -> Result<(), AdapterError> {
        // Resolve plugin + settings strictly from the receipt's payload
        // references (Manager-validated), failing closed on a malformed
        // receipt rather than falling back to ctx-derived defaults.
        let plugin = resolve_plugin(claim).ok_or_else(|| AdapterError::BundleInvalid {
            root: claim.resource_root.clone(),
            reason: "qoder receipt has no plugin resource".to_string(),
        })?;
        let settings = resolve_settings(claim, ctx.user_home.as_deref()).ok_or_else(|| {
            AdapterError::BundleInvalid {
                root: claim.resource_root.clone(),
                reason: "qoder receipt settings resource is missing or not ~/.qoder/settings.json"
                    .to_string(),
            }
        })?;
        let program = qodercli_program(ctx.user_home.as_deref()).ok_or_else(|| {
            AdapterError::FrameworkCli {
                program: "qodercli".to_string(),
                reason: "qodercli not found on PATH or under ~/.qoder/bin".to_string(),
            }
        })?;
        let staging = staging_symlink(ctx.user_home.as_deref(), &plugin).ok_or_else(|| {
            AdapterError::FrameworkCli {
                program: program.clone(),
                reason: "cannot resolve qoder plugin staging dir (no home / XDG_DATA_HOME)"
                    .to_string(),
            }
        })?;

        // 1. Stage a directory named after the plugin id (qodercli derives
        //    the id from the dir name) and install from it. The staging
        //    symlink is install-time only — remove it whether install
        //    succeeds or not.
        ctx.ops.create_symlink(&staging, &claim.resource_root)?;
        let install_cmd = build_install_cmd(&program, &staging);
        let cli_program = install_cmd.program.clone();
        let install = ctx.ops.run_framework_cli(install_cmd);
        let _ = ctx.ops.remove_tree(&staging);
        let output = install?;
        if !output.success() {
            return Err(AdapterError::FrameworkCli {
                program: cli_program,
                reason: cli_failure_reason("plugins install", &output),
            });
        }

        // 2. Merge our hooks + `<plugin>@local` into settings.json in place.
        let resolved = load_resolved_hooks(&claim.resource_root)?;
        let existing = ctx.ops.read_file(&settings)?;
        let mut root = load_settings_for_merge(existing, &settings)?;
        merge_managed(&mut root, &resolved, &plugin_entry(&plugin));
        let bytes = serde_json::to_vec_pretty(&Value::Object(root)).map_err(|source| {
            AdapterError::SettingsUnparseable {
                path: settings.clone(),
                reason: format!("failed to render merged settings JSON: {source}"),
            }
        })?;
        ctx.ops.write_file(&settings, &bytes)?;
        Ok(())
    }

    fn status(
        &self,
        claim: &AdapterClaim,
        ctx: &DriverCtx,
    ) -> Result<AdapterStatusReport, AdapterError> {
        let mut conditions = Vec::new();
        let detect = self.detect(&HostEnv {
            user_home: ctx.user_home.clone(),
        });
        conditions.push(AdapterCondition {
            kind: AdapterConditionKind::FrameworkDetected,
            status: bool_status(detect.detected),
            reason: Some(detect.reason.clone()),
            resource: None,
        });
        conditions.push(bundle_match_condition(claim));

        // Resolve strictly from the receipt payload; a receipt missing its
        // plugin or settings resource is malformed and must not be treated as
        // healthy or verifiable.
        let (Some(plugin), Some(settings)) = (
            resolve_plugin(claim),
            resolve_settings(claim, ctx.user_home.as_deref()),
        ) else {
            conditions.push(AdapterCondition {
                kind: AdapterConditionKind::JsonKeysPresent,
                status: ConditionStatus::False,
                reason: Some("receipt missing plugin or settings resource".to_string()),
                resource: Some(ClaimResourceRef {
                    id: RES_SETTINGS.to_string(),
                }),
            });
            conditions.push(AdapterCondition {
                kind: AdapterConditionKind::PluginRegistered,
                status: ConditionStatus::Unknown,
                reason: Some("receipt missing plugin resource".to_string()),
                resource: Some(ClaimResourceRef {
                    id: RES_PLUGIN.to_string(),
                }),
            });
            conditions.push(AdapterCondition {
                kind: AdapterConditionKind::VerificationSupported,
                status: ConditionStatus::False,
                reason: Some("receipt missing required resources".to_string()),
                resource: None,
            });
            return Ok(AdapterStatusReport {
                summary: summarize(claim.status, detect.detected, ConditionStatus::False),
                conditions,
            });
        };
        let probe = probe_settings(ctx, &settings, &plugin, managed_hook_names(claim));
        let (settings_status, settings_reason) = match probe {
            SettingsProbe::Present {
                hooks_present: true,
                plugin_enabled: true,
            } => (ConditionStatus::True, None),
            SettingsProbe::Present {
                hooks_present,
                plugin_enabled,
            } => {
                let mut missing: Vec<String> = Vec::new();
                if !hooks_present {
                    missing.push(format!("'{}' hooks", hook_prefix(&plugin)));
                }
                if !plugin_enabled {
                    missing.push(format!("'{}'", plugin_entry(&plugin)));
                }
                (
                    ConditionStatus::False,
                    Some(format!("settings.json missing {}", missing.join(" and "))),
                )
            }
            SettingsProbe::Absent => (
                ConditionStatus::False,
                Some("~/.qoder/settings.json absent".to_string()),
            ),
            SettingsProbe::Unverifiable => (
                ConditionStatus::Unknown,
                Some("~/.qoder/settings.json unreadable or unparseable".to_string()),
            ),
        };
        conditions.push(AdapterCondition {
            kind: AdapterConditionKind::JsonKeysPresent,
            status: settings_status,
            reason: settings_reason,
            resource: Some(ClaimResourceRef {
                id: RES_SETTINGS.to_string(),
            }),
        });

        // `qodercli plugins list` omits freshly installed plugins, so never
        // report registration as verified — leave it Unknown rather than
        // faking Healthy off an unreliable probe.
        conditions.push(AdapterCondition {
            kind: AdapterConditionKind::PluginRegistered,
            status: ConditionStatus::Unknown,
            reason: Some(
                "qodercli plugins list is unreliable; verified via settings.json instead"
                    .to_string(),
            ),
            resource: Some(ClaimResourceRef {
                id: RES_PLUGIN.to_string(),
            }),
        });
        // Settings-based verification does not need the CLI, so it is always
        // supported even when qodercli is absent.
        conditions.push(AdapterCondition {
            kind: AdapterConditionKind::VerificationSupported,
            status: ConditionStatus::True,
            reason: None,
            resource: None,
        });

        let summary = summarize(claim.status, detect.detected, settings_status);
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
        // Framework-side deregistration needs the CLI. Without it, the plugin
        // would stay in qodercli's cache, so keep the receipt for a retry
        // rather than pruning settings and pretending cleanup finished.
        let Some(cli) = resolve_qodercli(ctx.user_home.as_deref()) else {
            return Ok(DisableReport {
                cleanup_complete: false,
                messages: vec![
                    "qodercli not found on PATH or under ~/.qoder/bin; receipt kept for retry"
                        .to_string(),
                ],
            });
        };
        let program = cli.to_string_lossy().into_owned();

        // Fail closed: act only on resources the receipt actually declares.
        // A malformed/forged receipt missing the plugin or settings resource
        // must not drive `plugins uninstall` or a settings write against a
        // ctx-derived default — keep the receipt for manual resolution.
        let Some(plugin) = resolve_plugin(claim) else {
            return Ok(DisableReport {
                cleanup_complete: false,
                messages: vec![
                    "qoder receipt has no plugin resource; receipt kept (nothing safely removable)"
                        .to_string(),
                ],
            });
        };
        let Some(settings) = resolve_settings(claim, ctx.user_home.as_deref()) else {
            return Ok(DisableReport {
                cleanup_complete: false,
                messages: vec![
                    "qoder receipt settings resource is missing or not ~/.qoder/settings.json; \
                     receipt kept (nothing safely removable)"
                        .to_string(),
                ],
            });
        };

        let mut messages = Vec::new();

        // 1. Unregister the plugin. An already-removed plugin exits non-zero,
        //    so treat a CLI failure as clean only when the plugin cache is
        //    confirmed gone; otherwise cleanup is incomplete.
        let out = ctx
            .ops
            .run_framework_cli(build_uninstall_cmd(&program, &plugin))?;
        let plugin_ok = if out.success() {
            messages.push(format!("uninstalled qoder plugin '{plugin}'"));
            true
        } else if !plugin_cache_present(ctx.user_home.as_deref(), &plugin) {
            messages.push(format!("qoder plugin '{plugin}' already absent"));
            true
        } else {
            messages.push(format!(
                "qodercli plugins uninstall failed and plugin still cached: {}",
                cli_failure_reason("plugins uninstall", &out)
            ));
            false
        };

        // 2. Prune only ANOLISA-managed entries from settings.json.
        let settings_ok = prune_settings_via_ops(ctx, &settings, &plugin, &mut messages);

        Ok(DisableReport {
            cleanup_complete: plugin_ok && settings_ok,
            messages,
        })
    }
}

// ---------------------------------------------------------------------------
// Settings verification / mutation
// ---------------------------------------------------------------------------

/// Outcome of reading and inspecting `settings.json` for `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsProbe {
    /// The file exists and parses; whether our two managed entries are set.
    Present {
        hooks_present: bool,
        plugin_enabled: bool,
    },
    /// The file does not exist.
    Absent,
    /// The file cannot be read or parsed — do not claim absence.
    Unverifiable,
}

/// Read `settings` (through the controlled ops) and report whether *all*
/// `expected_hooks` and the `<plugin>@local` entry are present. `settings` is
/// the Manager-validated path from the receipt's settings resource.
fn probe_settings(
    ctx: &DriverCtx,
    settings: &Path,
    plugin: &str,
    expected_hooks: &[String],
) -> SettingsProbe {
    match ctx.ops.read_file(settings) {
        Ok(None) => SettingsProbe::Absent,
        Ok(Some(bytes)) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(root)) => {
                let (hooks_present, plugin_enabled) = settings_managed_present(
                    &root,
                    expected_hooks,
                    &hook_prefix(plugin),
                    &plugin_entry(plugin),
                );
                SettingsProbe::Present {
                    hooks_present,
                    plugin_enabled,
                }
            }
            // Parses but is not an object: our entries cannot be present.
            Ok(_) => SettingsProbe::Present {
                hooks_present: false,
                plugin_enabled: false,
            },
            Err(_) => SettingsProbe::Unverifiable,
        },
        Err(_) => SettingsProbe::Unverifiable,
    }
}

/// Prune managed entries from settings.json through the controlled ops.
/// Returns whether cleanup of the settings side succeeded (true also when
/// there was nothing to remove). Never rewrites an unparseable file.
fn prune_settings_via_ops(
    ctx: &DriverCtx,
    settings: &Path,
    plugin: &str,
    messages: &mut Vec<String>,
) -> bool {
    let bytes = match ctx.ops.read_file(settings) {
        Ok(None) => {
            messages.push("~/.qoder/settings.json absent; nothing to prune".to_string());
            return true;
        }
        Ok(Some(bytes)) => bytes,
        Err(err) => {
            messages.push(format!("failed to read settings.json: {err}"));
            return false;
        }
    };
    let mut root = match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(root)) => root,
        // A non-object settings file has none of our entries; leave it.
        Ok(_) => {
            messages.push("settings.json is not a JSON object; left untouched".to_string());
            return true;
        }
        Err(err) => {
            // Fail closed: never overwrite a file we cannot parse.
            messages.push(format!("settings.json unparseable; left untouched: {err}"));
            return false;
        }
    };
    if !prune_managed(&mut root, &hook_prefix(plugin), &plugin_entry(plugin)) {
        messages.push("settings.json already free of tokenless entries".to_string());
        return true;
    }
    let out = match serde_json::to_vec_pretty(&Value::Object(root)) {
        Ok(out) => out,
        Err(err) => {
            messages.push(format!("failed to render pruned settings JSON: {err}"));
            return false;
        }
    };
    match ctx.ops.write_file(settings, &out) {
        Ok(()) => {
            messages.push("pruned tokenless entries from settings.json".to_string());
            true
        }
        Err(err) => {
            messages.push(format!("failed to write settings.json: {err}"));
            false
        }
    }
}

/// Parse settings bytes for a merge: `None` (absent) or a non-object file
/// both start from an empty object; a parse error fails closed.
fn load_settings_for_merge(
    existing: Option<Vec<u8>>,
    path: &Path,
) -> Result<Map<String, Value>, AdapterError> {
    match existing {
        None => Ok(Map::new()),
        Some(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(root)) => Ok(root),
            // Mirror the legacy script: a non-dict settings.json is replaced
            // by a fresh object holding only our managed entries.
            Ok(_) => Ok(Map::new()),
            Err(source) => Err(AdapterError::SettingsUnparseable {
                path: path.to_path_buf(),
                reason: source.to_string(),
            }),
        },
    }
}

/// Read `hooks.json` from the bundle, expand [`HOOKS_PLACEHOLDER`] to the
/// absolute hook-scripts directory, and parse the result.
///
/// # Errors
///
/// [`AdapterError::Io`] when the file cannot be read;
/// [`AdapterError::BundleInvalid`] when it is not valid JSON.
fn load_resolved_hooks(resource_root: &Path) -> Result<Value, AdapterError> {
    let path = resource_root.join(QODER_HOOKS_FILE);
    let bytes = std::fs::read(&path).map_err(|source| AdapterError::Io {
        path: path.clone(),
        source,
    })?;
    let hooks_dir = common_hooks_dir(resource_root);
    let text = String::from_utf8_lossy(&bytes);
    let substituted = text.replace(HOOKS_PLACEHOLDER, &hooks_dir.to_string_lossy());
    serde_json::from_str(&substituted).map_err(|source| AdapterError::BundleInvalid {
        root: resource_root.to_path_buf(),
        reason: format!("failed to parse {QODER_HOOKS_FILE}: {source}"),
    })
}

/// Every hook name declared in the bundle's `hooks.json`, recorded in the
/// receipt so status can require each one present without re-reading the
/// bundle.
///
/// # Errors
///
/// Propagates [`load_resolved_hooks`] read/parse failures.
fn collect_expected_hook_names(resource_root: &Path) -> Result<Vec<String>, AdapterError> {
    let resolved = load_resolved_hooks(resource_root)?;
    let mut names = Vec::new();
    if let Some(hooks) = resolved.get("hooks").and_then(Value::as_object) {
        for entries in hooks.values() {
            if let Some(arr) = entries.as_array() {
                for entry in arr {
                    if let Some(hs) = entry.get("hooks").and_then(Value::as_array) {
                        for h in hs {
                            if let Some(name) = h.get("name").and_then(Value::as_str) {
                                names.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(names)
}

/// Add our hooks (deduped by hook name) and `<plugin>@local` to a settings
/// object, preserving every other key. The counterpart of [`prune_managed`].
fn merge_managed(root: &mut Map<String, Value>, resolved: &Value, plugin_entry: &str) {
    if let Some(resolved_hooks) = resolved.get("hooks").and_then(Value::as_object) {
        let hooks_slot = root
            .entry("hooks")
            .or_insert_with(|| Value::Object(Map::new()));
        let hooks = ensure_object(hooks_slot);
        for (event, entries) in resolved_hooks {
            let Some(entries) = entries.as_array() else {
                continue;
            };
            let slot = hooks
                .entry(event.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            let arr = ensure_array(slot);
            let existing_names = collect_hook_names(arr);
            for entry in entries {
                if let Some(name) = primary_hook_name(entry)
                    && !existing_names.contains(&name)
                {
                    arr.push(entry.clone());
                }
            }
        }
    }

    let plugins_slot = root
        .entry("plugins")
        .or_insert_with(|| Value::Object(Map::new()));
    let plugins = ensure_object(plugins_slot);
    let enabled_slot = plugins
        .entry("enabled")
        .or_insert_with(|| Value::Array(Vec::new()));
    let enabled = ensure_array(enabled_slot);
    if !enabled.iter().any(|v| v.as_str() == Some(plugin_entry)) {
        enabled.push(Value::String(plugin_entry.to_string()));
    }
}

/// Remove ANOLISA-managed entries (hooks whose name starts with `hook_prefix`,
/// and the `plugin_entry` in `plugins.enabled`), collapsing emptied
/// containers. Returns whether anything changed. User entries are untouched;
/// hooks are matched by *name prefix*, never by command substring, so a user
/// hook whose command merely mentions the plugin survives.
fn prune_managed(root: &mut Map<String, Value>, hook_prefix: &str, plugin_entry: &str) -> bool {
    let mut removed = false;

    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        let events: Vec<String> = hooks.keys().cloned().collect();
        for event in events {
            if let Some(arr) = hooks.get_mut(&event).and_then(Value::as_array_mut) {
                let before = arr.len();
                arr.retain(|entry| !entry_is_owned(entry, hook_prefix));
                if arr.len() != before {
                    removed = true;
                }
                if arr.is_empty() {
                    hooks.remove(&event);
                    removed = true;
                }
            }
        }
        if hooks.is_empty() {
            root.remove("hooks");
        }
    }

    if let Some(plugins) = root.get_mut("plugins").and_then(Value::as_object_mut) {
        if let Some(enabled) = plugins.get_mut("enabled").and_then(Value::as_array_mut) {
            let before = enabled.len();
            enabled.retain(|v| v.as_str() != Some(plugin_entry));
            if enabled.len() != before {
                removed = true;
            }
            if enabled.is_empty() {
                plugins.remove("enabled");
            }
        }
        if plugins.is_empty() {
            root.remove("plugins");
        }
    }

    removed
}

/// Whether our managed hooks and plugin entry are present in a settings
/// object. Returns `(hooks present, plugin entry enabled)`.
///
/// When `expected_hooks` is non-empty, hooks are present only if **every**
/// recorded name is still there — so removing a single managed hook degrades
/// rather than staying healthy. When it is empty (a legacy receipt written
/// before hook names were recorded), fall back to "any hook whose name starts
/// with `hook_prefix`".
fn settings_managed_present(
    root: &Map<String, Value>,
    expected_hooks: &[String],
    hook_prefix: &str,
    plugin_entry: &str,
) -> (bool, bool) {
    let present = present_hook_names(root);
    let hooks_present = if expected_hooks.is_empty() {
        present.iter().any(|n| n.starts_with(hook_prefix))
    } else {
        expected_hooks.iter().all(|n| present.contains(n))
    };
    let plugin_enabled = root
        .get("plugins")
        .and_then(Value::as_object)
        .and_then(|p| p.get("enabled"))
        .and_then(Value::as_array)
        .map(|arr| arr.iter().any(|v| v.as_str() == Some(plugin_entry)))
        .unwrap_or(false);
    (hooks_present, plugin_enabled)
}

/// All hook names currently present across every event in a settings object.
fn present_hook_names(root: &Map<String, Value>) -> HashSet<String> {
    let mut names = HashSet::new();
    if let Some(hooks) = root.get("hooks").and_then(Value::as_object) {
        for entries in hooks.values() {
            if let Some(arr) = entries.as_array() {
                names.extend(collect_hook_names(arr));
            }
        }
    }
    names
}

/// True when any hook inside `entry` has a name starting with `prefix`.
fn entry_is_owned(entry: &Value, prefix: &str) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .map(|hooks| {
            hooks.iter().any(|h| {
                h.get("name")
                    .and_then(Value::as_str)
                    .map(|n| n.starts_with(prefix))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// First hook name inside a merge `entry`, used to dedup on re-enable.
fn primary_hook_name(entry: &Value) -> Option<String> {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .and_then(|hooks| {
            hooks
                .iter()
                .find_map(|h| h.get("name").and_then(Value::as_str).map(str::to_string))
        })
}

/// All hook names already present across `entries`.
fn collect_hook_names(entries: &[Value]) -> HashSet<String> {
    let mut names = HashSet::new();
    for entry in entries {
        if let Some(hooks) = entry.get("hooks").and_then(Value::as_array) {
            for h in hooks {
                if let Some(name) = h.get("name").and_then(Value::as_str) {
                    names.insert(name.to_string());
                }
            }
        }
    }
    names
}

/// Coerce `v` into an object in place, returning a mutable handle.
fn ensure_object(v: &mut Value) -> &mut Map<String, Value> {
    if !v.is_object() {
        *v = Value::Object(Map::new());
    }
    match v.as_object_mut() {
        Some(m) => m,
        // Unreachable: `v` was just set to an object above.
        None => unreachable!("value coerced to object cannot fail as_object_mut"),
    }
}

/// Coerce `v` into an array in place, returning a mutable handle.
fn ensure_array(v: &mut Value) -> &mut Vec<Value> {
    if !v.is_array() {
        *v = Value::Array(Vec::new());
    }
    match v.as_array_mut() {
        Some(a) => a,
        // Unreachable: `v` was just set to an array above.
        None => unreachable!("value coerced to array cannot fail as_array_mut"),
    }
}

// ---------------------------------------------------------------------------
// Pure path / identifier helpers
// ---------------------------------------------------------------------------

/// Plugin name for the receipt: the bundle's resolved id, else component.
fn plugin_name(bundle: &AdapterBundle, ctx: &DriverCtx) -> String {
    bundle
        .plugin_id
        .clone()
        .unwrap_or_else(|| ctx.component.clone())
}

/// Managed-hook name prefix for a plugin (`<plugin>-`).
fn hook_prefix(plugin: &str) -> String {
    format!("{plugin}-")
}

/// Managed plugin entry in `plugins.enabled` (`<plugin>@local`).
fn plugin_entry(plugin: &str) -> String {
    format!("{plugin}@local")
}

/// `<user_home>/.qoder`.
fn qoder_home(user_home: &Path) -> PathBuf {
    user_home.join(".qoder")
}

/// `<user_home>/.qoder/settings.json`, when a home directory is known.
fn settings_path(user_home: Option<&Path>) -> Option<PathBuf> {
    user_home.map(|h| qoder_home(h).join("settings.json"))
}

/// ANOLISA data-home base: `${XDG_DATA_HOME:-<home>/.local/share}/anolisa`.
/// Mirrors the Codex driver so both stage under the same namespace.
fn anolisa_data_base(user_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let s = xdg.to_string_lossy();
        let trimmed = s.trim_end_matches('/');
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed).join("anolisa"));
        }
    }
    user_home.map(|h| h.join(".local").join("share").join("anolisa"))
}

/// Plugin staging root: `<data base>/qoder-plugins`.
fn plugin_staging_root(user_home: Option<&Path>) -> Option<PathBuf> {
    anolisa_data_base(user_home).map(|base| base.join("qoder-plugins"))
}

/// Install-time staging symlink: `<staging root>/<plugin>`.
fn staging_symlink(user_home: Option<&Path>, plugin: &str) -> Option<PathBuf> {
    plugin_staging_root(user_home).map(|root| root.join(plugin))
}

/// Absolute hook-scripts directory the [`HOOKS_PLACEHOLDER`] resolves to:
/// `<resource_root>/../common/hooks` (the sibling `common` bundle).
fn common_hooks_dir(resource_root: &Path) -> PathBuf {
    resource_root
        .parent()
        .unwrap_or(resource_root)
        .join("common")
        .join("hooks")
}

/// Whether qodercli's plugin cache holds `<plugin>` (or the target-suffixed
/// `<plugin>-qoder` variant the legacy scripts also accept).
fn plugin_cache_present(user_home: Option<&Path>, plugin: &str) -> bool {
    let Some(home) = user_home else {
        return false;
    };
    let base = qoder_home(home).join("plugins").join("cache").join("local");
    base.join(plugin).is_dir() || base.join(format!("{plugin}-qoder")).is_dir()
}

/// The Qoder-specific payload of a receipt, when it is one.
fn qoder_payload(claim: &AdapterClaim) -> Option<&QoderClaim> {
    match &claim.driver_payload {
        DriverPayload::Qoder(q) => Some(q),
        _ => None,
    }
}

/// Resolve the plugin name strictly from the payload's `plugin_resource`
/// reference. Returns `None` (fail closed) when the payload is not Qoder's,
/// the referenced resource is missing, or it is not a `FrameworkPlugin`.
///
/// [`AdapterClaim::validate`] only checks the resources that *exist*, not
/// that payload references resolve, so a forged/malformed receipt can drop a
/// key resource yet still parse. Resolving strictly here — with no fallback
/// to `claim.plugin_id`/`ctx.component` — ensures such a receipt cannot drive
/// the CLI off an unvalidated name.
fn resolve_plugin(claim: &AdapterClaim) -> Option<String> {
    let payload = qoder_payload(claim)?;
    claim
        .resource(&payload.plugin_resource)
        .and_then(|r| match &r.kind {
            ClaimResourceKind::FrameworkPlugin { plugin_id, .. } => Some(plugin_id.clone()),
            _ => None,
        })
}

/// Resolve the settings path strictly from the payload's `settings_resource`
/// reference, requiring it to equal the canonical `~/.qoder/settings.json`
/// recomputed from `user_home`.
///
/// The Manager only validates the recorded `ExternalPath` against the
/// driver's allowed roots, and the driver's allowed root is the *whole*
/// `~/.qoder` — so root-level validation alone would let a forged receipt
/// redirect the write to another file under it (e.g.
/// `~/.qoder/other.json`). Pinning the path to exactly `settings.json`
/// closes that redirect: a mismatch returns `None` (fail closed), never the
/// recorded path. Returns `None` when the reference is missing, is not an
/// `ExternalPath`, or `user_home` is unknown.
fn resolve_settings(claim: &AdapterClaim, user_home: Option<&Path>) -> Option<PathBuf> {
    let payload = qoder_payload(claim)?;
    let recorded = claim
        .resource(&payload.settings_resource)
        .and_then(|r| match &r.kind {
            ClaimResourceKind::ExternalPath { path } => Some(path.clone()),
            _ => None,
        })?;
    let expected = settings_path(user_home)?;
    (recorded == expected).then_some(recorded)
}

/// Hook names ANOLISA recorded as managed in the receipt payload.
fn managed_hook_names(claim: &AdapterClaim) -> &[String] {
    qoder_payload(claim)
        .map(|q| q.managed_hooks.as_slice())
        .unwrap_or(&[])
}

// ---------------------------------------------------------------------------
// qodercli resolution
// ---------------------------------------------------------------------------

/// Resolve the qodercli binary in the legacy search order, honoring the
/// `QODERCLI_BIN` override first.
fn resolve_qodercli(user_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(bin) = std::env::var_os("QODERCLI_BIN") {
        let s = bin.to_string_lossy();
        if !s.is_empty() {
            let p = PathBuf::from(s.as_ref());
            if is_executable_file(&p) {
                return Some(p);
            }
            // A bare name override resolves via PATH.
            return find_binary_in_path(&s);
        }
    }
    if let Some(home) = user_home {
        let dir = qoder_home(home).join("bin").join("qodercli");
        if let Some(versioned) = highest_versioned_qodercli(&dir) {
            return Some(versioned);
        }
        let unversioned = dir.join("qodercli");
        if is_executable_file(&unversioned) {
            return Some(unversioned);
        }
    }
    find_binary_in_path("qodercli")
}

/// Program string for a [`FrameworkCommand`] built from [`resolve_qodercli`].
fn qodercli_program(user_home: Option<&Path>) -> Option<String> {
    resolve_qodercli(user_home).map(|p| p.to_string_lossy().into_owned())
}

/// Highest-versioned `qodercli-X.Y.Z` under `dir` (semver-aware: 10 > 9).
fn highest_versioned_qodercli(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(Vec<u64>, String, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(suffix) = name.strip_prefix("qodercli-") else {
            continue;
        };
        if suffix.is_empty() {
            continue;
        }
        let path = entry.path();
        if !is_executable_file(&path) {
            continue;
        }
        let key = version_key(suffix);
        let better = match &best {
            None => true,
            Some((bk, bs, _)) => key > *bk || (key == *bk && suffix > bs.as_str()),
        };
        if better {
            best = Some((key, suffix.to_string(), path));
        }
    }
    best.map(|(_, _, p)| p)
}

/// Numeric components of a version suffix, for semver-ish ordering.
fn version_key(suffix: &str) -> Vec<u64> {
    suffix
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<u64>().ok())
        .collect()
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

// ---------------------------------------------------------------------------
// Command builders
// ---------------------------------------------------------------------------

fn base_cmd(program: &str, args: Vec<String>) -> FrameworkCommand {
    FrameworkCommand {
        program: program.to_string(),
        args,
        env_set: Vec::new(),
        env_remove: Vec::new(),
        path_prepend: Vec::new(),
        timeout: CLI_TIMEOUT,
    }
}

fn build_install_cmd(program: &str, staging: &Path) -> FrameworkCommand {
    base_cmd(
        program,
        vec![
            "plugins".to_string(),
            "install".to_string(),
            staging.to_string_lossy().into_owned(),
        ],
    )
}

fn build_uninstall_cmd(program: &str, plugin: &str) -> FrameworkCommand {
    base_cmd(
        program,
        vec![
            "plugins".to_string(),
            "uninstall".to_string(),
            plugin.to_string(),
        ],
    )
}

// ---------------------------------------------------------------------------
// Status assembly
// ---------------------------------------------------------------------------

/// Build the `ResourceBundleMatches` condition.
fn bundle_match_condition(claim: &AdapterClaim) -> AdapterCondition {
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

/// Roll signals into a summary. Healthy requires the framework detected and
/// our managed settings entries verified present. Plugin registration is
/// deliberately excluded (qodercli's list is unreliable).
fn summarize(
    claim_status: ClaimStatus,
    detected: bool,
    settings: ConditionStatus,
) -> AdapterSummary {
    if claim_status == ClaimStatus::CleanupFailed {
        return AdapterSummary::CleanupFailed;
    }
    if !detected {
        return AdapterSummary::Degraded;
    }
    match settings {
        ConditionStatus::True => AdapterSummary::Healthy,
        ConditionStatus::False => AdapterSummary::Degraded,
        ConditionStatus::Unknown => AdapterSummary::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(s: &str) -> Value {
        serde_json::from_str(s).expect("valid JSON")
    }

    fn obj(v: Value) -> Map<String, Value> {
        match v {
            Value::Object(m) => m,
            other => panic!("expected object, got {other}"),
        }
    }

    fn resolved_hooks() -> Value {
        json(
            r#"{
              "hooks": {
                "PreToolUse": [
                  { "hooks": [ { "type": "command", "name": "tokenless-rewrite" } ] }
                ],
                "PostToolUse": [
                  { "hooks": [ { "type": "command", "name": "tokenless-compress" } ] }
                ]
              }
            }"#,
        )
    }

    #[test]
    fn identifiers_are_plugin_scoped() {
        assert_eq!(hook_prefix("tokenless"), "tokenless-");
        assert_eq!(plugin_entry("tokenless"), "tokenless@local");
    }

    #[test]
    fn install_and_uninstall_cmd_shapes() {
        let install = build_install_cmd("qodercli", Path::new("/data/qoder-plugins/tokenless"));
        assert_eq!(install.program, "qodercli");
        assert_eq!(
            install.args,
            vec!["plugins", "install", "/data/qoder-plugins/tokenless"]
        );
        let uninstall = build_uninstall_cmd("qodercli", "tokenless");
        assert_eq!(uninstall.args, vec!["plugins", "uninstall", "tokenless"]);
    }

    #[test]
    fn version_key_orders_semver_numerically() {
        assert!(version_key("10.0.0") > version_key("9.9.9"));
        assert!(version_key("1.2.0") > version_key("1.1.9"));
    }

    #[test]
    fn common_hooks_dir_is_sibling_of_resource_root() {
        assert_eq!(
            common_hooks_dir(Path::new("/data/adapters/tokenless/qoder")),
            PathBuf::from("/data/adapters/tokenless/common/hooks")
        );
    }

    #[test]
    fn merge_into_empty_adds_hooks_and_plugin() {
        let mut root = Map::new();
        merge_managed(&mut root, &resolved_hooks(), "tokenless@local");
        let v = Value::Object(root);
        assert_eq!(
            v["hooks"]["PreToolUse"][0]["hooks"][0]["name"],
            "tokenless-rewrite"
        );
        assert_eq!(v["plugins"]["enabled"][0], "tokenless@local");
    }

    #[test]
    fn merge_preserves_user_hooks_and_config() {
        let mut root = obj(json(
            r#"{
              "theme": "dark",
              "hooks": {
                "PreToolUse": [
                  { "hooks": [ { "type": "command", "name": "user-audit" } ] }
                ]
              },
              "plugins": { "enabled": ["other@local"], "registry": "corp" }
            }"#,
        ));
        merge_managed(&mut root, &resolved_hooks(), "tokenless@local");
        let v = Value::Object(root);
        // User settings untouched.
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["plugins"]["registry"], "corp");
        // User hook survives alongside ours in the same event.
        let pre = v["hooks"]["PreToolUse"].as_array().expect("array");
        let names: Vec<&str> = pre
            .iter()
            .filter_map(|e| e["hooks"][0]["name"].as_str())
            .collect();
        assert!(names.contains(&"user-audit"));
        assert!(names.contains(&"tokenless-rewrite"));
        // Both plugin entries present, ours appended.
        let enabled = v["plugins"]["enabled"].as_array().expect("array");
        assert!(enabled.iter().any(|x| x == "other@local"));
        assert!(enabled.iter().any(|x| x == "tokenless@local"));
    }

    #[test]
    fn merge_is_idempotent() {
        let mut root = Map::new();
        merge_managed(&mut root, &resolved_hooks(), "tokenless@local");
        merge_managed(&mut root, &resolved_hooks(), "tokenless@local");
        let v = Value::Object(root);
        assert_eq!(
            v["hooks"]["PreToolUse"].as_array().expect("array").len(),
            1,
            "re-merge must not duplicate a hook by name"
        );
        assert_eq!(
            v["plugins"]["enabled"].as_array().expect("array").len(),
            1,
            "re-merge must not duplicate the plugin entry"
        );
    }

    #[test]
    fn prune_removes_only_managed_entries() {
        let mut root = obj(json(
            r#"{
              "theme": "dark",
              "hooks": {
                "PreToolUse": [
                  { "hooks": [ { "type": "command", "name": "user-audit" } ] },
                  { "hooks": [ { "type": "command", "name": "tokenless-rewrite" } ] }
                ],
                "PostToolUse": [
                  { "hooks": [ { "type": "command", "name": "tokenless-compress" } ] }
                ]
              },
              "plugins": { "enabled": ["other@local", "tokenless@local"] }
            }"#,
        ));
        let changed = prune_managed(&mut root, "tokenless-", "tokenless@local");
        assert!(changed);
        let v = Value::Object(root);
        // User settings preserved.
        assert_eq!(v["theme"], "dark");
        // User hook kept; ANOLISA hook removed from PreToolUse.
        let pre = v["hooks"]["PreToolUse"].as_array().expect("array");
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0]["hooks"][0]["name"], "user-audit");
        // PostToolUse had only our hook -> event removed entirely.
        assert!(v["hooks"].get("PostToolUse").is_none());
        // Only the user's plugin entry remains.
        let enabled = v["plugins"]["enabled"].as_array().expect("array");
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0], "other@local");
    }

    #[test]
    fn prune_does_not_match_user_hook_by_command_substring() {
        // A user hook whose *command* mentions tokenless but whose *name*
        // does not must survive: pruning matches by name prefix only.
        let mut root = obj(json(
            r#"{
              "hooks": {
                "PreToolUse": [
                  { "hooks": [ { "type": "command", "name": "my-hook",
                                 "command": "run tokenless stats" } ] }
                ]
              }
            }"#,
        ));
        let changed = prune_managed(&mut root, "tokenless-", "tokenless@local");
        assert!(!changed, "no managed entries present");
        let v = Value::Object(root);
        assert_eq!(v["hooks"]["PreToolUse"][0]["hooks"][0]["name"], "my-hook");
    }

    #[test]
    fn prune_collapses_emptied_containers_and_is_idempotent() {
        let mut root = obj(json(
            r#"{
              "hooks": {
                "PreToolUse": [
                  { "hooks": [ { "type": "command", "name": "tokenless-rewrite" } ] }
                ]
              },
              "plugins": { "enabled": ["tokenless@local"] }
            }"#,
        ));
        assert!(prune_managed(&mut root, "tokenless-", "tokenless@local"));
        // Fully collapsed to an empty object.
        assert!(root.get("hooks").is_none());
        assert!(root.get("plugins").is_none());
        // Second prune is a clean no-op.
        assert!(!prune_managed(&mut root, "tokenless-", "tokenless@local"));
    }

    #[test]
    fn settings_managed_present_detects_both_signals() {
        let expected = vec![
            "tokenless-rewrite".to_string(),
            "tokenless-compress-response".to_string(),
        ];
        let root = obj(json(
            r#"{
              "hooks": {
                "PreToolUse": [ { "hooks": [ { "name": "tokenless-rewrite" } ] } ],
                "PostToolUse": [ { "hooks": [ { "name": "tokenless-compress-response" } ] } ]
              },
              "plugins": { "enabled": ["tokenless@local"] }
            }"#,
        ));
        assert_eq!(
            settings_managed_present(&root, &expected, "tokenless-", "tokenless@local"),
            (true, true)
        );

        let partial = obj(json(r#"{ "plugins": { "enabled": ["tokenless@local"] } }"#));
        assert_eq!(
            settings_managed_present(&partial, &expected, "tokenless-", "tokenless@local"),
            (false, true)
        );

        let none = obj(json(r#"{ "theme": "dark" }"#));
        assert_eq!(
            settings_managed_present(&none, &expected, "tokenless-", "tokenless@local"),
            (false, false)
        );
    }

    #[test]
    fn settings_managed_present_requires_all_expected_hooks() {
        // One of the two expected hooks is missing -> hooks_present is false,
        // even though a tokenless-* hook still exists (partial drift).
        let expected = vec![
            "tokenless-rewrite".to_string(),
            "tokenless-compress-response".to_string(),
        ];
        let root = obj(json(
            r#"{
              "hooks": { "PreToolUse": [
                { "hooks": [ { "name": "tokenless-rewrite" } ] } ] },
              "plugins": { "enabled": ["tokenless@local"] }
            }"#,
        ));
        assert_eq!(
            settings_managed_present(&root, &expected, "tokenless-", "tokenless@local"),
            (false, true),
            "a missing managed hook must not read as present"
        );
    }

    #[test]
    fn settings_managed_present_falls_back_to_prefix_when_no_expected() {
        // Legacy receipt with no recorded hook names -> any tokenless-* hook
        // counts as present.
        let root = obj(json(
            r#"{ "hooks": { "PreToolUse": [
                { "hooks": [ { "name": "tokenless-rewrite" } ] } ] } }"#,
        ));
        let (hooks_present, _) =
            settings_managed_present(&root, &[], "tokenless-", "tokenless@local");
        assert!(hooks_present);
    }

    #[test]
    fn collect_expected_hook_names_reads_all_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("adapters").join("tokenless").join("qoder");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(
            root.join(QODER_HOOKS_FILE),
            br#"{"hooks":{
              "PreToolUse":[{"hooks":[{"name":"tokenless-rewrite"}]}],
              "PostToolUse":[{"hooks":[{"name":"tokenless-compress-response"}]}]
            }}"#,
        )
        .expect("write hooks");
        let mut names = collect_expected_hook_names(&root).expect("collect");
        names.sort();
        assert_eq!(
            names,
            vec!["tokenless-compress-response", "tokenless-rewrite"]
        );
    }

    #[test]
    fn load_settings_for_merge_fails_closed_on_malformed_json() {
        let path = Path::new("/home/u/.qoder/settings.json");
        // Absent -> empty object.
        assert!(
            load_settings_for_merge(None, path)
                .expect("absent ok")
                .is_empty()
        );
        // Non-object -> empty object (legacy behavior).
        assert!(
            load_settings_for_merge(Some(b"[1,2,3]".to_vec()), path)
                .expect("array ok")
                .is_empty()
        );
        // Malformed -> hard error, so enable never clobbers it.
        let err = load_settings_for_merge(Some(b"{not json".to_vec()), path)
            .expect_err("malformed must fail closed");
        assert!(matches!(err, AdapterError::SettingsUnparseable { .. }));
    }

    #[test]
    fn load_resolved_hooks_substitutes_placeholder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("adapters").join("tokenless").join("qoder");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(
            root.join(QODER_HOOKS_FILE),
            br#"{"hooks":{"PreToolUse":[{"hooks":[{"name":"tokenless-rewrite","command":"bash ${QODER_TOKENLESS_HOOKS}/rewrite.sh"}]}]}}"#,
        )
        .expect("write hooks");
        let resolved = load_resolved_hooks(&root).expect("resolve");
        let cmd = resolved["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .expect("command");
        assert!(
            cmd.contains("adapters/tokenless/common/hooks/rewrite.sh"),
            "{cmd}"
        );
        assert!(!cmd.contains(HOOKS_PLACEHOLDER));
    }

    #[test]
    fn read_bundle_requires_manifest_and_hooks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("qoder");
        std::fs::create_dir_all(root.join(".qoder-plugin")).expect("mkdir");
        let layout = anolisa_platform::fs_layout::FsLayout::user(PathBuf::from("/tmp/qoder-home"));

        struct StubOps;
        impl super::super::driver::AdapterOps for StubOps {
            fn run_framework_cli(
                &self,
                _: FrameworkCommand,
            ) -> Result<super::super::driver::CliOutput, AdapterError> {
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
            fn write_file(&self, _: &Path, _: &[u8]) -> Result<(), AdapterError> {
                unimplemented!()
            }
            fn create_symlink(&self, _: &Path, _: &Path) -> Result<(), AdapterError> {
                unimplemented!()
            }
            fn read_file(&self, _: &Path) -> Result<Option<Vec<u8>>, AdapterError> {
                unimplemented!()
            }
        }
        let ops = StubOps;
        let mk_ctx = |root: &Path| DriverCtx {
            component: "tokenless".to_string(),
            framework: "qoder".to_string(),
            layout: &layout,
            resource_root: root.to_path_buf(),
            user_home: Some(PathBuf::from("/tmp/qoder-home")),
            declared_plugin_id: Some("tokenless".to_string()),
            adapter_type: Some("plugin".to_string()),
            declared_skills: Vec::new(),
            declared_config: Vec::new(),
            declared_bundle_entry: None,
            dry_run: true,
            ops: &ops,
        };
        let driver = QoderDriver::new();

        // plugin.json only -> hooks.json missing.
        std::fs::write(root.join(QODER_PLUGIN_MANIFEST), br#"{"name":"tokenless"}"#)
            .expect("write manifest");
        let err = driver
            .read_bundle(&mk_ctx(&root))
            .expect_err("hooks.json missing must fail");
        assert!(matches!(err, AdapterError::BundleInvalid { .. }));

        // Both present -> ok.
        std::fs::write(root.join(QODER_HOOKS_FILE), b"{}").expect("write hooks");
        let bundle = driver.read_bundle(&mk_ctx(&root)).expect("both present");
        assert_eq!(bundle.plugin_id.as_deref(), Some("tokenless"));
    }
}
