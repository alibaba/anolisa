//! Qoder framework driver backed exclusively by the native plugin lifecycle.
//!
//! Registration, activation, inventory, and removal go through
//! `qodercli plugins`. The driver never writes Qoder activation state. It only
//! reads a legacy settings resource recorded by an older receipt so those
//! exact entries can be removed after a verified native installation.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use super::AdapterError;
use super::claim::{
    AdapterClaim, CLAIM_SCHEMA_VERSION, ClaimResource, ClaimResourceKind, ClaimStatus,
    DRIVER_SCHEMA_VERSION, DriverPayload, QoderClaim, QoderManagedHook, validate_plugin_id,
};
use super::driver::{
    AdapterBundle, AdapterCondition, AdapterConditionKind, AdapterStatusReport, AdapterSummary,
    ClaimResourceRef, ConditionStatus, DetectResult, DisableReport, DriverCtx, DriverPlan,
    EnableProgress, FrameworkCommand, FrameworkDriver, HostEnv, PreparedEnable,
    find_binary_in_path,
};
use super::util::{bool_status, cli_failure_reason, digest_tree, display_command, now_iso8601};

mod settings;

use settings::{prune_settings_via_ops, reconstruct_legacy_hook_specs};

const CLI_TIMEOUT: Duration = Duration::from_secs(60);
const QODER_PLUGIN_MANIFEST: &str = ".qoder-plugin/plugin.json";
const QODER_HOOKS_FILE: &str = "hooks/hooks.json";
const QODER_COMMAND_FILE: &str = "commands/tokenless-stats.md";
const QODER_HOOK_RUNNER: &str = "hooks/run-hook.sh";
const QODER_EMBEDDED_RESOURCES: [&str; 6] = [
    "common/hooks/hook_utils.py",
    "common/hooks/tool_ready_hook.sh",
    "common/hooks/rewrite_hook.py",
    "common/hooks/compress_response_hook.py",
    "common/tool-ready-spec.json",
    "common/tokenless-env-fix.sh",
];
const RES_PLUGIN: &str = "qoder_plugin";

