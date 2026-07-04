//! Claude Code framework driver.
//!
//! Claude Code v2 installs plugins from a registered marketplace. ANOLISA
//! exposes the component's resource root (which ships
//! `.claude-plugin/marketplace.json` + `.claude-plugin/plugin.json`) as a
//! single-plugin marketplace named `anolisa`, then installs the plugin via
//! the official CLI:
//!
//! ```text
//! claude plugin validate <resource_root>
//! claude plugin marketplace add <resource_root>
//! claude plugin install <plugin>@anolisa
//! ```
//!
//! `disable` reverses this with `claude plugin uninstall` /
//! `claude plugin marketplace remove`. ANOLISA never edits
//! `~/.claude/settings.json` directly: when the CLI is unavailable it
//! reports cleanup as incomplete and keeps the receipt, rather than
//! hand-patching a file it does not own.
//!
//! Env contract: `CLAUDE_BIN` overrides the executable (tests point it at a
//! fake CLI).

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::AdapterError;
use super::claim::{
    AdapterClaim, CLAIM_SCHEMA_VERSION, ClaimResource, ClaimResourceKind, ClaimStatus,
    ClaudeCodeClaim, DRIVER_SCHEMA_VERSION, DriverPayload, validate_marketplace_name,
    validate_plugin_id,
};
use super::driver::{
    AdapterBundle, AdapterCondition, AdapterConditionKind, AdapterStatusReport, AdapterSummary,
    ClaimResourceRef, ConditionStatus, DetectResult, DisableReport, DriverCtx, DriverPlan,
    FrameworkCommand, FrameworkDriver, HostEnv, find_binary_in_path,
};
use super::util::{bool_status, cli_failure_reason, digest_tree, display_command, now_iso8601};

/// Default timeout for a Claude Code CLI invocation.
const CLI_TIMEOUT: Duration = Duration::from_secs(60);

/// Marketplace name ANOLISA registers with Claude Code. Matches the name
/// declared in every component's `.claude-plugin/marketplace.json`.
///
/// Ownership constraint: `anolisa` is a shared, ANOLISA-owned marketplace
/// by convention — all ANOLISA components publish into it, and this driver
/// assumes any marketplace of that name is ANOLISA's to add/remove. If a
/// user has an unrelated marketplace also named `anolisa`, enable would
/// re-point it and disable would remove it. That collision is accepted as
/// out of scope: the name is reserved for ANOLISA. A future multi-owner
/// design would need to verify the marketplace *source* before removal.
const MARKETPLACE_NAME: &str = "anolisa";

/// Native marketplace manifest inside a Claude Code plugin bundle.
const MARKETPLACE_MANIFEST: &str = ".claude-plugin/marketplace.json";
/// Native plugin manifest inside a Claude Code plugin bundle.
const PLUGIN_MANIFEST: &str = ".claude-plugin/plugin.json";

/// Resource ids used in Claude Code receipts.
const RES_MARKETPLACE: &str = "claude_code_marketplace";
const RES_PLUGIN: &str = "claude_code_plugin";

/// Claude Code driver. Stateless; all per-operation context arrives via
/// [`DriverCtx`].
pub struct ClaudeCodeDriver;

impl ClaudeCodeDriver {
    /// Construct the driver.
    pub fn new() -> Self {
        Self
    }
}

