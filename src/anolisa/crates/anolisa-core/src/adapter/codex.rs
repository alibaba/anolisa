//! Codex framework driver.
//!
//! Codex installs plugins from a *marketplace*: a directory holding a
//! `.agents/plugins/marketplace.json` manifest plus a symlink to the plugin
//! source. ANOLISA builds a per-user marketplace under
//! `${XDG_DATA_HOME:-~/.local/share}/anolisa/codex-marketplace/`, points a
//! symlink at the component's resource root, then drives the official CLI:
//!
//! ```text
//! codex plugin marketplace add <marketplace_root>
//! codex plugin add <plugin>@<marketplace>
//! ```
//!
//! `disable` reverses this via `codex plugin remove` /
//! `codex plugin marketplace remove` and then removes the marketplace
//! directory (which contains the symlink). All CLI, file, and symlink IO
//! goes through the Manager's [`AdapterOps`](super::driver::AdapterOps).
//!
//! Env contract: `CODEX_BIN` overrides the executable (tests point it at a
//! fake CLI); `XDG_DATA_HOME` relocates the marketplace base.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::AdapterError;
use super::claim::{
    AdapterClaim, CLAIM_SCHEMA_VERSION, ClaimResource, ClaimResourceKind, ClaimStatus, CodexClaim,
    DRIVER_SCHEMA_VERSION, DriverPayload, validate_marketplace_name, validate_plugin_id,
};
use super::driver::{
    AdapterBundle, AdapterCondition, AdapterConditionKind, AdapterStatusReport, AdapterSummary,
    ClaimResourceRef, ConditionStatus, DetectResult, DisableReport, DriverCtx, DriverPlan,
    FrameworkCommand, FrameworkDriver, HostEnv, PreparedEnable, find_binary_in_path,
};
use super::util::{bool_status, cli_failure_reason, digest_tree, display_command, now_iso8601};

/// Default timeout for a Codex CLI invocation.
const CLI_TIMEOUT: Duration = Duration::from_secs(60);

/// Codex-native plugin manifest inside the bundle. Its presence is what
/// makes the resource root a valid codex plugin (the contract may override
/// via `[adapters.bundle].entry`).
const CODEX_PLUGIN_MANIFEST: &str = ".codex-plugin/plugin.json";

/// Resource ids used in Codex receipts.
const RES_MARKETPLACE_DIR: &str = "codex_marketplace_dir";
const RES_SYMLINK: &str = "codex_symlink";
const RES_MARKETPLACE: &str = "codex_marketplace";
const RES_PLUGIN: &str = "codex_plugin";

/// Codex driver. Stateless; all per-operation context arrives via
/// [`DriverCtx`].
pub struct CodexDriver;

impl CodexDriver {
    /// Construct the driver.
    pub fn new() -> Self {
        Self
    }
}

impl Default for CodexDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkDriver for CodexDriver {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn detect(&self, _env: &HostEnv) -> DetectResult {
        match find_binary_in_path(&codex_bin()) {
            Some(path) => DetectResult {
                detected: true,
                reason: format!("codex CLI found at {}", path.display()),
            },
            None => DetectResult {
                detected: false,
                reason: "codex CLI not found on PATH".to_string(),
            },
        }
    }