/// Qoder driver. All mutable framework state is delegated to `qodercli`.
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

    fn probe_bundle(&self, resource_root: &Path, declared_entry: Option<&str>) -> bool {
        std::iter::once(declared_entry.unwrap_or(QODER_PLUGIN_MANIFEST))
            .chain([QODER_HOOKS_FILE, QODER_COMMAND_FILE, QODER_HOOK_RUNNER])
            .chain(QODER_EMBEDDED_RESOURCES)
            .all(|relative| resource_root.join(relative).is_file())
    }

    fn detect(&self, env: &HostEnv) -> DetectResult {
        match resolve_qodercli(env.user_home.as_deref()) {
            Some(path) => DetectResult {
                detected: true,
                reason: format!("qodercli found at {}", path.display()),
            },
            None => DetectResult {
                detected: false,
                reason:
                    "qodercli not found (checked $QODERCLI_BIN, $QODER_CONFIG_DIR, ~/.qoder, PATH)"
                        .to_string(),
            },
        }
    }

    fn allowed_external_roots(&self, ctx: &DriverCtx) -> Vec<PathBuf> {
        // Native plugin state is owned by qodercli. This root exists only so a
        // validated legacy receipt can remove its exact old settings entries.
        ctx.user_home
            .as_deref()
            .map(qoder_home)
            .into_iter()
            .collect()
    }

    fn read_bundle(&self, ctx: &DriverCtx) -> Result<AdapterBundle, AdapterError> {
        let root = &ctx.resource_root;
        if !root.is_dir() {
            return Err(invalid_bundle(root, "resource root is not a directory"));
        }
        let manifest = ctx
            .declared_bundle_entry
            .as_deref()
            .unwrap_or(QODER_PLUGIN_MANIFEST);
        for relative in std::iter::once(manifest)
            .chain([QODER_HOOKS_FILE, QODER_COMMAND_FILE, QODER_HOOK_RUNNER])
            .chain(QODER_EMBEDDED_RESOURCES)
        {
            if !root.join(relative).is_file() {
                return Err(invalid_bundle(
                    root,
                    &format!("native Qoder plugin resource '{relative}' is missing"),
                ));
            }
        }
        validate_hook_bundle(ctx)?;

        let plugin_id = ctx
            .declared_plugin_id
            .clone()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| ctx.component.clone());
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
        let install = build_install_cmd(&program, &bundle.resource_root);
        Ok(DriverPlan {
            framework: self.name().to_string(),
            component: ctx.component.clone(),
            actions: vec![
                "validate Qoder native plugin capabilities and bundle discovery".to_string(),
                format!("install and enable native Qoder plugin '{plugin}'"),
                "verify plugin registration, activation, hooks, and command via JSON inventory"
                    .to_string(),
                "migrate exact settings entries retained by a legacy receipt, when present"
                    .to_string(),
            ],
            register_command: Some(display_command(&install)),
        })
    }

    fn prepare_enable(
        &self,
        bundle: &AdapterBundle,
        ctx: &DriverCtx,
    ) -> Result<(AdapterClaim, PreparedEnable), AdapterError> {
        let plugin = plugin_name(bundle, ctx);
        validate_plugin_id(&plugin)?;
        let program = require_qodercli(ctx)?;

        for subcommand in ["validate", "install", "list", "uninstall"] {
            require_success(
                &program,
                &format!("plugins {subcommand} --help"),
                ctx.ops
                    .run_framework_cli(build_help_cmd(&program, subcommand))?,
            )?;
        }
        require_success(
            &program,
            "plugins validate",
            ctx.ops
                .run_framework_cli(build_validate_cmd(&program, &bundle.resource_root))?,
        )?;
        let before = ctx.ops.run_framework_cli(build_list_cmd(&program))?;
        require_success(&program, "plugins list --json", before.clone())?;
        let inventory =
            parse_plugin_list(&before.stdout, &plugin_entry(&plugin)).map_err(|reason| {
                AdapterError::FrameworkCli {
                    program: program.clone(),
                    reason: format!(
                        "Qoder native plugin inventory is unavailable ({reason}); upgrade Qoder"
                    ),
                }
            })?;
        // Presence and health are separate: a disabled or partially loaded
        // registration still belongs to the pre-enable state and must not be
        // removed by rollback.
        let preexisting_native_plugin = inventory.is_some();

        let claim = AdapterClaim {
            claim_schema: CLAIM_SCHEMA_VERSION,
            component: ctx.component.clone(),
            framework: self.name().to_string(),
            plugin_id: Some(plugin.clone()),
            adapter_type: ctx.adapter_type.clone(),
            enabled_at: now_iso8601(),
            resource_root: bundle.resource_root.clone(),
            bundle_digest: bundle.digest.clone(),
            driver_schema: DRIVER_SCHEMA_VERSION,
            status: ClaimStatus::Enabled,
            notices: Vec::new(),
            resources: vec![ClaimResource {
                id: RES_PLUGIN.to_string(),
                purpose: "qoder_plugin".to_string(),
                kind: ClaimResourceKind::FrameworkPlugin {
                    framework: self.name().to_string(),
                    plugin_id: plugin,
                },
            }],
            driver_payload: DriverPayload::Qoder(QoderClaim {
                plugin_resource: RES_PLUGIN.to_string(),
                settings_resource: None,
                managed_hooks: Vec::new(),
                managed_hook_specs: Vec::new(),
            }),
        };
        Ok((
            claim,
            PreparedEnable::Qoder {
                preexisting_native_plugin,
            },
        ))
    }

    fn preserve_reenable_facts(
        &self,
        prior: &AdapterClaim,
        next: &mut AdapterClaim,
    ) -> Result<(), AdapterError> {
        preserve_legacy_facts(prior, next)
    }

    fn apply_enable(
        &self,
        claim: &mut AdapterClaim,
        prepared: &PreparedEnable,
        ctx: &DriverCtx,
        progress: &mut dyn EnableProgress,
    ) -> Result<(), AdapterError> {
        let PreparedEnable::Qoder {
            preexisting_native_plugin,
        } = prepared
        else {
            return Err(invalid_bundle(
                &claim.resource_root,
                "Qoder enable requires prepared native-plugin capabilities",
            ));
        };
        let plugin = resolve_plugin(claim).ok_or_else(|| {
            invalid_bundle(&claim.resource_root, "receipt has no Qoder plugin resource")
        })?;
        let program = require_qodercli(ctx)?;

        let install = ctx
            .ops
            .run_framework_cli(build_install_cmd(&program, &claim.resource_root))?;
        require_success(&program, "plugins install", install)?;

        if let Err(reason) = verify_native_plugin(ctx, &program, &plugin) {
            let rollback = rollback_new_install(ctx, &program, &plugin, *preexisting_native_plugin);
            return Err(AdapterError::FrameworkCli {
                program,
                reason: append_rollback(reason, rollback),
            });
        }

        if let Some(settings) = resolve_legacy_settings(claim, ctx.user_home.as_deref()) {
            let mut messages = Vec::new();
            let managed_hooks = match managed_hook_specs_for_migration(claim) {
                Ok(hooks) => hooks,
                Err(reason) => {
                    let rollback =
                        rollback_new_install(ctx, &program, &plugin, *preexisting_native_plugin);
                    return Err(AdapterError::SettingsUnparseable {
                        path: settings,
                        reason: append_rollback(reason, rollback),
                    });
                }
            };
            if !prune_settings_via_ops(
                ctx,
                &settings,
                &plugin_entry(&plugin),
                &managed_hooks,
                &mut messages,
            ) {
                let rollback =
                    rollback_new_install(ctx, &program, &plugin, *preexisting_native_plugin);
                return Err(AdapterError::SettingsUnparseable {
                    path: settings,
                    reason: append_rollback(messages.join("; "), rollback),
                });
            }
            clear_legacy_facts(claim);
            progress.persist_claim(claim)?;
        } else if has_legacy_facts(claim) {
            let rollback = rollback_new_install(ctx, &program, &plugin, *preexisting_native_plugin);
            let reason = append_rollback(
                "legacy Qoder settings resource is missing or not ~/.qoder/settings.json"
                    .to_string(),
                rollback,
            );
            return Err(invalid_bundle(&claim.resource_root, &reason));
        }
        Ok(())
    }

    fn status(
        &self,
        claim: &AdapterClaim,
        ctx: &DriverCtx,
    ) -> Result<AdapterStatusReport, AdapterError> {
        let detect = self.detect(&HostEnv {
            user_home: ctx.user_home.clone(),
        });
        let mut conditions = vec![
            AdapterCondition {
                kind: AdapterConditionKind::FrameworkDetected,
                status: bool_status(detect.detected),
                reason: Some(detect.reason),
                resource: None,
            },
            bundle_match_condition(claim),
        ];
        let Some(plugin) = resolve_plugin(claim) else {
            push_inventory_conditions(
                &mut conditions,
                ConditionStatus::False,
                ConditionStatus::False,
                ConditionStatus::False,
                ConditionStatus::False,
                Some("receipt has no Qoder plugin resource".to_string()),
            );
            return Ok(AdapterStatusReport {
                summary: summarize(claim.status, false, false, false),
                conditions,
            });
        };
        let Some(program) = qodercli_program(ctx.user_home.as_deref()) else {
            push_inventory_conditions(
                &mut conditions,
                ConditionStatus::Unknown,
                ConditionStatus::Unknown,
                ConditionStatus::Unknown,
                ConditionStatus::False,
                Some("qodercli unavailable; plugin inventory cannot be read".to_string()),
            );
            return Ok(AdapterStatusReport {
                summary: summarize(claim.status, false, false, false),
                conditions,
            });
        };

        let output = ctx.ops.run_framework_cli(build_list_cmd(&program))?;
        if !output.success() {
            push_inventory_conditions(
                &mut conditions,
                ConditionStatus::Unknown,
                ConditionStatus::Unknown,
                ConditionStatus::Unknown,
                ConditionStatus::False,
                Some(cli_failure_reason("plugins list --json", &output)),
            );
            return Ok(AdapterStatusReport {
                summary: summarize(claim.status, true, false, false),
                conditions,
            });
        }
        let inventory = match parse_plugin_list(&output.stdout, &plugin_entry(&plugin)) {
            Ok(inventory) => inventory,
            Err(reason) => {
                push_inventory_conditions(
                    &mut conditions,
                    ConditionStatus::Unknown,
                    ConditionStatus::Unknown,
                    ConditionStatus::Unknown,
                    ConditionStatus::False,
                    Some(reason),
                );
                return Ok(AdapterStatusReport {
                    summary: summarize(claim.status, true, false, false),
                    conditions,
                });
            }
        };
        let (registered, enabled, resources, reason) = match inventory {
            None => (
                false,
                false,
                false,
                Some("tokenless@local is absent".to_string()),
            ),
            Some(inventory) => {
                let reason = (!inventory.resources_complete()).then(|| inventory.diagnostics());
                (
                    true,
                    inventory.enabled,
                    inventory.resources_complete(),
                    reason,
                )
            }
        };
        push_inventory_conditions(
            &mut conditions,
            bool_status(registered),
            bool_status(enabled),
            bool_status(resources),
            ConditionStatus::True,
            reason,
        );
        Ok(AdapterStatusReport {
            summary: summarize(claim.status, registered, enabled, resources),
            conditions,
        })
    }

    fn disable(
        &self,
        claim: &AdapterClaim,
        ctx: &DriverCtx,
    ) -> Result<DisableReport, AdapterError> {
        let Some(plugin) = resolve_plugin(claim) else {
            return Ok(DisableReport {
                cleanup_complete: false,
                messages: vec!["Qoder receipt has no plugin resource; receipt kept".to_string()],
            });
        };
        let mut messages = Vec::new();
        let settings_ok = cleanup_legacy_settings(claim, ctx, &plugin, &mut messages);
        let Some(program) = qodercli_program(ctx.user_home.as_deref()) else {
            messages.push("qodercli unavailable; receipt kept for retry".to_string());
            return Ok(DisableReport {
                cleanup_complete: false,
                messages,
            });
        };
        let uninstall = ctx
            .ops
            .run_framework_cli(build_uninstall_cmd(&program, &plugin))?;
        if uninstall.success() {
            messages.push(format!("uninstalled native Qoder plugin '{plugin}'"));
        } else {
            messages.push(cli_failure_reason("plugins uninstall", &uninstall));
        }
        let plugin_ok = match ctx.ops.run_framework_cli(build_list_cmd(&program)) {
            Ok(output) if output.success() => {
                match parse_plugin_list(&output.stdout, &plugin_entry(&plugin)) {
                    Ok(None) => true,
                    Ok(Some(_)) => {
                        messages.push(
                            "Qoder still reports tokenless@local after uninstall".to_string(),
                        );
                        false
                    }
                    Err(reason) => {
                        messages.push(format!("cannot verify Qoder uninstall: {reason}"));
                        false
                    }
                }
            }
            Ok(output) => {
                messages.push(cli_failure_reason("plugins list --json", &output));
                false
            }
            Err(err) => {
                messages.push(format!("cannot verify Qoder uninstall: {err}"));
                false
            }
        };

        Ok(DisableReport {
            cleanup_complete: plugin_ok && settings_ok,
            messages,
        })
    }
}