impl Default for ClaudeCodeDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkDriver for ClaudeCodeDriver {
    fn name(&self) -> &'static str {
        "claude-code"
    }

    fn detect(&self, _env: &HostEnv) -> DetectResult {
        match find_binary_in_path(&claude_bin()) {
            Some(path) => DetectResult {
                detected: true,
                reason: format!("claude CLI found at {}", path.display()),
            },
            None => DetectResult {
                detected: false,
                reason: "claude CLI not found on PATH".to_string(),
            },
        }
    }

    fn allowed_external_roots(&self, _ctx: &DriverCtx) -> Vec<PathBuf> {
        // Claude Code owns its own registry and settings; ANOLISA writes no
        // external filesystem paths of its own (the marketplace source is
        // the shared resource root, added by reference via the CLI).
        Vec::new()
    }

    fn read_bundle(&self, ctx: &DriverCtx) -> Result<AdapterBundle, AdapterError> {
        let root = &ctx.resource_root;
        if !root.is_dir() {
            return Err(AdapterError::BundleInvalid {
                root: root.clone(),
                reason: "resource root does not exist or is not a directory".to_string(),
            });
        }
        if !root.join(MARKETPLACE_MANIFEST).is_file() {
            return Err(AdapterError::BundleInvalid {
                root: root.clone(),
                reason: format!(
                    "Claude Code marketplace manifest '{MARKETPLACE_MANIFEST}' missing"
                ),
            });
        }
        if !root.join(PLUGIN_MANIFEST).is_file() {
            return Err(AdapterError::BundleInvalid {
                root: root.clone(),
                reason: format!(
                    "Claude Code plugin manifest '{PLUGIN_MANIFEST}' missing (run: make stamp-adapter-templates)"
                ),
            });
        }
        let plugin_id = Some(
            ctx.declared_plugin_id
                .clone()
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| ctx.component.clone()),
        );
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
        let plugin = plugin_name(bundle, ctx);
        let plugin_ref = plugin_ref(&plugin);
        let install_cmd = build_plugin_install_cmd(&plugin_ref);
        let actions = vec![
            format!(
                "validate Claude Code plugin at {}",
                bundle.resource_root.display()
            ),
            format!(
                "register Claude Code marketplace '{MARKETPLACE_NAME}' from {}",
                bundle.resource_root.display()
            ),
            format!("install Claude Code plugin '{plugin_ref}'"),
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
        validate_marketplace_name(MARKETPLACE_NAME)?;

        let resources = vec![
            ClaimResource {
                id: RES_MARKETPLACE.to_string(),
                purpose: "claude_code_marketplace".to_string(),
                kind: ClaimResourceKind::FrameworkMarketplace {
                    framework: self.name().to_string(),
                    marketplace: MARKETPLACE_NAME.to_string(),
                },
            },
            ClaimResource {
                id: RES_PLUGIN.to_string(),
                purpose: "claude_code_plugin".to_string(),
                kind: ClaimResourceKind::FrameworkPlugin {
                    framework: self.name().to_string(),
                    plugin_id: plugin.clone(),
                },
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
            driver_payload: DriverPayload::ClaudeCode(ClaudeCodeClaim {
                marketplace_resource: RES_MARKETPLACE.to_string(),
                plugin_resource: RES_PLUGIN.to_string(),
            }),
        })
    }

    fn apply_enable(&self, claim: &AdapterClaim, ctx: &DriverCtx) -> Result<(), AdapterError> {
        let plugin = claim_plugin(claim).ok_or_else(|| AdapterError::BundleInvalid {
            root: claim.resource_root.clone(),
            reason: "claude-code receipt has no plugin resource".to_string(),
        })?;

        // 1. Validate the bundle via the official CLI — same gate `install`
        //    hits, but surfaces a cleaner error here.
        let validate_cmd = build_validate_cmd(&claim.resource_root);
        let program = validate_cmd.program.clone();
        let output = ctx.ops.run_framework_cli(validate_cmd)?;
        if !output.success() {
            return Err(AdapterError::FrameworkCli {
                program,
                reason: cli_failure_reason("plugin validate", &output),
            });
        }

        // 2. Register the marketplace (idempotent: skip when already listed
        //    so a re-enable does not error on a duplicate source).
        if !marketplace_registered(ctx) {
            let add_cmd = build_marketplace_add_cmd(&claim.resource_root);
            let program = add_cmd.program.clone();
            let output = ctx.ops.run_framework_cli(add_cmd)?;
            if !output.success() {
                return Err(AdapterError::FrameworkCli {
                    program,
                    reason: cli_failure_reason("plugin marketplace add", &output),
                });
            }
        }

        // 3. Install the plugin from the marketplace.
        let install_cmd = build_plugin_install_cmd(&plugin_ref(&plugin));
        let program = install_cmd.program.clone();
        let output = ctx.ops.run_framework_cli(install_cmd)?;
        if !output.success() {
            return Err(AdapterError::FrameworkCli {
                program,
                reason: cli_failure_reason("plugin install", &output),
            });
        }
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

        let plugin = claim_plugin(claim);
        let (mkt_status, plugin_status) = if !detect.detected {
            conditions.push(AdapterCondition {
                kind: AdapterConditionKind::MarketplaceRegistered,
                status: ConditionStatus::Unknown,
                reason: Some("claude CLI unavailable; cannot verify".to_string()),
                resource: Some(ClaimResourceRef {
                    id: RES_MARKETPLACE.to_string(),
                }),
            });
            conditions.push(AdapterCondition {
                kind: AdapterConditionKind::PluginRegistered,
                status: ConditionStatus::Unknown,
                reason: Some("claude CLI unavailable; cannot verify".to_string()),
                resource: Some(ClaimResourceRef {
                    id: RES_PLUGIN.to_string(),
                }),
            });
            conditions.push(AdapterCondition {
                kind: AdapterConditionKind::VerificationSupported,
                status: ConditionStatus::False,
                reason: Some("claude CLI unavailable".to_string()),
                resource: None,
            });
            (ConditionStatus::Unknown, ConditionStatus::Unknown)
        } else {
            let mkt = marketplace_registered(ctx);
            let plugin_ok = plugin
                .as_deref()
                .map(|p| plugin_registered(p, ctx))
                .unwrap_or(false);
            conditions.push(AdapterCondition {
                kind: AdapterConditionKind::MarketplaceRegistered,
                status: bool_status(mkt),
                reason: (!mkt).then(|| "marketplace not in `plugin marketplace list`".to_string()),
                resource: Some(ClaimResourceRef {
                    id: RES_MARKETPLACE.to_string(),
                }),
            });
            conditions.push(AdapterCondition {
                kind: AdapterConditionKind::PluginRegistered,
                status: bool_status(plugin_ok),
                reason: (!plugin_ok).then(|| "plugin not in `plugin list`".to_string()),
                resource: Some(ClaimResourceRef {
                    id: RES_PLUGIN.to_string(),
                }),
            });
            conditions.push(AdapterCondition {
                kind: AdapterConditionKind::VerificationSupported,
                status: ConditionStatus::True,
                reason: None,
                resource: None,
            });
            (bool_status(mkt), bool_status(plugin_ok))
        };

        let summary = summarize(claim.status, detect.detected, mkt_status, plugin_status);
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
        // Deregistration is only possible through the CLI. ANOLISA must not
        // hand-edit ~/.claude/settings.json, so without the CLI we report
        // cleanup as incomplete and keep the receipt for a later retry.
        if find_binary_in_path(&claude_bin()).is_none() {
            return Ok(DisableReport {
                cleanup_complete: false,
                messages: vec![
                    "claude CLI not found on PATH; receipt kept (ANOLISA will not hand-edit settings.json)"
                        .to_string(),
                ],
            });
        }

        let mut messages = Vec::new();
        let mut cleanup_complete = true;

        if let Some(plugin) = claim_plugin(claim) {
            let cmd = build_plugin_uninstall_cmd(&plugin_ref(&plugin));
            let output = ctx.ops.run_framework_cli(cmd)?;
            if output.success() {
                messages.push(format!(
                    "uninstalled claude-code plugin '{}'",
                    plugin_ref(&plugin)
                ));
            } else {
                cleanup_complete = false;
                messages.push(format!(
                    "claude plugin uninstall failed: {}",
                    cli_failure_reason("plugin uninstall", &output)
                ));
            }
        } else {
            messages.push("receipt records no plugin to uninstall".to_string());
        }

        let cmd = build_marketplace_remove_cmd();
        let output = ctx.ops.run_framework_cli(cmd)?;
        if output.success() {
            messages.push(format!(
                "removed claude-code marketplace '{MARKETPLACE_NAME}'"
            ));
        } else {
            cleanup_complete = false;
            messages.push(format!(
                "claude plugin marketplace remove failed: {}",
                cli_failure_reason("plugin marketplace remove", &output)
            ));
        }

        Ok(DisableReport {
            cleanup_complete,
            messages,
        })
    }
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// `CLAUDE_BIN` override, else `claude`.
fn claude_bin() -> String {
    std::env::var("CLAUDE_BIN")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "claude".to_string())
}