    fn allowed_external_roots(&self, ctx: &DriverCtx) -> Vec<PathBuf> {
        // The only external root Codex writes is ANOLISA's own namespace in
        // the user's data home; the marketplace dir and the plugin symlink
        // both live below it. The symlink *target* points back at the
        // ANOLISA-owned resource bundle and is validated separately against
        // the trusted layout roots (see `ClaimResourceKind::Symlink`), so it
        // must NOT be authorized via a receipt-derived external root here.
        marketplace_base(ctx.user_home.as_deref())
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
        // Require the codex-native plugin manifest before persisting a
        // receipt, so a malformed bundle fails at read time rather than
        // after `prepare_enable` has written state and `codex plugin add`
        // fails downstream. The contract may name a different entry.
        let manifest = ctx
            .declared_bundle_entry
            .as_deref()
            .unwrap_or(CODEX_PLUGIN_MANIFEST);
        if !root.join(manifest).is_file() {
            return Err(AdapterError::BundleInvalid {
                root: root.clone(),
                reason: format!("codex plugin manifest '{manifest}' missing from resource root"),
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
        let layout = MarketplaceLayout::resolve(bundle, ctx)?;
        let add_cmd = build_marketplace_add_cmd(&layout.root);
        let plugin_cmd = build_plugin_add_cmd(&layout.plugin_ref());
        let actions = vec![
            format!(
                "write codex marketplace manifest {}",
                layout.manifest().display()
            ),
            format!(
                "symlink {} -> {}",
                layout.symlink().display(),
                bundle.resource_root.display()
            ),
            format!("register codex marketplace '{}'", layout.marketplace),
            format!("add codex plugin '{}'", layout.plugin_ref()),
        ];
        Ok(DriverPlan {
            framework: self.name().to_string(),
            component: ctx.component.clone(),
            actions,
            register_command: Some(format!(
                "{} && {}",
                display_command(&add_cmd),
                display_command(&plugin_cmd)
            )),
        })
    }

    fn prepare_enable(
        &self,
        bundle: &AdapterBundle,
        ctx: &DriverCtx,
    ) -> Result<(AdapterClaim, PreparedEnable), AdapterError> {
        let layout = MarketplaceLayout::resolve(bundle, ctx)?;
        validate_plugin_id(&layout.plugin)?;
        validate_marketplace_name(&layout.marketplace)?;

        let resources = vec![
            ClaimResource {
                id: RES_MARKETPLACE_DIR.to_string(),
                purpose: "codex_marketplace_dir".to_string(),
                kind: ClaimResourceKind::ExternalPath {
                    path: layout.root.clone(),
                },
            },
            ClaimResource {
                id: RES_SYMLINK.to_string(),
                purpose: "codex_plugin_symlink".to_string(),
                kind: ClaimResourceKind::Symlink {
                    link: layout.symlink(),
                    target: bundle.resource_root.clone(),
                },
            },
            ClaimResource {
                id: RES_MARKETPLACE.to_string(),
                purpose: "codex_marketplace".to_string(),
                kind: ClaimResourceKind::FrameworkMarketplace {
                    framework: self.name().to_string(),
                    marketplace: layout.marketplace.clone(),
                },
            },
            ClaimResource {
                id: RES_PLUGIN.to_string(),
                purpose: "codex_plugin".to_string(),
                kind: ClaimResourceKind::FrameworkPlugin {
                    framework: self.name().to_string(),
                    plugin_id: layout.plugin.clone(),
                },
            },
        ];

        Ok((
            AdapterClaim {
                claim_schema: CLAIM_SCHEMA_VERSION,
                component: ctx.component.clone(),
                framework: self.name().to_string(),
                plugin_id: Some(layout.plugin.clone()),
                adapter_type: ctx.adapter_type.clone(),
                enabled_at: now_iso8601(),
                resource_root: bundle.resource_root.clone(),
                bundle_digest: bundle.digest.clone(),
                driver_schema: DRIVER_SCHEMA_VERSION,
                status: ClaimStatus::Enabled,
                resources,
                driver_payload: DriverPayload::Codex(CodexClaim {
                    marketplace_dir_resource: RES_MARKETPLACE_DIR.to_string(),
                    symlink_resource: RES_SYMLINK.to_string(),
                    marketplace_resource: RES_MARKETPLACE.to_string(),
                    plugin_resource: RES_PLUGIN.to_string(),
                }),
            },
            PreparedEnable::None,
        ))
    }

    fn apply_enable(
        &self,
        claim: &mut AdapterClaim,
        _prepared: &PreparedEnable,
        ctx: &DriverCtx,
        _progress: &mut dyn super::driver::EnableProgress,
    ) -> Result<(), AdapterError> {
        let layout = MarketplaceLayout::from_claim(claim)?;

        // 1. Write the marketplace manifest and the plugin symlink.
        ctx.ops
            .write_file(&layout.manifest(), marketplace_json(&layout).as_bytes())?;
        ctx.ops
            .create_symlink(&layout.symlink(), &claim.resource_root)?;

        // 2. Register the marketplace, replacing any stale registration so
        //    re-enable is idempotent.
        if marketplace_registered(&layout.marketplace, ctx) {
            let _ = ctx
                .ops
                .run_framework_cli(build_marketplace_remove_cmd(&layout.marketplace));
        }
        let add_cmd = build_marketplace_add_cmd(&layout.root);
        let program = add_cmd.program.clone();
        let output = ctx.ops.run_framework_cli(add_cmd)?;
        if !output.success() {
            return Err(AdapterError::FrameworkCli {
                program,
                reason: cli_failure_reason("plugin marketplace add", &output),
            });
        }

        // 3. Add the plugin from the marketplace.
        let plugin_cmd = build_plugin_add_cmd(&layout.plugin_ref());
        let program = plugin_cmd.program.clone();
        let output = ctx.ops.run_framework_cli(plugin_cmd)?;
        if !output.success() {
            return Err(AdapterError::FrameworkCli {
                program,
                reason: cli_failure_reason("plugin add", &output),
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

        // Symlink presence is a reliable filesystem check independent of
        // the CLI.
        let symlink_ok = claim_symlink(claim)
            .map(|(link, target)| symlink_points_to(&link, &target))
            .unwrap_or(false);
        conditions.push(AdapterCondition {
            kind: AdapterConditionKind::SymlinkPresent,
            status: bool_status(symlink_ok),
            reason: (!symlink_ok).then(|| "plugin symlink missing or retargeted".to_string()),
            resource: Some(ClaimResourceRef {
                id: RES_SYMLINK.to_string(),
            }),
        });

        let layout = MarketplaceLayout::from_claim(claim).ok();
        let (mkt_status, plugin_status) = if !detect.detected {
            conditions.push(AdapterCondition {
                kind: AdapterConditionKind::MarketplaceRegistered,
                status: ConditionStatus::Unknown,
                reason: Some("codex CLI unavailable; cannot verify".to_string()),
                resource: Some(ClaimResourceRef {
                    id: RES_MARKETPLACE.to_string(),
                }),
            });
            conditions.push(AdapterCondition {
                kind: AdapterConditionKind::PluginRegistered,
                status: ConditionStatus::Unknown,
                reason: Some("codex CLI unavailable; cannot verify".to_string()),
                resource: Some(ClaimResourceRef {
                    id: RES_PLUGIN.to_string(),
                }),
            });
            conditions.push(AdapterCondition {
                kind: AdapterConditionKind::VerificationSupported,
                status: ConditionStatus::False,
                reason: Some("codex CLI unavailable".to_string()),
                resource: None,
            });
            (ConditionStatus::Unknown, ConditionStatus::Unknown)
        } else if let Some(layout) = layout {
            let mkt = marketplace_registered(&layout.marketplace, ctx);
            let plugin = plugin_registered(&layout, ctx);
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
                status: bool_status(plugin),
                reason: (!plugin).then(|| "plugin not in `plugin list`".to_string()),
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
            (bool_status(mkt), bool_status(plugin))
        } else {
            conditions.push(AdapterCondition {
                kind: AdapterConditionKind::VerificationSupported,
                status: ConditionStatus::False,
                reason: Some("codex receipt is missing marketplace layout data".to_string()),
                resource: None,
            });
            (ConditionStatus::Unknown, ConditionStatus::Unknown)
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
        let mut messages = Vec::new();
        let mut cleanup_complete = true;
        let layout = MarketplaceLayout::from_claim(claim)?;

        // Framework-side deregistration needs the CLI. Without it the codex
        // config would keep a dangling marketplace/plugin entry, so keep
        // the receipt for a later retry rather than deleting our directory
        // and pretending cleanup finished.
        if find_binary_in_path(&codex_bin()).is_none() {
            return Ok(DisableReport {
                cleanup_complete: false,
                messages: vec![
                    "codex CLI not found on PATH; receipt kept so cleanup can be retried"
                        .to_string(),
                ],
            });
        }

        // Remove the plugin registration. We do not trust the exit code
        // (an already-removed plugin exits non-zero), but we must not
        // silently ignore a real failure either: verify absence via
        // `plugin list` and only then treat it as clean.
        let plugin_ref = layout.plugin_ref();
        let out = ctx
            .ops
            .run_framework_cli(build_plugin_remove_cmd(&plugin_ref))?;
        if out.success() {
            messages.push(format!("removed codex plugin '{plugin_ref}'"));
        } else if !plugin_registered(&layout, ctx) {
            messages.push(format!("codex plugin '{plugin_ref}' already absent"));
        } else {
            cleanup_complete = false;
            messages.push(format!(
                "codex plugin remove failed: {}",
                cli_failure_reason("plugin remove", &out)
            ));
        }

        // Remove the marketplace registration, verifying the same way.
        let out = ctx
            .ops
            .run_framework_cli(build_marketplace_remove_cmd(&layout.marketplace))?;
        if out.success() {
            messages.push(format!(
                "removed codex marketplace '{}'",
                layout.marketplace
            ));
        } else if !marketplace_registered(&layout.marketplace, ctx) {
            messages.push(format!(
                "codex marketplace '{}' already absent",
                layout.marketplace
            ));
        } else {
            cleanup_complete = false;
            messages.push(format!(
                "codex marketplace remove failed: {}",
                cli_failure_reason("plugin marketplace remove", &out)
            ));
        }

        match ctx.ops.remove_tree(&layout.root) {
            Ok(true) => messages.push(format!(
                "removed codex marketplace directory {}",
                layout.root.display()
            )),
            Ok(false) => messages.push(format!(
                "codex marketplace directory {} already absent",
                layout.root.display()
            )),
            Err(err) => {
                cleanup_complete = false;
                messages.push(format!(
                    "failed to remove codex marketplace directory {}: {err}",
                    layout.root.display()
                ));
            }
        }

        Ok(DisableReport {
            cleanup_complete,
            messages,
        })
    }
}

// ---------------------------------------------------------------------------
// Marketplace layout
// ---------------------------------------------------------------------------

/// Resolved per-user codex marketplace layout for one component.
struct MarketplaceLayout {
    /// Marketplace root directory ANOLISA owns.
    root: PathBuf,
    /// Marketplace name registered with codex (`anolisa-<component>`).
    marketplace: String,
    /// Plugin name inside the marketplace.
    plugin: String,
}

impl MarketplaceLayout {
    /// Resolve from a freshly read bundle + context (enable/plan path).
    fn resolve(bundle: &AdapterBundle, ctx: &DriverCtx) -> Result<Self, AdapterError> {
        let root = marketplace_root(ctx.user_home.as_deref())?;
        let plugin = bundle
            .plugin_id
            .clone()
            .unwrap_or_else(|| ctx.component.clone());
        Ok(Self {
            root,
            marketplace: marketplace_name(&ctx.component),
            plugin,
        })
    }

    /// Reconstruct from a persisted receipt (apply/status/disable path).
    /// Reads the validated resource entries so the layout stays anchored to
    /// what the Manager already re-validated.
    fn from_claim(claim: &AdapterClaim) -> Result<Self, AdapterError> {
        let root = claim
            .resource(RES_MARKETPLACE_DIR)
            .and_then(|r| match &r.kind {
                ClaimResourceKind::ExternalPath { path } => Some(path.clone()),
                _ => None,
            })
            .ok_or_else(|| AdapterError::BundleInvalid {
                root: claim.resource_root.clone(),
                reason: "codex receipt has no marketplace directory resource".to_string(),
            })?;
        let marketplace = claim
            .resource(RES_MARKETPLACE)
            .and_then(|r| match &r.kind {
                ClaimResourceKind::FrameworkMarketplace { marketplace, .. } => {
                    Some(marketplace.clone())
                }
                _ => None,
            })
            .ok_or_else(|| AdapterError::BundleInvalid {
                root: claim.resource_root.clone(),
                reason: "codex receipt has no marketplace resource".to_string(),
            })?;
        let plugin = claim
            .resource(RES_PLUGIN)
            .and_then(|r| match &r.kind {
                ClaimResourceKind::FrameworkPlugin { plugin_id, .. } => Some(plugin_id.clone()),
                _ => None,
            })
            .or_else(|| claim.plugin_id.clone())
            .ok_or_else(|| AdapterError::BundleInvalid {
                root: claim.resource_root.clone(),
                reason: "codex receipt has no plugin resource".to_string(),
            })?;
        Ok(Self {
            root,
            marketplace,
            plugin,
        })
    }

    /// `<root>/.agents/plugins/marketplace.json`.
    fn manifest(&self) -> PathBuf {
        self.root
            .join(".agents")
            .join("plugins")
            .join("marketplace.json")
    }

    /// `<root>/<plugin>` — the symlink codex resolves relative to the
    /// marketplace root.
    fn symlink(&self) -> PathBuf {
        self.root.join(&self.plugin)
    }

    /// `<plugin>@<marketplace>` argument for `codex plugin add/remove`.
    fn plugin_ref(&self) -> String {
        format!("{}@{}", self.plugin, self.marketplace)
    }
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// `CODEX_BIN` override, else `codex`.
fn codex_bin() -> String {
    std::env::var("CODEX_BIN")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "codex".to_string())
}

/// Marketplace name for a component: `anolisa-<component>`, matching the
/// name the legacy install script registered.
fn marketplace_name(component: &str) -> String {
    format!("anolisa-{component}")
}

/// ANOLISA data-home base: `${XDG_DATA_HOME:-<user_home>/.local/share}/anolisa`.
fn marketplace_base(user_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let s = xdg.to_string_lossy();
        let trimmed = s.trim_end_matches('/');
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed).join("anolisa"));
        }
    }
    user_home.map(|h| h.join(".local").join("share").join("anolisa"))
}

/// Marketplace root directory, or an error when `$HOME`/`XDG_DATA_HOME`
/// cannot be resolved.
fn marketplace_root(user_home: Option<&Path>) -> Result<PathBuf, AdapterError> {
    marketplace_base(user_home)
        .map(|base| base.join("codex-marketplace"))
        .ok_or_else(|| AdapterError::FrameworkCli {
            program: codex_bin(),
            reason: "cannot resolve codex marketplace dir (no $HOME and no XDG_DATA_HOME)"
                .to_string(),
        })
}

/// Render the codex marketplace manifest JSON. Pure data built from
/// validated identifiers — never an argv or executable path.
fn marketplace_json(layout: &MarketplaceLayout) -> String {
    format!(
        r#"{{
    "name": "{marketplace}",
    "interface": {{
        "displayName": "ANOLISA {plugin}"
    }},
    "plugins": [
        {{
            "name": "{plugin}",
            "source": {{
                "source": "local",
                "path": "./{plugin}"
            }},
            "policy": {{
                "installation": "AVAILABLE"
            }},
            "category": "developer-tools"
        }}
    ]
}}
"#,
        marketplace = layout.marketplace,
        plugin = layout.plugin,
    )
}

fn base_cmd(args: Vec<String>) -> FrameworkCommand {
    FrameworkCommand {
        program: codex_bin(),
        args,
        stdin: None,
        env_set: Vec::new(),
        env_remove: Vec::new(),
        path_prepend: Vec::new(),
        timeout: CLI_TIMEOUT,
    }
}

fn build_marketplace_add_cmd(root: &Path) -> FrameworkCommand {
    base_cmd(vec![
        "plugin".to_string(),
        "marketplace".to_string(),
        "add".to_string(),
        root.to_string_lossy().into_owned(),
    ])
}

fn build_marketplace_remove_cmd(marketplace: &str) -> FrameworkCommand {
    base_cmd(vec![
        "plugin".to_string(),
        "marketplace".to_string(),
        "remove".to_string(),
        marketplace.to_string(),
    ])
}

fn build_marketplace_list_cmd() -> FrameworkCommand {
    base_cmd(vec![
        "plugin".to_string(),
        "marketplace".to_string(),
        "list".to_string(),
    ])
}

fn build_plugin_add_cmd(plugin_ref: &str) -> FrameworkCommand {
    base_cmd(vec![
        "plugin".to_string(),
        "add".to_string(),
        plugin_ref.to_string(),
    ])
}

fn build_plugin_remove_cmd(plugin_ref: &str) -> FrameworkCommand {
    base_cmd(vec![
        "plugin".to_string(),
        "remove".to_string(),
        plugin_ref.to_string(),
    ])
}

fn build_plugin_list_cmd() -> FrameworkCommand {
    base_cmd(vec!["plugin".to_string(), "list".to_string()])
}

/// True when `codex plugin marketplace list` reports `marketplace`.
fn marketplace_registered(marketplace: &str, ctx: &DriverCtx) -> bool {
    match ctx.ops.run_framework_cli(build_marketplace_list_cmd()) {
        Ok(output) if output.success() => list_contains_token(&output.stdout, marketplace),
        _ => false,
    }
}

/// True when `codex plugin list` reports the plugin (by bare name or
/// `plugin@marketplace` ref).
fn plugin_registered(layout: &MarketplaceLayout, ctx: &DriverCtx) -> bool {
    match ctx.ops.run_framework_cli(build_plugin_list_cmd()) {
        Ok(output) if output.success() => {
            list_contains_token(&output.stdout, &layout.plugin)
                || list_contains_token(&output.stdout, &layout.plugin_ref())
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

/// True when `link` is a symlink resolving to `target`.
fn symlink_points_to(link: &Path, target: &Path) -> bool {
    std::fs::read_link(link)
        .map(|dest| dest == target)
        .unwrap_or(false)
}

/// Extract `(link, target)` from a receipt's symlink resource.
fn claim_symlink(claim: &AdapterClaim) -> Option<(PathBuf, PathBuf)> {
    claim.resource(RES_SYMLINK).and_then(|r| match &r.kind {
        ClaimResourceKind::Symlink { link, target } => Some((link.clone(), target.clone())),
        _ => None,
    })
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
    fn marketplace_name_is_component_scoped() {
        assert_eq!(marketplace_name("tokenless"), "anolisa-tokenless");
    }

    #[test]
    fn plugin_ref_combines_validated_parts() {
        let layout = MarketplaceLayout {
            root: PathBuf::from("/home/u/.local/share/anolisa/codex-marketplace"),
            marketplace: "anolisa-tokenless".to_string(),
            plugin: "tokenless".to_string(),
        };
        assert_eq!(layout.plugin_ref(), "tokenless@anolisa-tokenless");
        assert_eq!(
            layout.symlink(),
            PathBuf::from("/home/u/.local/share/anolisa/codex-marketplace/tokenless")
        );
        assert_eq!(
            layout.manifest(),
            PathBuf::from(
                "/home/u/.local/share/anolisa/codex-marketplace/.agents/plugins/marketplace.json"
            )
        );
    }

    #[test]
    fn marketplace_json_is_valid_and_references_plugin() {
        let layout = MarketplaceLayout {
            root: PathBuf::from("/tmp/mkt"),
            marketplace: "anolisa-tokenless".to_string(),
            plugin: "tokenless".to_string(),
        };
        let json = marketplace_json(&layout);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(parsed["name"], "anolisa-tokenless");
        assert_eq!(parsed["plugins"][0]["name"], "tokenless");
        assert_eq!(parsed["plugins"][0]["source"]["path"], "./tokenless");
    }

    #[test]
    fn add_and_plugin_cmd_shapes() {
        let add = build_marketplace_add_cmd(Path::new("/tmp/mkt"));
        assert_eq!(add.program, "codex");
        assert_eq!(add.args, vec!["plugin", "marketplace", "add", "/tmp/mkt"]);
        let plugin = build_plugin_add_cmd("tokenless@anolisa-tokenless");
        assert_eq!(
            plugin.args,
            vec!["plugin", "add", "tokenless@anolisa-tokenless"]
        );
        let rm = build_plugin_remove_cmd("tokenless@anolisa-tokenless");
        assert_eq!(
            rm.args,
            vec!["plugin", "remove", "tokenless@anolisa-tokenless"]
        );
        let mrm = build_marketplace_remove_cmd("anolisa-tokenless");
        assert_eq!(
            mrm.args,
            vec!["plugin", "marketplace", "remove", "anolisa-tokenless"]
        );
    }

    #[test]
    fn list_contains_token_matches_whole_word() {
        assert!(list_contains_token(
            "anolisa-tokenless  local\n",
            "anolisa-tokenless"
        ));
        assert!(!list_contains_token(
            "anolisa-tokenless-x\n",
            "anolisa-tokenless"
        ));
        assert!(!list_contains_token("", "anolisa-tokenless"));
    }

    #[test]
    fn marketplace_base_fallback_without_xdg() {
        // Only the fallback branch is asserted, and only when the ambient
        // env would exercise it — mutating XDG_DATA_HOME here would race
        // with parallel FsLayout tests in the same binary.
        if std::env::var_os("XDG_DATA_HOME").is_some() {
            return;
        }
        assert_eq!(
            marketplace_base(Some(Path::new("/home/u"))),
            Some(PathBuf::from("/home/u/.local/share/anolisa"))
        );
    }
}