fn cleanup_legacy_settings(
    claim: &AdapterClaim,
    ctx: &DriverCtx,
    plugin: &str,
    messages: &mut Vec<String>,
) -> bool {
    if let Some(settings) = resolve_legacy_settings(claim, ctx.user_home.as_deref()) {
        match managed_hook_specs_for_migration(claim) {
            Ok(managed_hooks) => prune_settings_via_ops(
                ctx,
                &settings,
                &plugin_entry(plugin),
                &managed_hooks,
                messages,
            ),
            Err(reason) => {
                messages.push(reason);
                false
            }
        }
    } else if has_legacy_facts(claim) {
        messages.push(
            "legacy settings resource is missing or not ~/.qoder/settings.json; receipt kept"
                .to_string(),
        );
        false
    } else {
        true
    }
}

#[derive(Debug, Clone)]
struct PluginInventory {
    enabled: bool,
    pre_tool_use: usize,
    post_tool_use: usize,
    command_present: bool,
}

impl PluginInventory {
    fn resources_complete(&self) -> bool {
        self.pre_tool_use == 2 && self.post_tool_use == 1 && self.command_present
    }

    fn diagnostics(&self) -> String {
        format!(
            "incomplete Qoder resources: PreToolUse={}, PostToolUse={}, tokenless-stats={}",
            self.pre_tool_use, self.post_tool_use, self.command_present
        )
    }
}