/// Plugin name for the receipt: declared plugin id, else component.
fn plugin_name(bundle: &AdapterBundle, ctx: &DriverCtx) -> String {
    bundle
        .plugin_id
        .clone()
        .unwrap_or_else(|| ctx.component.clone())
}

/// `<plugin>@anolisa` argument for `claude plugin install/uninstall`.
fn plugin_ref(plugin: &str) -> String {
    format!("{plugin}@{MARKETPLACE_NAME}")
}

/// Extract the plugin name from a receipt's plugin resource.
fn claim_plugin(claim: &AdapterClaim) -> Option<String> {
    claim
        .resource(RES_PLUGIN)
        .and_then(|r| match &r.kind {
            ClaimResourceKind::FrameworkPlugin { plugin_id, .. } => Some(plugin_id.clone()),
            _ => None,
        })
        .or_else(|| claim.plugin_id.clone())
}

fn base_cmd(args: Vec<String>) -> FrameworkCommand {
    FrameworkCommand {
        program: claude_bin(),
        args,
        env_set: Vec::new(),
        env_remove: Vec::new(),
        path_prepend: Vec::new(),
        timeout: CLI_TIMEOUT,
    }
}

fn build_validate_cmd(resource_root: &Path) -> FrameworkCommand {
    base_cmd(vec![
        "plugin".to_string(),
        "validate".to_string(),
        resource_root.to_string_lossy().into_owned(),
    ])
}

fn build_marketplace_add_cmd(resource_root: &Path) -> FrameworkCommand {
    base_cmd(vec![
        "plugin".to_string(),
        "marketplace".to_string(),
        "add".to_string(),
        resource_root.to_string_lossy().into_owned(),
    ])
}

fn build_marketplace_remove_cmd() -> FrameworkCommand {
    base_cmd(vec![
        "plugin".to_string(),
        "marketplace".to_string(),
        "remove".to_string(),
        MARKETPLACE_NAME.to_string(),
    ])
}

fn build_marketplace_list_cmd() -> FrameworkCommand {
    base_cmd(vec![
        "plugin".to_string(),
        "marketplace".to_string(),
        "list".to_string(),
    ])
}

fn build_plugin_install_cmd(plugin_ref: &str) -> FrameworkCommand {
    base_cmd(vec![
        "plugin".to_string(),
        "install".to_string(),
        plugin_ref.to_string(),
    ])
}

fn build_plugin_uninstall_cmd(plugin_ref: &str) -> FrameworkCommand {
    base_cmd(vec![
        "plugin".to_string(),
        "uninstall".to_string(),
        plugin_ref.to_string(),
    ])
}

fn build_plugin_list_cmd() -> FrameworkCommand {
    base_cmd(vec!["plugin".to_string(), "list".to_string()])
}

/// True when `claude plugin marketplace list` reports the ANOLISA
/// marketplace.
fn marketplace_registered(ctx: &DriverCtx) -> bool {
    match ctx.ops.run_framework_cli(build_marketplace_list_cmd()) {
        Ok(output) if output.success() => list_contains_token(&output.stdout, MARKETPLACE_NAME),
        _ => false,
    }
}