fn validate_hook_bundle(ctx: &DriverCtx) -> Result<(), AdapterError> {
    let root = &ctx.resource_root;
    let path = root.join(QODER_HOOKS_FILE);
    let bytes = ctx
        .ops
        .read_file(&path)?
        .ok_or_else(|| invalid_bundle(root, &format!("{QODER_HOOKS_FILE} is missing")))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|err| invalid_bundle(root, &format!("invalid {QODER_HOOKS_FILE}: {err}")))?;
    let hooks = value
        .get("hooks")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_bundle(root, "hooks/hooks.json lacks the 'hooks' object"))?;
    let pre = hooks
        .get("PreToolUse")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let post = hooks
        .get("PostToolUse")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if pre != 2 || post != 1 {
        return Err(invalid_bundle(
            root,
            &format!("expected 2 PreToolUse and 1 PostToolUse hooks, found {pre} and {post}"),
        ));
    }
    Ok(())
}

fn parse_plugin_list(text: &str, expected_id: &str) -> Result<Option<PluginInventory>, String> {
    let root: Value = serde_json::from_str(text).map_err(|err| format!("invalid JSON: {err}"))?;
    let plugins = if let Some(items) = root.as_array() {
        items
    } else {
        ["plugins", "installed", "items"]
            .iter()
            .find_map(|key| root.get(key).and_then(Value::as_array))
            .ok_or_else(|| "plugin inventory root is not an array".to_string())?
    };
    let Some(plugin) = plugins.iter().find(|item| {
        item.get("id").and_then(Value::as_str) == Some(expected_id)
            || item.get("pluginId").and_then(Value::as_str) == Some(expected_id)
    }) else {
        return Ok(None);
    };
    let enabled = plugin
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let resources = plugin
        .get("resources")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{expected_id} has no resource inventory"))?;
    let hook_events: Vec<&str> = resources
        .get("hooks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|hook| hook.get("event").and_then(Value::as_str))
        .collect();
    let command_present = resources
        .get("commands")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|command| command.get("name").and_then(Value::as_str))
        .any(|name| name == "tokenless-stats" || name.ends_with(":tokenless-stats"));
    Ok(Some(PluginInventory {
        enabled,
        pre_tool_use: hook_events
            .iter()
            .filter(|event| **event == "PreToolUse")
            .count(),
        post_tool_use: hook_events
            .iter()
            .filter(|event| **event == "PostToolUse")
            .count(),
        command_present,
    }))
}

fn verify_native_plugin(ctx: &DriverCtx, program: &str, plugin: &str) -> Result<(), String> {
    let output = ctx
        .ops
        .run_framework_cli(build_list_cmd(program))
        .map_err(|err| err.to_string())?;
    if !output.success() {
        return Err(cli_failure_reason("plugins list --json", &output));
    }
    let inventory = parse_plugin_list(&output.stdout, &plugin_entry(plugin))?
        .ok_or_else(|| format!("{} is absent after install", plugin_entry(plugin)))?;
    if !inventory.enabled {
        return Err(format!(
            "{} is installed but disabled",
            plugin_entry(plugin)
        ));
    }
    if !inventory.resources_complete() {
        return Err(inventory.diagnostics());
    }
    Ok(())
}

fn preserve_legacy_facts(
    prior: &AdapterClaim,
    next: &mut AdapterClaim,
) -> Result<(), AdapterError> {
    let DriverPayload::Qoder(prior_payload) = &prior.driver_payload else {
        return Err(invalid_bundle(
            &prior.resource_root,
            "prior receipt is not Qoder",
        ));
    };
    let Some(resource_id) = prior_payload.settings_resource.as_deref() else {
        return Ok(());
    };
    let resource = prior.resource(resource_id).cloned().ok_or_else(|| {
        invalid_bundle(
            &prior.resource_root,
            "prior Qoder receipt references a missing settings resource",
        )
    })?;
    if next.resource(resource_id).is_none() {
        next.resources.push(resource);
    }
    let DriverPayload::Qoder(next_payload) = &mut next.driver_payload else {
        return Err(invalid_bundle(
            &next.resource_root,
            "next receipt is not Qoder",
        ));
    };
    next_payload.settings_resource = Some(resource_id.to_string());
    next_payload.managed_hooks = prior_payload.managed_hooks.clone();
    next_payload.managed_hook_specs = if prior_payload.managed_hook_specs.is_empty() {
        reconstruct_legacy_hook_specs(&prior.resource_root, &prior_payload.managed_hooks)
            .map_err(|reason| invalid_bundle(&prior.resource_root, &reason))?
    } else {
        prior_payload.managed_hook_specs.clone()
    };
    Ok(())
}