/// True when `claude plugin list` reports the plugin (bare name or
/// `plugin@anolisa` ref).
fn plugin_registered(plugin: &str, ctx: &DriverCtx) -> bool {
    match ctx.ops.run_framework_cli(build_plugin_list_cmd()) {
        Ok(output) if output.success() => {
            list_contains_token(&output.stdout, plugin)
                || list_contains_token(&output.stdout, &plugin_ref(plugin))
        }
        _ => false,
    }
}

/// True when `token` appears as a whole whitespace-delimited word on any
/// line of `stdout`.
fn list_contains_token(stdout: &str, token: &str) -> bool {
    stdout
        .lines()
        .any(|line| line.split_whitespace().any(|t| t == token))
}

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
/// both marketplace and plugin verified present.
fn summarize(
    claim_status: ClaimStatus,
    detected: bool,
    marketplace: ConditionStatus,
    plugin: ConditionStatus,
) -> AdapterSummary {
    if claim_status == ClaimStatus::CleanupFailed {
        return AdapterSummary::CleanupFailed;
    }
    if !detected {
        return AdapterSummary::Degraded;
    }
    match (marketplace, plugin) {
        (ConditionStatus::True, ConditionStatus::True) => AdapterSummary::Healthy,
        (ConditionStatus::False, _) | (_, ConditionStatus::False) => AdapterSummary::Degraded,
        _ => AdapterSummary::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_ref_uses_anolisa_marketplace() {
        assert_eq!(plugin_ref("tokenless"), "tokenless@anolisa");
    }

    #[test]
    fn cmd_shapes() {
        assert_eq!(
            build_validate_cmd(Path::new("/data/cc")).args,
            vec!["plugin", "validate", "/data/cc"]
        );
        assert_eq!(
            build_marketplace_add_cmd(Path::new("/data/cc")).args,
            vec!["plugin", "marketplace", "add", "/data/cc"]
        );
        assert_eq!(
            build_plugin_install_cmd("tokenless@anolisa").args,
            vec!["plugin", "install", "tokenless@anolisa"]
        );
        assert_eq!(
            build_plugin_uninstall_cmd("tokenless@anolisa").args,
            vec!["plugin", "uninstall", "tokenless@anolisa"]
        );
        assert_eq!(
            build_marketplace_remove_cmd().args,
            vec!["plugin", "marketplace", "remove", "anolisa"]
        );
    }

    #[test]
    fn read_bundle_requires_both_manifests() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("claude-code");
        std::fs::create_dir_all(root.join(".claude-plugin")).expect("mkdir");
        let layout = anolisa_platform::fs_layout::FsLayout::user(PathBuf::from("/tmp/cc-home"));

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
        }
        let ops = StubOps;
        let mk_ctx = |root: &Path| DriverCtx {
            component: "tokenless".to_string(),
            framework: "claude-code".to_string(),
            layout: &layout,
            resource_root: root.to_path_buf(),
            user_home: Some(PathBuf::from("/tmp/cc-home")),
            declared_plugin_id: Some("tokenless".to_string()),
            adapter_type: Some("plugin".to_string()),
            declared_skills: Vec::new(),
            declared_config: Vec::new(),
            declared_bundle_entry: None,
            dry_run: true,
            ops: &ops,
        };
        let driver = ClaudeCodeDriver::new();

        // marketplace.json only -> still missing plugin.json.
        std::fs::write(root.join(MARKETPLACE_MANIFEST), b"{}").expect("write");
        let err = driver
            .read_bundle(&mk_ctx(&root))
            .expect_err("plugin.json missing must fail");
        assert!(matches!(err, AdapterError::BundleInvalid { .. }));

        // Both present -> ok.
        std::fs::write(root.join(PLUGIN_MANIFEST), b"{}").expect("write");
        let bundle = driver.read_bundle(&mk_ctx(&root)).expect("both present");
        assert_eq!(bundle.plugin_id.as_deref(), Some("tokenless"));
    }
}