fn clear_legacy_facts(claim: &mut AdapterClaim) {
    let resource_id = match &mut claim.driver_payload {
        DriverPayload::Qoder(payload) => {
            payload.managed_hooks.clear();
            payload.managed_hook_specs.clear();
            payload.settings_resource.take()
        }
        _ => None,
    };
    if let Some(resource_id) = resource_id {
        claim
            .resources
            .retain(|resource| resource.id != resource_id);
    }
}

fn has_legacy_facts(claim: &AdapterClaim) -> bool {
    qoder_payload(claim).is_some_and(|payload| {
        payload.settings_resource.is_some()
            || !payload.managed_hooks.is_empty()
            || !payload.managed_hook_specs.is_empty()
    })
}

fn rollback_new_install(ctx: &DriverCtx, program: &str, plugin: &str, preexisting: bool) -> String {
    if preexisting {
        return "pre-existing native plugin retained".to_string();
    }
    let command = build_uninstall_cmd(program, plugin);
    match ctx.ops.run_framework_cli(command.clone()) {
        Ok(output) if output.success() => "newly installed plugin rolled back".to_string(),
        Ok(output) => format!(
            "DEGRADED: rollback failed ({}); run `{}`",
            cli_failure_reason("plugins uninstall", &output),
            display_command(&command)
        ),
        Err(err) => format!(
            "DEGRADED: rollback failed ({err}); run `{}`",
            display_command(&command)
        ),
    }
}

fn append_rollback(reason: String, rollback: String) -> String {
    format!("{reason}; {rollback}")
}

fn push_inventory_conditions(
    conditions: &mut Vec<AdapterCondition>,
    registered: ConditionStatus,
    enabled: ConditionStatus,
    resources: ConditionStatus,
    verification: ConditionStatus,
    reason: Option<String>,
) {
    for (kind, status) in [
        (AdapterConditionKind::PluginRegistered, registered),
        (AdapterConditionKind::ActivationEnabled, enabled),
        (AdapterConditionKind::PluginResourcesLoaded, resources),
        (AdapterConditionKind::VerificationSupported, verification),
    ] {
        conditions.push(AdapterCondition {
            kind,
            status,
            reason: reason.clone(),
            resource: (kind != AdapterConditionKind::VerificationSupported).then(|| {
                ClaimResourceRef {
                    id: RES_PLUGIN.to_string(),
                }
            }),
        });
    }
}

fn summarize(
    claim_status: ClaimStatus,
    registered: bool,
    enabled: bool,
    resources: bool,
) -> AdapterSummary {
    if claim_status == ClaimStatus::CleanupFailed {
        AdapterSummary::CleanupFailed
    } else if registered && enabled && resources {
        AdapterSummary::Healthy
    } else {
        AdapterSummary::Degraded
    }
}

fn bundle_match_condition(claim: &AdapterClaim) -> AdapterCondition {
    let (status, reason) = match (&claim.bundle_digest, digest_tree(&claim.resource_root)) {
        (Some(recorded), Some(current)) if recorded == &current => (ConditionStatus::True, None),
        (Some(_), Some(_)) => (
            ConditionStatus::False,
            Some("resource bundle changed since enable".to_string()),
        ),
        _ => (
            ConditionStatus::Unknown,
            Some("resource bundle unavailable or digest missing".to_string()),
        ),
    };
    AdapterCondition {
        kind: AdapterConditionKind::ResourceBundleMatches,
        status,
        reason,
        resource: None,
    }
}

fn invalid_bundle(root: &Path, reason: &str) -> AdapterError {
    AdapterError::BundleInvalid {
        root: root.to_path_buf(),
        reason: reason.to_string(),
    }
}

fn require_qodercli(ctx: &DriverCtx) -> Result<String, AdapterError> {
    qodercli_program(ctx.user_home.as_deref()).ok_or_else(|| AdapterError::FrameworkCli {
        program: "qodercli".to_string(),
        reason: "qodercli not found; install or upgrade Qoder".to_string(),
    })
}

fn require_success(
    program: &str,
    operation: &str,
    output: super::driver::CliOutput,
) -> Result<(), AdapterError> {
    if output.success() {
        Ok(())
    } else {
        Err(AdapterError::FrameworkCli {
            program: program.to_string(),
            reason: format!(
                "{}; upgrade Qoder if this native plugin command is unavailable",
                cli_failure_reason(operation, &output)
            ),
        })
    }
}

fn plugin_name(bundle: &AdapterBundle, ctx: &DriverCtx) -> String {
    bundle
        .plugin_id
        .clone()
        .unwrap_or_else(|| ctx.component.clone())
}

fn plugin_entry(plugin: &str) -> String {
    format!("{plugin}@local")
}

fn qoder_home(user_home: &Path) -> PathBuf {
    user_home.join(".qoder")
}

fn legacy_settings_path(user_home: Option<&Path>) -> Option<PathBuf> {
    user_home.map(|home| qoder_home(home).join("settings.json"))
}

fn qoder_config_root(user_home: Option<&Path>) -> Option<PathBuf> {
    std::env::var_os("QODER_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| user_home.map(qoder_home))
}

fn qoder_payload(claim: &AdapterClaim) -> Option<&QoderClaim> {
    match &claim.driver_payload {
        DriverPayload::Qoder(payload) => Some(payload),
        _ => None,
    }
}

fn resolve_plugin(claim: &AdapterClaim) -> Option<String> {
    let payload = qoder_payload(claim)?;
    claim
        .resource(&payload.plugin_resource)
        .and_then(|resource| match &resource.kind {
            ClaimResourceKind::FrameworkPlugin { plugin_id, .. } => Some(plugin_id.clone()),
            _ => None,
        })
}

fn resolve_legacy_settings(claim: &AdapterClaim, user_home: Option<&Path>) -> Option<PathBuf> {
    let payload = qoder_payload(claim)?;
    let resource_id = payload.settings_resource.as_deref()?;
    let recorded = claim
        .resource(resource_id)
        .and_then(|resource| match &resource.kind {
            ClaimResourceKind::ExternalPath { path } => Some(path.clone()),
            _ => None,
        })?;
    let expected = legacy_settings_path(user_home)?;
    (recorded == expected).then_some(recorded)
}

fn managed_hook_specs_for_migration(claim: &AdapterClaim) -> Result<Vec<QoderManagedHook>, String> {
    let payload = qoder_payload(claim).ok_or_else(|| "receipt is not Qoder".to_string())?;
    if !payload.managed_hook_specs.is_empty() {
        return Ok(payload.managed_hook_specs.clone());
    }
    if payload.settings_resource.is_none() {
        return Ok(Vec::new());
    }
    reconstruct_legacy_hook_specs(&claim.resource_root, &payload.managed_hooks)
}

fn resolve_qodercli(user_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(bin) = std::env::var_os("QODERCLI_BIN") {
        let value = bin.to_string_lossy();
        if !value.is_empty() {
            let path = PathBuf::from(value.as_ref());
            return is_executable_file(&path)
                .then_some(path)
                .or_else(|| find_binary_in_path(&value));
        }
    }
    if let Some(root) = qoder_config_root(user_home) {
        let dir = root.join("bin").join("qodercli");
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

fn qodercli_program(user_home: Option<&Path>) -> Option<String> {
    resolve_qodercli(user_home).map(|path| path.to_string_lossy().into_owned())
}

fn highest_versioned_qodercli(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(Vec<u64>, bool, String, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(suffix) = name.strip_prefix("qodercli-") else {
            continue;
        };
        let path = entry.path();
        if suffix.is_empty() || !is_executable_file(&path) {
            continue;
        }
        let key = version_key(suffix);
        let stable = is_stable_version_suffix(suffix);
        let better = match &best {
            None => true,
            Some((best_key, best_stable, best_suffix, _)) => {
                key > *best_key
                    || (key == *best_key && stable && !*best_stable)
                    || (key == *best_key && stable == *best_stable && suffix > best_suffix.as_str())
            }
        };
        if better {
            best = Some((key, stable, suffix.to_string(), path));
        }
    }
    best.map(|(_, _, _, path)| path)
}

fn version_key(suffix: &str) -> Vec<u64> {
    suffix
        .split_once('-')
        .map(|(stable, _)| stable)
        .unwrap_or(suffix)
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn is_stable_version_suffix(suffix: &str) -> bool {
    suffix
        .chars()
        .all(|character| character.is_ascii_digit() || character == '.')
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn base_cmd(program: &str, args: Vec<String>) -> FrameworkCommand {
    FrameworkCommand {
        program: program.to_string(),
        args,
        stdin: None,
        env_set: Vec::new(),
        env_remove: Vec::new(),
        path_prepend: Vec::new(),
        timeout: CLI_TIMEOUT,
    }
}

fn build_help_cmd(program: &str, subcommand: &str) -> FrameworkCommand {
    base_cmd(
        program,
        vec![
            "plugins".to_string(),
            subcommand.to_string(),
            "--help".to_string(),
        ],
    )
}

fn build_validate_cmd(program: &str, root: &Path) -> FrameworkCommand {
    base_cmd(
        program,
        vec![
            "plugins".to_string(),
            "validate".to_string(),
            root.to_string_lossy().into_owned(),
        ],
    )
}

fn build_install_cmd(program: &str, root: &Path) -> FrameworkCommand {
    base_cmd(
        program,
        vec![
            "plugins".to_string(),
            "install".to_string(),
            root.to_string_lossy().into_owned(),
            "--scope".to_string(),
            "user".to_string(),
        ],
    )
}

fn build_list_cmd(program: &str) -> FrameworkCommand {
    base_cmd(
        program,
        vec![
            "plugins".to_string(),
            "list".to_string(),
            "--json".to_string(),
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
            "--scope".to_string(),
            "user".to_string(),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST_JSON: &str = r#"[{
      "id":"tokenless@local","enabled":true,
      "resources":{
        "commands":[{"name":"tokenless:tokenless-stats"}],
        "hooks":[
          {"event":"PreToolUse","type":"command"},
          {"event":"PreToolUse","type":"command","matcher":"Bash"},
          {"event":"PostToolUse","type":"command"}
        ]
      }
    }]"#;

    #[test]
    fn native_command_shapes_include_scope_and_json() {
        assert_eq!(
            build_install_cmd("qodercli", Path::new("/data/qoder")).args,
            ["plugins", "install", "/data/qoder", "--scope", "user"]
        );
        assert_eq!(
            build_uninstall_cmd("qodercli", "tokenless").args,
            ["plugins", "uninstall", "tokenless", "--scope", "user"]
        );
        assert_eq!(
            build_list_cmd("qodercli").args,
            ["plugins", "list", "--json"]
        );
    }

    #[test]
    fn inventory_requires_all_native_resources() {
        let inventory = parse_plugin_list(LIST_JSON, "tokenless@local")
            .expect("valid inventory")
            .expect("plugin");
        assert!(inventory.enabled);
        assert!(inventory.resources_complete());

        let incomplete = LIST_JSON.replacen("PostToolUse", "PreToolUse", 1);
        assert!(
            !parse_plugin_list(&incomplete, "tokenless@local")
                .expect("valid inventory")
                .expect("plugin")
                .resources_complete()
        );

        let unrelated = LIST_JSON.replace("tokenless:tokenless-stats", "not-tokenless-stats");
        assert!(
            !parse_plugin_list(&unrelated, "tokenless@local")
                .expect("valid inventory")
                .expect("plugin")
                .resources_complete()
        );
    }

    #[test]
    fn inventory_distinguishes_absent_and_invalid_json() {
        assert!(
            parse_plugin_list("[]", "tokenless@local")
                .unwrap()
                .is_none()
        );
        assert!(parse_plugin_list("not json", "tokenless@local").is_err());
    }

    #[test]
    fn version_key_orders_semver_numerically() {
        assert!(version_key("10.0.0") > version_key("9.9.9"));
        assert_eq!(version_key("1.0.0-rc1"), version_key("1.0.0"));
        assert!(is_stable_version_suffix("1.0.0"));
        assert!(!is_stable_version_suffix("1.0.0-rc1"));
    }
}
