//! `enable --dry-run` plan contract.
//!
//! [`plan_enable`] is a pure, side-effect-free function that takes a
//! catalog snapshot, distribution index, environment facts, install-mode
//! string, and a capability name and returns an [`EnablePlan`] describing
//! what `anolisa enable <cap>` *would* do. It does NOT touch the
//! filesystem, write state, download artifacts, or modify any service —
//! the plan is data only.
//!
//! Status vocabulary (`PlanStatus`):
//!
//! * `Ready` - every precheck passed and an artifact resolved for every
//!   component.
//! * `Degraded` - at least one non-fatal warning (e.g. version drift, an
//!   optional probe is unknown). The plan can still execute.
//! * `Blocked` - at least one blocker: env mismatch, missing component
//!   manifest, install-mode unsupported, no matching artifact. The plan
//!   cannot execute as-is.
//!
//! `PlanError` is reserved for inputs that prevent us from producing any
//! plan at all — currently only an unknown capability name. Everything
//! else surfaces inside the plan via per-check / per-component status.

use serde::Serialize;

use anolisa_env::EnvFacts;
use anolisa_platform::fs_layout::FsLayout;

use crate::catalog::Catalog;
use crate::contract_lint::{LintFinding, LintSeverity, lint_capability};
use crate::distribution::{
    ArtifactType, DistributionEntry, DistributionIndex, ResolveError, ResolveQuery,
};
use crate::health::CheckSpec;
use crate::install_runner::SUPPORTED_ARTIFACT_TYPES;
use crate::manifest::{
    DistributionSelector, EnvRequirements, InstallCapabilitySpec, InstallFileSpec,
};

/// JSON schema version for the [`EnablePlan`] payload. Bump whenever the
/// wire shape changes in a way that consumers must observe.
pub const PLAN_SCHEMA_VERSION: u32 = 1;

/// Errors that prevent generating any plan. Surface as INVALID_ARGUMENT.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    /// Requested capability name is absent from the catalog, so there is
    /// no meaningful plan payload to return.
    #[error("capability '{0}' is not in the catalog")]
    UnknownCapability(String),
}

/// Overall plan status, in machine-friendly snake_case for `--json`.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    /// All mandatory checks passed and every component has an artifact.
    Ready,
    /// Execution is allowed, but warnings should be shown to the user.
    Degraded,
    /// At least one hard precheck failed; execution must refuse.
    Blocked,
}

impl PlanStatus {
    /// Wire label, mirrors the serde discriminator so human renderers can
    /// reuse the same vocabulary without round-tripping through JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::Blocked => "blocked",
        }
    }
}

/// Plan envelope: the structured preview of what `enable` would execute.
#[derive(Debug, Serialize)]
pub struct EnablePlan {
    /// Schema version for consumers that parse `--json`.
    pub schema_version: u32,
    /// Capability requested by the user.
    pub capability: String,
    /// Capability stability channel copied from its manifest.
    pub stability: String,
    /// Install mode the plan was built against.
    pub install_mode: String,
    /// `true` for preview-only invocations.
    pub dry_run: bool,
    /// Structural readiness of the plan, independent of CLI execution
    /// policy.
    pub status: PlanStatus,
    /// First hard blocker, present only when [`status`](Self::status) is
    /// [`PlanStatus::Blocked`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    /// Per-component install preview in execution order.
    pub components: Vec<ComponentPlan>,
    /// Host and mode checks that affected plan status.
    pub prechecks: Vec<PrecheckResult>,
    /// Environment facts relevant to component selection.
    pub env_facts: EnvFactsSummary,
    /// Resolved target layout used for template substitution.
    pub layout: LayoutSummary,
    /// Human-facing warnings accumulated while planning.
    pub warnings: Vec<String>,
    /// Human-facing remediation hints.
    pub advice: Vec<String>,
    /// Suggested next CLI commands.
    pub next_actions: Vec<String>,
    /// Structured manifest/distribution-index lint findings. Empty when
    /// the contract is clean. Each finding carries `{capability,
    /// component, severity, code, message}`; errors push the plan to
    /// `Blocked` and warnings degrade it (see [`crate::contract_lint`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lint: Vec<LintFinding>,
    /// CLI-side execute permission for this plan.
    ///
    /// `None` is the planner's default — `plan_enable` knows nothing about
    /// execution policy, so the field is opt-in metadata injected by the
    /// caller (the CLI). When `Some`, this is authoritative for "would
    /// real execute be allowed from this binary"; [`status`](Self::status)
    /// only describes the plan's structural readiness.
    ///
    /// Consumers should prefer `execute_gate.allowed` over scanning
    /// [`warnings`](Self::warnings) for the legacy `"execute gate:"` prefix
    /// — the warning is human-facing, the gate is structured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execute_gate: Option<ExecuteGate>,
}

/// CLI execute-permission record for an [`EnablePlan`].
///
/// Surfaced as a sibling of [`PlanStatus`] specifically so the plan's
/// readiness semantics stay clean: a capability can be `status = ready`
/// (the plan would execute cleanly) while `execute_gate.allowed = false`
/// (the CLI declines to run it because the execution policy has not
/// graduated this surface yet).
///
/// Set this once, from the CLI, after `plan_enable` returns. The plan
/// itself never inspects it.
#[derive(Debug, Serialize)]
pub struct ExecuteGate {
    /// `true` when real-execute would be permitted, `false` when the CLI
    /// has declined (typically because the policy file has no graduating
    /// entry for this capability).
    pub allowed: bool,
    /// Free-form human-readable reason the gate is closed. `None` when
    /// `allowed = true` — there is no need to explain a permitted gate.
    /// Mirrors the text in [`EnablePlan::warnings`] so consumers that
    /// only read one channel still see the same explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Per-component slice of the plan. Mirrors what the install runner
/// would do for one component without naming the runner explicitly.
#[derive(Debug, Serialize)]
pub struct ComponentPlan {
    /// Stable component name from the manifest.
    pub name: String,
    /// Component manifest version when the manifest declared one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_version: Option<String>,
    /// Readiness of this component slice.
    pub status: PlanStatus,
    /// Component-local blocker when this slice is not executable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    /// Resolved artifact chosen for this component.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactPlan>,
    /// Service units declared by the manifest.
    pub services: Vec<String>,
    /// Structured `install.files` entries, kept verbatim so users see
    /// manifest intent before any layout substitution happens.
    pub files: Vec<InstallFileSpec>,
    /// `files` install paths rendered against the resolved [`FsLayout`] (e.g.
    /// `"/usr/local/bin/agentsight"`). Same length and order as `files`.
    pub resolved_files: Vec<String>,
    /// Linux capability assignments requested by the manifest.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<InstallCapabilitySpec>,
    /// `true` when executing this component requires elevated privileges.
    pub requires_privilege: bool,
    /// Component-level host requirements that influenced prechecks.
    pub env_requirements: EnvRequirements,
    /// Post-install health probe carried from the manifest so the executor
    /// can verify the component without re-loading the catalog: the declared
    /// `[component.health_check]`, or a synthesized `binary_version` over the
    /// first executable file. `None` when neither is available — see
    /// [`crate::manifest::ComponentManifest::health_spec`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check: Option<CheckSpec>,
}

/// Concrete artifact the resolver picked for a component.
#[derive(Debug, Serialize)]
pub struct ArtifactPlan {
    /// Distribution-index artifact type accepted by the install runner.
    pub artifact_type: String,
    /// Distribution backend label used to resolve the artifact.
    pub backend: String,
    /// Artifact version selected for the target environment.
    pub version: String,
    /// Download URL copied from the distribution index.
    pub url: String,
    /// Expected artifact sha256. Missing values block real execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Optional signature reference preserved for future verification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Provider-specific artifact id preserved for diagnostics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    /// sha256 of the registry `meta.toml` used at planning time. When set,
    /// real-execute verifies the artifact's `.anolisa/component.toml` matches
    /// byte-for-byte (contract I3) before installing anything — the T1.4
    /// "planned A, installing B" guard. The planner itself leaves this `None`
    /// (it is IO-free); the CLI fills it after `RegistryClient::fetch_meta`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta_sha256: Option<String>,
}

/// Single precheck outcome. `status` is one of `"ok" | "warn" | "fail"`.
#[derive(Debug, Serialize, Clone)]
pub struct PrecheckResult {
    /// Stable precheck identifier shown by the CLI.
    pub name: String,
    /// Wire status: `"ok"`, `"warn"`, or `"fail"`.
    pub status: String,
    /// Expected condition in human-readable form.
    pub expected: String,
    /// Actual host or manifest value observed by the planner.
    pub actual: String,
    /// Additional diagnostic when status is not self-explanatory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Subset of [`EnvFacts`] that is meaningful for capability enabling.
#[derive(Debug, Serialize)]
pub struct EnvFactsSummary {
    /// Operating-system label used by distribution matching.
    pub os: String,
    /// CPU architecture label used by distribution matching.
    pub arch: String,
    /// Detected libc family, when relevant to artifact selection.
    pub libc: Option<String>,
    /// Package ecosystem such as `rpm` or `deb`.
    pub pkg_base: Option<String>,
    /// Kernel release observed during planning.
    pub kernel: Option<String>,
    /// Whether BTF metadata was detected.
    pub btf: Option<bool>,
    /// Whether CAP_BPF capability is available to the current context.
    pub cap_bpf: Option<bool>,
}

/// Resolved filesystem layout paths the install would target.
#[derive(Debug, Serialize)]
pub struct LayoutSummary {
    /// Directory where executable files would be installed.
    pub bin_dir: String,
    /// Directory where ANOLISA-owned config files would be installed.
    pub etc_dir: String,
    /// Directory containing state such as `installed.toml`.
    pub state_dir: String,
    /// Directory containing central and component logs.
    pub log_dir: String,
    /// Directory searched for manifest overlays.
    pub manifests_overlay: String,
}

/// Build an [`EnablePlan`] for `capability_name`. Performs no IO.
///
/// On unknown capabilities returns `PlanError::UnknownCapability`. Every
/// other failure mode (env mismatch, missing component manifest, install
/// mode unsupported, no matching artifact) is reported in the plan's
/// `status` / `blocked_reason` / per-component `status` so the caller can
/// render a single coherent dry-run output.
pub fn plan_enable(
    catalog: &Catalog,
    dist_index: &DistributionIndex,
    env: &EnvFacts,
    install_mode: &str,
    layout: &FsLayout,
    capability_name: &str,
) -> Result<EnablePlan, PlanError> {
    let cap = catalog
        .capability(capability_name)
        .ok_or_else(|| PlanError::UnknownCapability(capability_name.to_string()))?;

    let mut warnings: Vec<String> = Vec::new();
    let advice: Vec<String> = Vec::new();

    // Lint first so contract breakage surfaces as `lint: [...]` in the
    // payload regardless of whether downstream prechecks succeed. We
    // only fold *errors* into the human `warnings` channel — lint
    // warnings are surfaced via the structured `lint` field and via
    // status (Degraded), and we don't want them to double up in the
    // human renderer.
    let lint = lint_capability(catalog, dist_index, layout, capability_name);
    let lint_has_error = lint.iter().any(|f| f.severity == LintSeverity::Error);
    let lint_has_warning = lint.iter().any(|f| f.severity == LintSeverity::Warning);
    for finding in &lint {
        if finding.severity == LintSeverity::Error {
            warnings.push(format!(
                "lint[error/{code}]: {message}",
                code = finding.code,
                message = finding.message,
            ));
        }
    }

    let mut prechecks = capability_prechecks(&cap.env_requirements, env, install_mode);

    if dist_index.entries.is_empty() {
        warnings.push(
            "distribution index is empty or unavailable — no prebuilt artifacts will resolve"
                .to_string(),
        );
    }

    let mut components: Vec<ComponentPlan> = Vec::with_capacity(cap.components.len());
    let mut component_ctx = PlanComponentCtx {
        catalog,
        dist_index,
        env,
        install_mode,
        layout,
        prechecks: &mut prechecks,
        warnings: &mut warnings,
    };
    for comp_name in &cap.components {
        components.push(plan_component(&mut component_ctx, comp_name));
    }

    let any_fail = prechecks.iter().any(|p| p.status == "fail");
    let any_warn = prechecks.iter().any(|p| p.status == "warn");
    let any_comp_blocked = components.iter().any(|c| c.status == PlanStatus::Blocked);
    let any_comp_degraded = components.iter().any(|c| c.status == PlanStatus::Degraded);

    let status = if any_fail || any_comp_blocked || lint_has_error {
        PlanStatus::Blocked
    } else if any_warn || any_comp_degraded || !warnings.is_empty() || lint_has_warning {
        PlanStatus::Degraded
    } else {
        PlanStatus::Ready
    };

    let blocked_reason = if status == PlanStatus::Blocked {
        Some(first_blocker(&prechecks, &components, &lint))
    } else {
        None
    };

    let next_actions = next_actions_for(status, capability_name);

    let env_facts = EnvFactsSummary {
        os: env.os.clone(),
        arch: env.arch.clone(),
        libc: env.libc.clone(),
        pkg_base: env.pkg_base.clone(),
        kernel: env.kernel.clone(),
        btf: env.btf,
        cap_bpf: env.cap_bpf,
    };

    let layout = LayoutSummary {
        bin_dir: layout.bin_dir.to_string_lossy().into_owned(),
        etc_dir: layout.etc_dir.to_string_lossy().into_owned(),
        state_dir: layout.state_dir.to_string_lossy().into_owned(),
        log_dir: layout.log_dir.to_string_lossy().into_owned(),
        manifests_overlay: layout.manifests_overlay.to_string_lossy().into_owned(),
    };

    Ok(EnablePlan {
        schema_version: PLAN_SCHEMA_VERSION,
        capability: cap.capability.name.clone(),
        stability: cap.capability.stability.clone(),
        install_mode: install_mode.to_string(),
        dry_run: true,
        status,
        blocked_reason,
        components,
        prechecks,
        env_facts,
        layout,
        warnings,
        advice,
        next_actions,
        lint,
        // The planner has no view of the CLI's execution policy; the
        // caller (`anolisa-cli`) sets this via [`EnablePlan::set_execute_gate`].
        execute_gate: None,
    })
}

impl EnablePlan {
    /// Record the CLI's execute-policy decision against this plan.
    ///
    /// Two side effects:
    ///
    /// 1. Populates [`Self::execute_gate`] with a structured
    ///    `{allowed, reason}` record so machine consumers can decide
    ///    "is real execute allowed from this CLI build" without parsing
    ///    free-form warning text.
    /// 2. When `allowed = false`, rewrites [`Self::next_actions`] so the
    ///    plan no longer suggests `anolisa enable <cap>` — that command
    ///    will refuse. Also pushes a `"execute gate: ..."` warning if no
    ///    matching warning is already present, so human-mode renderers
    ///    surface the explanation without us double-printing it.
    ///
    /// **Single-shot contract.** Designed to be called once per plan,
    /// after `plan_enable` returns. Idempotency is narrow:
    ///
    /// - Repeated *closed* calls deduplicate the `"execute gate: ..."`
    ///   warning and overwrite `execute_gate` with the latest reason.
    ///   `next_actions` is rewritten on each closed call (except on
    ///   Blocked plans, where the structural blocker hint wins).
    /// - A subsequent *open* call replaces `execute_gate`, but does NOT
    ///   undo a prior closed call's side effects: the `"execute gate"`
    ///   warning stays in `warnings`, and `next_actions` stays at the
    ///   gated-fallback text. There is no closed→open recovery path
    ///   because the CLI never needs one — the execute-policy decision
    ///   is computed once and applied once.
    ///
    /// If a future caller needs closed→open recovery, change this to
    /// snapshot `warnings`/`next_actions` on entry, or rebuild them
    /// from the (cap, status) pair the way `plan_enable` does.
    pub fn set_execute_gate(&mut self, allowed: bool, reason: Option<String>) {
        if allowed {
            self.execute_gate = Some(ExecuteGate {
                allowed: true,
                reason: None,
            });
            return;
        }

        // Closed-gate path.
        let reason_text = reason.unwrap_or_else(|| "execute gate is closed".to_string());

        // Surface the explanation in `warnings` so human renderers print
        // it. Only inject the prefixed line if the caller hasn't already
        // pushed one with the same body to avoid duplicates.
        let warning = format!("execute gate: {reason_text}");
        if !self.warnings.iter().any(|w| w == &warning) {
            self.warnings.push(warning);
        }

        // Override next_actions so we never suggest `anolisa enable` for
        // a gated capability. The structural blocker hint still wins for
        // a Blocked plan — that's a "fix your manifest/env" condition,
        // separate from the CLI gate — so we only rewrite for non-Blocked
        // statuses where the original guidance would have pointed users at
        // a command that will refuse.
        if self.status != PlanStatus::Blocked {
            self.next_actions = vec![
                "execute gate is closed for this capability — see the `warnings` section for the policy hint, or run `anolisa enable --dry-run` for plan details only".to_string(),
            ];
        }

        self.execute_gate = Some(ExecuteGate {
            allowed: false,
            reason: Some(reason_text),
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn capability_prechecks(
    reqs: &EnvRequirements,
    env: &EnvFacts,
    install_mode: &str,
) -> Vec<PrecheckResult> {
    let mut out = eval_env_requirements(reqs, env, "");
    out.push(PrecheckResult {
        name: "install_mode".to_string(),
        status: "ok".to_string(),
        expected: install_mode.to_string(),
        actual: install_mode.to_string(),
        message: None,
    });
    out
}

/// Evaluate an [`EnvRequirements`] against an [`EnvFacts`] snapshot and
/// return one [`PrecheckResult`] per declared requirement. Used by both
/// capability-level checks and per-component checks; the latter passes a
/// `prefix` so the wire names disambiguate (`agentsight.btf` vs `btf`).
///
/// Three-valued result per check:
/// * `"ok"`   — requirement met.
/// * `"warn"` — probe unavailable on this host so we cannot evaluate; the
///   plan may still execute but should be reviewed.
/// * `"fail"` — probe returned a value that violates the requirement.
fn eval_env_requirements(
    reqs: &EnvRequirements,
    env: &EnvFacts,
    prefix: &str,
) -> Vec<PrecheckResult> {
    let mut out = Vec::new();
    let qualify = |n: &str| -> String {
        if prefix.is_empty() {
            n.to_string()
        } else {
            format!("{prefix}.{n}")
        }
    };

    if !reqs.os.is_empty() {
        let ok = reqs.os.iter().any(|o| o == &env.os);
        out.push(PrecheckResult {
            name: qualify("os"),
            status: if ok { "ok" } else { "fail" }.to_string(),
            expected: reqs.os.join("|"),
            actual: env.os.clone(),
            message: None,
        });
    }

    if !reqs.arch.is_empty() {
        let ok = reqs.arch.iter().any(|a| a == &env.arch);
        out.push(PrecheckResult {
            name: qualify("arch"),
            status: if ok { "ok" } else { "fail" }.to_string(),
            expected: reqs.arch.join("|"),
            actual: env.arch.clone(),
            message: None,
        });
    }

    if !reqs.libc.is_empty() {
        match env.libc.as_deref() {
            Some(l) => {
                let ok = reqs.libc.iter().any(|x| x == l);
                out.push(PrecheckResult {
                    name: qualify("libc"),
                    status: if ok { "ok" } else { "fail" }.to_string(),
                    expected: reqs.libc.join("|"),
                    actual: l.to_string(),
                    message: None,
                });
            }
            None => out.push(PrecheckResult {
                name: qualify("libc"),
                status: "warn".to_string(),
                expected: reqs.libc.join("|"),
                actual: "unknown".to_string(),
                message: Some("libc probe is unavailable on this host".to_string()),
            }),
        }
    }

    if !reqs.pkg_base.is_empty() {
        match env.pkg_base.as_deref() {
            Some(b) => {
                let ok = reqs.pkg_base.iter().any(|x| x == b);
                out.push(PrecheckResult {
                    name: qualify("pkg_base"),
                    status: if ok { "ok" } else { "fail" }.to_string(),
                    expected: reqs.pkg_base.join("|"),
                    actual: b.to_string(),
                    message: None,
                });
            }
            None => out.push(PrecheckResult {
                name: qualify("pkg_base"),
                status: "warn".to_string(),
                expected: reqs.pkg_base.join("|"),
                actual: "unknown".to_string(),
                message: Some("pkg_base probe is unavailable on this host".to_string()),
            }),
        }
    }

    if let Some(min) = reqs.kernel_min.as_deref() {
        // `kernel_min` is preserved verbatim from the manifest (e.g.
        // `">=5.8"` or `"5.8"`). Display whichever form the manifest
        // used, but normalize once for the numeric compare.
        let expected = if min.starts_with('>') {
            min.to_string()
        } else {
            format!(">={min}")
        };
        match env.kernel.as_deref() {
            Some(actual) => match compare_kernel(actual, min) {
                KernelCmp::Satisfies => out.push(PrecheckResult {
                    name: qualify("kernel"),
                    status: "ok".to_string(),
                    expected,
                    actual: actual.to_string(),
                    message: None,
                }),
                KernelCmp::Fails => out.push(PrecheckResult {
                    name: qualify("kernel"),
                    status: "fail".to_string(),
                    expected,
                    actual: actual.to_string(),
                    message: None,
                }),
                KernelCmp::Unparseable => out.push(PrecheckResult {
                    name: qualify("kernel"),
                    status: "warn".to_string(),
                    expected,
                    actual: actual.to_string(),
                    message: Some("could not parse kernel version string".to_string()),
                }),
            },
            None => out.push(PrecheckResult {
                name: qualify("kernel"),
                status: "warn".to_string(),
                expected,
                actual: "unknown".to_string(),
                message: Some("kernel probe is unavailable on this host".to_string()),
            }),
        }
    }

    if let Some(req) = reqs.btf {
        let expected = req.to_string();
        match env.btf {
            Some(actual) if actual == req => out.push(PrecheckResult {
                name: qualify("btf"),
                status: "ok".to_string(),
                expected,
                actual: actual.to_string(),
                message: None,
            }),
            Some(actual) => out.push(PrecheckResult {
                name: qualify("btf"),
                status: "fail".to_string(),
                expected,
                actual: actual.to_string(),
                message: Some("/sys/kernel/btf/vmlinux is not present".to_string()),
            }),
            None => out.push(PrecheckResult {
                name: qualify("btf"),
                status: "warn".to_string(),
                expected,
                actual: "unknown".to_string(),
                message: Some("BTF probe is unavailable on this host".to_string()),
            }),
        }
    }

    if let Some(req) = reqs.cap_bpf {
        let expected = req.to_string();
        match env.cap_bpf {
            Some(actual) if actual == req => out.push(PrecheckResult {
                name: qualify("cap_bpf"),
                status: "ok".to_string(),
                expected,
                actual: actual.to_string(),
                message: None,
            }),
            Some(actual) => out.push(PrecheckResult {
                name: qualify("cap_bpf"),
                status: "fail".to_string(),
                expected,
                actual: actual.to_string(),
                message: Some("process lacks CAP_BPF — run as root or grant CAP_BPF".to_string()),
            }),
            None => out.push(PrecheckResult {
                name: qualify("cap_bpf"),
                status: "warn".to_string(),
                expected,
                actual: "unknown".to_string(),
                message: Some("CAP_BPF probe is unavailable on this host".to_string()),
            }),
        }
    }

    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KernelCmp {
    Satisfies,
    Fails,
    Unparseable,
}

/// Best-effort `actual >= requirement` comparison on dotted version strings.
/// Returns `Unparseable` when neither side is a clean `<int>(.int)*` prefix
/// so callers can downgrade to `warn` instead of false-failing.
fn compare_kernel(actual: &str, requirement: &str) -> KernelCmp {
    fn parse(s: &str) -> Option<Vec<u64>> {
        // Take everything up to the first non-numeric / non-dot character so
        // suffixes like "-anolis23.x86_64" don't break parsing.
        let head: String = s
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if head.is_empty() {
            return None;
        }
        let mut nums = Vec::new();
        for part in head.split('.') {
            if part.is_empty() {
                continue;
            }
            nums.push(part.parse::<u64>().ok()?);
        }
        if nums.is_empty() { None } else { Some(nums) }
    }
    let req = requirement
        .trim_start_matches(">=")
        .trim_start_matches('>')
        .trim();
    let (Some(a), Some(r)) = (parse(actual), parse(req)) else {
        return KernelCmp::Unparseable;
    };
    let len = a.len().max(r.len());
    for i in 0..len {
        let av = a.get(i).copied().unwrap_or(0);
        let rv = r.get(i).copied().unwrap_or(0);
        if av > rv {
            return KernelCmp::Satisfies;
        }
        if av < rv {
            return KernelCmp::Fails;
        }
    }
    KernelCmp::Satisfies
}

struct PlanComponentCtx<'a> {
    catalog: &'a Catalog,
    dist_index: &'a DistributionIndex,
    env: &'a EnvFacts,
    install_mode: &'a str,
    layout: &'a FsLayout,
    prechecks: &'a mut Vec<PrecheckResult>,
    warnings: &'a mut Vec<String>,
}

fn plan_component(ctx: &mut PlanComponentCtx<'_>, name: &str) -> ComponentPlan {
    let comp = match ctx.catalog.component(name) {
        Some(c) => c,
        None => {
            return ComponentPlan {
                name: name.to_string(),
                manifest_version: None,
                status: PlanStatus::Blocked,
                blocked_reason: Some(format!(
                    "component '{name}' is not in the catalog (catalog inconsistency)"
                )),
                artifact: None,
                services: Vec::new(),
                files: Vec::new(),
                resolved_files: Vec::new(),
                capabilities: Vec::new(),
                requires_privilege: false,
                env_requirements: EnvRequirements::default(),
                health_check: None,
            };
        }
    };

    let requires_privilege = ctx.install_mode == "system";
    let resolved_files = render_files(&comp.install.files, ctx.layout);
    // Carry the synthesized/declared health probe into every plan slice so the
    // executor never re-loads the catalog to find it.
    let health_check = comp.health_spec();

    // Evaluate the component's own env_requirements and append to the
    // top-level precheck list so users see a single ordered table. The
    // outcome also drives this component's status (warn → degraded,
    // fail → blocked) without needing a second pass.
    let comp_checks = eval_env_requirements(&comp.env_requirements, ctx.env, &comp.component.name);
    let comp_has_fail = comp_checks.iter().any(|p| p.status == "fail");
    let comp_has_warn = comp_checks.iter().any(|p| p.status == "warn");
    let first_comp_fail = comp_checks
        .iter()
        .find(|p| p.status == "fail")
        .map(describe_precheck_blocker);
    ctx.prechecks.extend(comp_checks);

    // Install-mode gate from component manifest. The resolver also enforces
    // this, but checking it here gives a clearer blocked reason than
    // `UnsupportedMode` and avoids confusion when the component manifest
    // is more conservative than the index.
    if !comp.install.modes.is_empty() && !comp.install.modes.iter().any(|m| m == ctx.install_mode) {
        return ComponentPlan {
            name: comp.component.name.clone(),
            manifest_version: Some(comp.component.version.clone()),
            status: PlanStatus::Blocked,
            blocked_reason: Some(format!(
                "component '{}' does not support install_mode '{}' (allowed: {})",
                comp.component.name,
                ctx.install_mode,
                comp.install.modes.join(", "),
            )),
            artifact: None,
            services: comp.install.services.clone(),
            files: comp.install.files.clone(),
            resolved_files,
            capabilities: comp.install.capabilities.clone(),
            requires_privilege,
            env_requirements: comp.env_requirements.clone(),
            health_check: health_check.clone(),
        };
    }

    if comp_has_fail {
        return ComponentPlan {
            name: comp.component.name.clone(),
            manifest_version: Some(comp.component.version.clone()),
            status: PlanStatus::Blocked,
            blocked_reason: first_comp_fail.or_else(|| {
                Some(format!(
                    "component '{}' env requirements not satisfied",
                    comp.component.name
                ))
            }),
            artifact: None,
            services: comp.install.services.clone(),
            files: comp.install.files.clone(),
            resolved_files,
            capabilities: comp.install.capabilities.clone(),
            requires_privilege,
            env_requirements: comp.env_requirements.clone(),
            health_check: health_check.clone(),
        };
    }

    let preferred = select_preferred_types(&comp.distribution_selectors, ctx.env, ctx.install_mode);

    let query = ResolveQuery {
        component: &comp.component.name,
        version: None,
        channel: None,
        install_mode: ctx.install_mode,
        os: &ctx.env.os,
        arch: &ctx.env.arch,
        libc: ctx.env.libc.as_deref(),
        pkg_base: ctx.env.pkg_base.as_deref(),
        preferred_types: &preferred,
    };

    match ctx.dist_index.resolve(&query) {
        Ok(entry) => {
            if entry.version != comp.component.version {
                ctx.warnings.push(format!(
                    "component '{}': artifact version {} differs from manifest version {}",
                    comp.component.name, entry.version, comp.component.version,
                ));
            }
            // Missing sha256 is a structural blocker, not a soft warning:
            // the executor's DownloadCache does not enforce a checksum when
            // given None, so a Degraded-but-executable plan would silently
            // install unverified bytes. Surface as Blocked so the user
            // fixes the distribution index instead.
            if entry.sha256.is_none() {
                return ComponentPlan {
                    name: comp.component.name.clone(),
                    manifest_version: Some(comp.component.version.clone()),
                    status: PlanStatus::Blocked,
                    blocked_reason: Some(format!(
                        "resolved artifact for component '{}' has no sha256 — refuse to install without verification",
                        comp.component.name,
                    )),
                    artifact: Some(artifact_plan_from(&entry)),
                    services: comp.install.services.clone(),
                    files: comp.install.files.clone(),
                    resolved_files,
                    capabilities: comp.install.capabilities.clone(),
                    requires_privilege,
                    env_requirements: comp.env_requirements.clone(),
                    health_check: health_check.clone(),
                };
            }

            // Convention-derived URLs (empty `url`) need the repo base_url,
            // which the capability planner does not carry — only `anolisa
            // install` can join them. Block instead of letting an empty URL
            // surface later as a baffling download error.
            if entry.url.is_empty() {
                return ComponentPlan {
                    name: comp.component.name.clone(),
                    manifest_version: Some(comp.component.version.clone()),
                    status: PlanStatus::Blocked,
                    blocked_reason: Some(format!(
                        "resolved artifact for component '{}' omits its url (convention-layout row) — only `anolisa install` can derive it",
                        comp.component.name,
                    )),
                    artifact: Some(artifact_plan_from(&entry)),
                    services: comp.install.services.clone(),
                    files: comp.install.files.clone(),
                    resolved_files,
                    capabilities: comp.install.capabilities.clone(),
                    requires_privilege,
                    env_requirements: comp.env_requirements.clone(),
                    health_check: health_check.clone(),
                };
            }

            // Resolved-artifact safety gate.
            //
            // The lint pass emits W_UNSUPPORTED_ARTIFACT_TYPE for sibling
            // rpm/deb/oci entries when a supported entry also exists, but
            // the resolver does not filter by supported type — it ranks
            // by os/arch/install_mode/version and `preferred_artifact_types`.
            // So a higher-version rpm can win over a lower-version tar_gz,
            // or `preferred_artifact_types = ["rpm", "tar_gz"]` can pick
            // rpm explicitly. Without a post-resolve check those paths
            // would yield Ready and then crash at the runner with
            // `InstallError::UnsupportedArtifactType`. Block the component
            // here so plan readiness can't lie about runtime feasibility.
            let wire_type = artifact_type_label(entry.artifact_type);
            if !SUPPORTED_ARTIFACT_TYPES.contains(&wire_type.as_str()) {
                return ComponentPlan {
                    name: comp.component.name.clone(),
                    manifest_version: Some(comp.component.version.clone()),
                    status: PlanStatus::Blocked,
                    blocked_reason: Some(format!(
                        "resolved artifact for component '{}' is artifact_type '{}' which the install runner cannot handle (supported: {})",
                        comp.component.name,
                        wire_type,
                        SUPPORTED_ARTIFACT_TYPES.join(", "),
                    )),
                    artifact: Some(artifact_plan_from(&entry)),
                    services: comp.install.services.clone(),
                    files: comp.install.files.clone(),
                    resolved_files,
                    capabilities: comp.install.capabilities.clone(),
                    requires_privilege,
                    env_requirements: comp.env_requirements.clone(),
                    health_check: health_check.clone(),
                };
            }

            let status = if comp_has_warn {
                PlanStatus::Degraded
            } else {
                PlanStatus::Ready
            };
            ComponentPlan {
                name: comp.component.name.clone(),
                manifest_version: Some(comp.component.version.clone()),
                status,
                blocked_reason: None,
                artifact: Some(artifact_plan_from(&entry)),
                services: comp.install.services.clone(),
                files: comp.install.files.clone(),
                resolved_files,
                capabilities: comp.install.capabilities.clone(),
                requires_privilege,
                env_requirements: comp.env_requirements.clone(),
                health_check: health_check.clone(),
            }
        }
        Err(err) => {
            let reason =
                describe_resolve_error(&comp.component.name, &err, ctx.env, ctx.install_mode);
            ComponentPlan {
                name: comp.component.name.clone(),
                manifest_version: Some(comp.component.version.clone()),
                status: PlanStatus::Blocked,
                blocked_reason: Some(reason),
                artifact: None,
                services: comp.install.services.clone(),
                files: comp.install.files.clone(),
                resolved_files,
                capabilities: comp.install.capabilities.clone(),
                requires_privilege,
                env_requirements: comp.env_requirements.clone(),
                health_check: health_check.clone(),
            }
        }
    }
}

/// Substitute `{bindir}`, `{etcdir}`/`{etc_dir}`/`{sysconfdir}`,
/// `{statedir}`/`{state_dir}`, `{logdir}`/`{log_dir}`,
/// `{datadir}`/`{sharedir}`, `{libexecdir}`/`{libexec_dir}` in each manifest
/// file pattern using the resolved layout. Unknown placeholders are left as-is
/// so users can still see which patterns the install runner would need to
/// handle.
///
/// `{sysconfdir}` and `{sharedir}` are the minimal-schema spellings; the legacy
/// `{etcdir}`/`{datadir}` names remain accepted for additive compatibility.
fn render_files(files: &[InstallFileSpec], layout: &FsLayout) -> Vec<String> {
    let bin = layout.bin_dir.to_string_lossy().into_owned();
    let etc = layout.etc_dir.to_string_lossy().into_owned();
    let state = layout.state_dir.to_string_lossy().into_owned();
    let log = layout.log_dir.to_string_lossy().into_owned();
    let data = layout.datadir.to_string_lossy().into_owned();
    let libexec = layout.libexec_dir.to_string_lossy().into_owned();
    files
        .iter()
        .filter_map(|f| f.install_path())
        .map(|f| {
            f.replace("{bindir}", &bin)
                .replace("{sysconfdir}", &etc)
                .replace("{etcdir}", &etc)
                .replace("{etc_dir}", &etc)
                .replace("{statedir}", &state)
                .replace("{state_dir}", &state)
                .replace("{logdir}", &log)
                .replace("{log_dir}", &log)
                .replace("{sharedir}", &data)
                .replace("{datadir}", &data)
                .replace("{libexecdir}", &libexec)
                .replace("{libexec_dir}", &libexec)
        })
        .collect()
}

fn describe_precheck_blocker(p: &PrecheckResult) -> String {
    let detail = p.message.as_deref().unwrap_or("");
    if detail.is_empty() {
        format!(
            "precheck '{}' failed: expected {}, actual {}",
            p.name, p.expected, p.actual,
        )
    } else {
        format!(
            "precheck '{}' failed: expected {}, actual {} ({})",
            p.name, p.expected, p.actual, detail,
        )
    }
}

fn artifact_plan_from(entry: &DistributionEntry) -> ArtifactPlan {
    ArtifactPlan {
        artifact_type: artifact_type_label(entry.artifact_type),
        backend: entry.backend.clone(),
        version: entry.version.clone(),
        url: entry.url.clone(),
        sha256: entry.sha256.clone(),
        signature: entry.signature.clone(),
        artifact_id: entry.artifact_id.clone(),
        meta_sha256: None,
    }
}

fn artifact_type_label(t: ArtifactType) -> String {
    match t {
        ArtifactType::Rpm => "rpm",
        ArtifactType::Deb => "deb",
        ArtifactType::TarGz => "tar_gz",
        ArtifactType::Zip => "zip",
        ArtifactType::Oci => "oci",
        ArtifactType::File => "file",
        ArtifactType::Binary => "binary",
    }
    .to_string()
}

fn select_preferred_types(
    selectors: &[DistributionSelector],
    env: &EnvFacts,
    install_mode: &str,
) -> Vec<ArtifactType> {
    selectors
        .iter()
        .find(|sel| selector_matches(sel, env, install_mode))
        .map(|sel| sel.preferred_artifact_types.clone())
        .unwrap_or_default()
}

fn selector_matches(sel: &DistributionSelector, env: &EnvFacts, install_mode: &str) -> bool {
    if let Some(m) = sel.install_mode.as_deref()
        && m != install_mode
    {
        return false;
    }
    if !sel.os.is_empty() && !sel.os.iter().any(|o| o == &env.os) {
        return false;
    }
    if !sel.arch.is_empty() && !sel.arch.iter().any(|a| a == &env.arch) {
        return false;
    }
    if let Some(l) = sel.libc.as_deref() {
        match env.libc.as_deref() {
            Some(env_l) if env_l == l => {}
            _ => return false,
        }
    }
    if let Some(b) = sel.pkg_base.as_deref() {
        match env.pkg_base.as_deref() {
            Some(env_b) if env_b == b => {}
            _ => return false,
        }
    }
    true
}

fn describe_resolve_error(
    component: &str,
    err: &ResolveError,
    env: &EnvFacts,
    install_mode: &str,
) -> String {
    match err {
        ResolveError::NotFound => format!(
            "no prebuilt artifact for component '{}' on {}/{} (install_mode={})",
            component, env.os, env.arch, install_mode,
        ),
        ResolveError::UnsupportedMode => {
            format!("component '{component}' has no artifact for install_mode '{install_mode}'")
        }
        ResolveError::Ambiguous(list) => format!(
            "multiple artifacts match component '{}' ({} candidates); declare preferred_artifact_types to disambiguate",
            component,
            list.len(),
        ),
        ResolveError::ChecksumMissing => {
            format!("matching artifact for component '{component}' has no sha256")
        }
    }
}

fn first_blocker(
    prechecks: &[PrecheckResult],
    components: &[ComponentPlan],
    lint: &[LintFinding],
) -> String {
    if let Some(p) = prechecks.iter().find(|p| p.status == "fail") {
        return describe_precheck_blocker(p);
    }
    if let Some(c) = components.iter().find(|c| c.status == PlanStatus::Blocked) {
        if let Some(r) = c.blocked_reason.as_deref() {
            return r.to_string();
        }
        return format!("component '{}' is blocked", c.name);
    }
    // Lint errors land here when prechecks/components are otherwise
    // clean but the manifest itself is structurally broken (e.g. an
    // install dest outside owned roots that we caught before the
    // executor ever ran). Naming the lint code makes the blocker
    // searchable in CI logs.
    if let Some(finding) = lint.iter().find(|f| f.severity == LintSeverity::Error) {
        return format!(
            "manifest lint failed ({code}): {message}",
            code = finding.code,
            message = finding.message,
        );
    }
    "plan is blocked".to_string()
}

fn next_actions_for(status: PlanStatus, capability: &str) -> Vec<String> {
    match status {
        PlanStatus::Ready => vec![format!("run `anolisa enable {capability}` to execute")],
        PlanStatus::Degraded => vec![
            "review warnings above".to_string(),
            format!("run `anolisa enable {capability}` to execute after reviewing warnings"),
        ],
        PlanStatus::Blocked => vec![format!(
            "resolve the blocker above before running `anolisa enable {capability}`"
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::distribution::{ArtifactType, DistributionEntry, DistributionIndex};
    use crate::manifest::{
        BuildSpec, CapabilityManifest, CapabilityMeta, ComponentManifest, ComponentMeta,
        DependenciesSpec, DistributionSelector, FeatureSpec, FileKind, InstallCapabilitySpec,
        InstallFileSpec, InstallSpec, SourceSpec,
    };
    use crate::{Catalog, CatalogLayers};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn make_env(os: &str, arch: &str) -> EnvFacts {
        EnvFacts {
            os: os.to_string(),
            arch: arch.to_string(),
            libc: if os == "linux" {
                Some("glibc".to_string())
            } else {
                None
            },
            kernel: None,
            pkg_base: if os == "linux" {
                Some("anolis23".to_string())
            } else {
                None
            },
            btf: if os == "linux" { Some(true) } else { None },
            cap_bpf: if os == "linux" { Some(true) } else { None },
            container: None,
            user: "tester".to_string(),
            uid: 1000,
            home: PathBuf::from("/tmp/home"),
        }
    }

    fn make_layout() -> FsLayout {
        FsLayout::user(PathBuf::from("/tmp/home"))
    }

    fn make_cap(
        name: &str,
        components: Vec<String>,
        os: Vec<String>,
        arch: Vec<String>,
    ) -> CapabilityManifest {
        CapabilityManifest {
            schema_version: 2,
            capability: CapabilityMeta {
                name: name.to_string(),
                description: format!("test capability {name}"),
                layer: "tier1-capability".to_string(),
                stability: "stable".to_string(),
            },
            components,
            default_features: Vec::new(),
            env_requirements: EnvRequirements {
                os,
                arch,
                libc: Vec::new(),
                kernel_min: None,
                btf: None,
                cap_bpf: None,
                pkg_base: Vec::new(),
            },
        }
    }

    fn make_component(
        name: &str,
        version: &str,
        install_modes: Vec<String>,
        selectors: Vec<DistributionSelector>,
    ) -> ComponentManifest {
        ComponentManifest {
            schema_version: 2,
            component: ComponentMeta {
                name: name.to_string(),
                version: version.to_string(),
                layer: "runtime".to_string(),
                domain: Some("test".to_string()),
                display_name: None,
                owner: None,
                license: None,
                repository: None,
            },
            contract: Default::default(),
            artifact: Default::default(),
            source: SourceSpec::default(),
            distribution_selectors: selectors,
            build: BuildSpec::default(),
            install: InstallSpec {
                modes: install_modes,
                files: vec![InstallFileSpec {
                    kind: FileKind::Data,
                    source: None,
                    dest: Some("{bindir}/agentsight".to_string()),
                    mode: None,
                }],
                services: vec!["agentsight.service".to_string()],
                capabilities: Vec::new(),
            },
            env_requirements: EnvRequirements::default(),
            dependencies: DependenciesSpec::default(),
            features: Vec::<FeatureSpec>::new(),
            adapters: Vec::new(),
            health_check: None,
            health_checks: Vec::new(),
        }
    }

    fn make_catalog(caps: Vec<CapabilityManifest>, comps: Vec<ComponentManifest>) -> Catalog {
        let mut capabilities = BTreeMap::new();
        for c in caps {
            capabilities.insert(c.capability.name.clone(), c);
        }
        let mut components = BTreeMap::new();
        for c in comps {
            components.insert(c.component.name.clone(), c);
        }
        Catalog {
            capabilities,
            components,
            layers: CatalogLayers::bundled_only(PathBuf::from("/dev/null")),
        }
    }

    fn agentsight_entry(
        version: &str,
        artifact_type: ArtifactType,
        backend: &str,
        install_modes: Vec<String>,
        os: &str,
        arch: &str,
        pkg_base: Option<String>,
    ) -> DistributionEntry {
        DistributionEntry {
            component: "agentsight".to_string(),
            version: version.to_string(),
            channel: "stable".to_string(),
            artifact_type,
            backend: backend.to_string(),
            url: format!("https://example.invalid/agentsight-{version}.{backend}"),
            os: os.to_string(),
            arch: arch.to_string(),
            libc: Some("glibc".to_string()),
            pkg_base,
            install_modes,
            sha256: Some("0".repeat(64)),
            signature: None,
            artifact_id: None,
            manifest_digest: None,
            size: None,
            signature_url: None,
            os_version: None,
            dependencies: Vec::new(),
        }
    }

    fn make_index(entries: Vec<DistributionEntry>) -> DistributionIndex {
        DistributionIndex {
            schema_version: 1,
            channel: None,
            generated_at: None,
            expires_at: None,
            publisher: None,
            signature: None,
            entries,
        }
    }

    #[test]
    fn happy_path_linux_x86_64_yields_ready_plan() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        // Use a supported artifact_type (`tar_gz`) so the new
        // `E_UNSUPPORTED_ARTIFACT_TYPE` lint does not fire here. The test
        // is about the env+install-mode happy path, not about exercising
        // rpm specifically — rpm support is gated separately in
        // `unsupported_artifact_type_blocks_plan`.
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::TarGz,
            "tar_gz",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Ready);
        assert!(plan.blocked_reason.is_none());
        assert_eq!(plan.components.len(), 1);
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.name, "agentsight");
        assert_eq!(comp_plan.status, PlanStatus::Ready);
        let artifact = comp_plan.artifact.as_ref().expect("artifact selected");
        assert_eq!(artifact.artifact_type, "tar_gz");
        assert_eq!(artifact.version, "0.2.0");
        assert!(comp_plan.requires_privilege);
        assert!(plan.warnings.is_empty());
    }

    /// rpm/deb/oci entries must not produce a `Ready` plan even though
    /// the env / install-mode / sha256 checks all pass — the install
    /// runner cannot execute them. The lint pipe is the gate, so this
    /// covers the contract that lint errors fold into `PlanStatus::Blocked`
    /// the same as any other structural breakage.
    #[test]
    fn unsupported_artifact_type_blocks_plan() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::Rpm,
            "rpm",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(
            plan.status,
            PlanStatus::Blocked,
            "rpm must block the plan even when env+sha checks pass",
        );
        assert!(
            plan.lint
                .iter()
                .any(|f| f.code == "E_UNSUPPORTED_ARTIFACT_TYPE"),
            "expected E_UNSUPPORTED_ARTIFACT_TYPE in lint findings, got: {:?}",
            plan.lint,
        );
    }

    /// Hole closed: lint sibling-aware demotion is informational only.
    /// When the resolver actually selects an unsupported artifact_type
    /// (here `preferred_artifact_types = [Rpm, TarGz]` picks rpm even
    /// though a supported tar_gz sibling exists), the planner must
    /// block the component at the post-resolve gate so a Ready plan
    /// can't be followed by `InstallError::UnsupportedArtifactType`.
    /// The lint emits only a warning in this configuration — the
    /// safety gate is the resolved-entry check, not the lint.
    #[test]
    fn resolver_picks_unsupported_type_blocks_component_even_with_supported_sibling() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        // preferred_artifact_types forces the resolver to rank rpm above
        // tar_gz for this env/install_mode.
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            vec![DistributionSelector {
                install_mode: Some("system".to_string()),
                os: vec!["linux".to_string()],
                arch: vec!["x86_64".to_string()],
                libc: None,
                pkg_base: None,
                preferred_artifact_types: vec![ArtifactType::Rpm, ArtifactType::TarGz],
            }],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![
            agentsight_entry(
                "0.2.0",
                ArtifactType::TarGz,
                "tar_gz",
                vec!["system".to_string()],
                "linux",
                "x86_64",
                Some("anolis23".to_string()),
            ),
            agentsight_entry(
                "0.2.0",
                ArtifactType::Rpm,
                "rpm",
                vec!["system".to_string()],
                "linux",
                "x86_64",
                Some("anolis23".to_string()),
            ),
        ]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");

        // Plan-level: Blocked because the resolved-artifact gate fires
        // even though the lint emits only a sibling warning.
        assert_eq!(
            plan.status,
            PlanStatus::Blocked,
            "resolved rpm must block the plan despite a tar_gz sibling, got: {plan:?}",
        );
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.status, PlanStatus::Blocked);
        // The artifact field is preserved so users see what got selected.
        let artifact = comp_plan.artifact.as_ref().expect("artifact surfaced");
        assert_eq!(
            artifact.artifact_type, "rpm",
            "the resolver did pick rpm — that's the whole point of this test",
        );
        let reason = comp_plan.blocked_reason.as_deref().unwrap_or("");
        assert!(
            reason.contains("rpm") && reason.contains("install runner"),
            "blocked_reason should name the selected artifact_type, got: {reason}",
        );

        // Lint contract: the sibling rule keeps it as a *warning* in this
        // configuration. The safety gate is the post-resolve gate, not the
        // lint, and these tests pin that contract so the two don't drift.
        let codes: Vec<_> = plan.lint.iter().map(|f| f.code).collect();
        assert!(
            codes.contains(&"W_UNSUPPORTED_ARTIFACT_TYPE"),
            "lint should still emit the sibling warning, got: {codes:?}",
        );
        assert!(
            !codes.contains(&"E_UNSUPPORTED_ARTIFACT_TYPE"),
            "lint must not escalate when a supported sibling exists, got: {codes:?}",
        );
    }

    #[test]
    fn version_drift_between_manifest_and_index_marks_degraded() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        // Index advertises only 0.1.0 — older than manifest. Use a
        // supported artifact_type so the only lint signal is the
        // version-drift warning under test.
        let index = make_index(vec![agentsight_entry(
            "0.1.0",
            ArtifactType::TarGz,
            "tar_gz",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Degraded);
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.contains("0.1.0") && w.contains("0.2.0"))
        );
    }

    #[test]
    fn unknown_capability_returns_plan_error() {
        let catalog = make_catalog(Vec::new(), Vec::new());
        let index = make_index(Vec::new());
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let err = plan_enable(&catalog, &index, &env, "system", &layout, "not-real")
            .expect_err("must error");
        assert_eq!(err, PlanError::UnknownCapability("not-real".to_string()));
    }

    #[test]
    fn missing_component_blocks_plan_with_clear_reason() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        // No component manifest registered — catalog inconsistency.
        let catalog = make_catalog(vec![cap], Vec::new());
        let index = make_index(Vec::new());
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Blocked);
        let reason = plan.blocked_reason.as_deref().unwrap_or("");
        assert!(
            reason.contains("agentsight"),
            "reason mentions component: {reason}"
        );
        assert_eq!(plan.components[0].status, PlanStatus::Blocked);
        assert!(plan.components[0].artifact.is_none());
    }

    #[test]
    fn no_matching_artifact_on_macos_blocks_plan_without_panic() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::Rpm,
            "rpm",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("macos", "aarch64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Blocked);
        // OS precheck fails AND component artifact unresolved.
        assert!(
            plan.prechecks
                .iter()
                .any(|p| p.name == "os" && p.status == "fail")
        );
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.status, PlanStatus::Blocked);
        assert!(comp_plan.artifact.is_none());
        let reason = plan.blocked_reason.as_deref().unwrap_or("");
        assert!(!reason.is_empty(), "blocked_reason should be populated");
    }

    #[test]
    fn install_mode_user_blocks_when_component_is_system_only() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::Rpm,
            "rpm",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "user",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Blocked);
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.status, PlanStatus::Blocked);
        let reason = comp_plan.blocked_reason.as_deref().unwrap_or("");
        assert!(reason.contains("install_mode"), "reason: {reason}");
        assert!(reason.contains("user"));
    }

    /// Resolved artifact without sha256 → Blocked, not Degraded: the
    /// executor cannot verify unsigned/unhashed bytes and must refuse.
    #[test]
    fn missing_sha256_marks_component_blocked() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let mut entry = agentsight_entry(
            "0.2.0",
            ArtifactType::Rpm,
            "rpm",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        );
        entry.sha256 = None;
        let index = make_index(vec![entry]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Blocked);
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.status, PlanStatus::Blocked);
        // Artifact stays populated so users see what was selected and why.
        assert!(comp_plan.artifact.is_some(), "artifact should be surfaced");
        let reason = comp_plan.blocked_reason.as_deref().unwrap_or("");
        assert!(reason.contains("sha256"), "reason names sha256: {reason}");
    }

    /// Convention-layout rows (empty `url`) need the repo base_url to derive
    /// the artifact location; the capability planner has none → Blocked.
    #[test]
    fn empty_url_marks_component_blocked() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let mut entry = agentsight_entry(
            "0.2.0",
            ArtifactType::Rpm,
            "rpm",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        );
        entry.url = String::new();
        let index = make_index(vec![entry]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Blocked);
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.status, PlanStatus::Blocked);
        let reason = comp_plan.blocked_reason.as_deref().unwrap_or("");
        assert!(reason.contains("omits its url"), "reason: {reason}");
        assert!(reason.contains("anolisa install"), "reason: {reason}");
    }

    #[test]
    fn preferred_artifact_types_tiebreak_selects_tar_gz() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            vec![DistributionSelector {
                install_mode: Some("system".to_string()),
                os: vec!["linux".to_string()],
                arch: vec!["x86_64".to_string()],
                libc: None,
                pkg_base: None,
                preferred_artifact_types: vec![ArtifactType::TarGz, ArtifactType::Binary],
            }],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        // Tiebreak uses two supported artifact_types so the lint stays
        // silent and the only signal is which one the planner picks.
        let index = make_index(vec![
            agentsight_entry(
                "0.2.0",
                ArtifactType::Binary,
                "binary",
                vec!["system".to_string()],
                "linux",
                "x86_64",
                Some("anolis23".to_string()),
            ),
            agentsight_entry(
                "0.2.0",
                ArtifactType::TarGz,
                "tar",
                vec!["system".to_string()],
                "linux",
                "x86_64",
                Some("anolis23".to_string()),
            ),
        ]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Ready);
        let artifact = plan.components[0]
            .artifact
            .as_ref()
            .expect("tiebreak picked an artifact");
        assert_eq!(artifact.artifact_type, "tar_gz");
    }

    #[test]
    fn libc_unknown_on_non_linux_marks_warn_not_fail() {
        // Requirement says libc=glibc but env is macOS with libc=None.
        let mut cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["macos".to_string()],
            vec!["aarch64".to_string()],
        );
        cap.env_requirements.libc = vec!["glibc".to_string()];
        let comp = make_component("agentsight", "0.2.0", vec!["user".to_string()], Vec::new());
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![DistributionEntry {
            component: "agentsight".to_string(),
            version: "0.2.0".to_string(),
            channel: "stable".to_string(),
            artifact_type: ArtifactType::TarGz,
            backend: "tar".to_string(),
            url: "https://example.invalid/agentsight-0.2.0-macos.tar.gz".to_string(),
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            libc: None,
            pkg_base: None,
            install_modes: vec!["user".to_string()],
            sha256: Some("0".repeat(64)),
            signature: None,
            artifact_id: None,
            manifest_digest: None,
            size: None,
            signature_url: None,
            os_version: None,
            dependencies: Vec::new(),
        }]);
        let env = make_env("macos", "aarch64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "user",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        // libc warn alone (no fail) — overall status: degraded, not blocked.
        assert_eq!(plan.status, PlanStatus::Degraded);
        let libc = plan
            .prechecks
            .iter()
            .find(|p| p.name == "libc")
            .expect("libc precheck");
        assert_eq!(libc.status, "warn");
        assert_eq!(libc.actual, "unknown");
    }

    /// Component declares btf=true / cap_bpf=true / kernel_min=">=5.8" and
    /// host env satisfies all of them — the plan still resolves to Ready
    /// and the component prechecks appear with `agentsight.*` namespace.
    #[test]
    fn component_env_requirements_satisfied_keeps_plan_ready() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let mut comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        comp.env_requirements.kernel_min = Some("5.8".to_string());
        comp.env_requirements.btf = Some(true);
        comp.env_requirements.cap_bpf = Some(true);
        let catalog = make_catalog(vec![cap], vec![comp]);
        // Supported artifact_type so the env-prechecks happy path is what
        // gets exercised here (not the unsupported-type lint).
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::TarGz,
            "tar_gz",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let mut env = make_env("linux", "x86_64");
        env.kernel = Some("6.6.0-anolis23.x86_64".to_string());
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Ready);
        // Component checks should be present and namespaced.
        let names: Vec<&str> = plan.prechecks.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"agentsight.kernel"));
        assert!(names.contains(&"agentsight.btf"));
        assert!(names.contains(&"agentsight.cap_bpf"));
        for p in &plan.prechecks {
            assert_eq!(p.status, "ok", "{}: {p:?}", p.name);
        }
    }

    /// Component requires btf=true but host reports btf=Some(false) — the
    /// component must be blocked and the namespaced precheck must fail.
    #[test]
    fn component_btf_required_but_disabled_blocks_component() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let mut comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        comp.env_requirements.btf = Some(true);
        let catalog = make_catalog(vec![cap], vec![comp]);
        // Artifact type kept as rpm here on purpose — the env precheck
        // is the primary blocker and rpm just stacks a second lint error
        // on top of the Blocked status; the assertion target stays
        // `PlanStatus::Blocked` either way.
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::Rpm,
            "rpm",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let mut env = make_env("linux", "x86_64");
        env.btf = Some(false);
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Blocked);
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.status, PlanStatus::Blocked);
        let btf = plan
            .prechecks
            .iter()
            .find(|p| p.name == "agentsight.btf")
            .expect("namespaced btf precheck");
        assert_eq!(btf.status, "fail");
    }

    /// Component requires btf=true but the probe is unknown (e.g. macOS
    /// host that still happens to satisfy os/arch). Should warn, not fail,
    /// and the component should land in `degraded`, not `blocked`.
    #[test]
    fn component_btf_unknown_marks_component_degraded() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let mut comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        comp.env_requirements.btf = Some(true);
        let catalog = make_catalog(vec![cap], vec![comp]);
        // Supported artifact_type so the only escalation signal is the
        // btf-unknown precheck under test.
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::TarGz,
            "tar_gz",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let mut env = make_env("linux", "x86_64");
        env.btf = None;
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Degraded);
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.status, PlanStatus::Degraded);
        let btf = plan
            .prechecks
            .iter()
            .find(|p| p.name == "agentsight.btf")
            .expect("namespaced btf precheck");
        assert_eq!(btf.status, "warn");
    }

    /// kernel_min comparator parses `>=X.Y` and dotted version with suffix.
    /// `5.4` < `5.8` must fail; `5.15.0-anolis23.x86_64` >= `5.8` must
    /// satisfy; an unparseable kernel string must downgrade to warn, not
    /// false-fail.
    #[test]
    fn kernel_min_compare_handles_typical_inputs() {
        assert_eq!(
            compare_kernel("5.15.0-anolis23.x86_64", "5.8"),
            KernelCmp::Satisfies
        );
        assert_eq!(compare_kernel("5.4.123", "5.8"), KernelCmp::Fails);
        assert_eq!(compare_kernel("5.8", "5.8"), KernelCmp::Satisfies);
        assert_eq!(compare_kernel("6.6", ">=5.8"), KernelCmp::Satisfies);
        assert_eq!(
            compare_kernel("not-a-version", "5.8"),
            KernelCmp::Unparseable
        );
    }

    /// Empty index → planner still returns a plan, with a top-level warning
    /// and components blocked because no artifact resolved.
    #[test]
    fn empty_distribution_index_yields_blocked_plan_with_warning() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(Vec::new());
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Blocked);
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.contains("distribution index")),
            "expected index warning, got {:?}",
            plan.warnings,
        );
        let comp_plan = &plan.components[0];
        assert_eq!(comp_plan.status, PlanStatus::Blocked);
        assert!(comp_plan.artifact.is_none());
    }

    /// `resolved_files` must substitute layout vars. We use a user-mode
    /// layout under a tmp home so the assertion is deterministic.
    #[test]
    fn resolved_files_substitute_layout_placeholders() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component("agentsight", "0.2.0", vec!["user".to_string()], Vec::new());
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::TarGz,
            "tar",
            vec!["user".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("linux", "x86_64");
        let layout = FsLayout::user(PathBuf::from("/tmp/home"));
        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "user",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        let comp_plan = &plan.components[0];
        assert_eq!(
            comp_plan.files,
            vec![InstallFileSpec {
                kind: FileKind::Data,
                source: None,
                dest: Some("{bindir}/agentsight".to_string()),
                mode: None,
            }]
        );
        let resolved = &comp_plan.resolved_files[0];
        assert!(
            !resolved.contains('{') && resolved.ends_with("/agentsight"),
            "resolved file should be substituted: {resolved}",
        );
        assert!(
            resolved.starts_with("/tmp/home/.local/bin"),
            "resolved should be under XDG bin_dir: {resolved}",
        );
    }

    #[test]
    fn plan_preserves_structured_install_files_and_resolves_install_paths() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let mut comp = make_component("agentsight", "0.2.0", vec!["user".to_string()], Vec::new());
        comp.install.files = vec![
            InstallFileSpec {
                kind: FileKind::Data,
                source: Some("target/release/agentsight".to_string()),
                dest: Some("{bindir}/agentsight".to_string()),
                mode: Some("0755".to_string()),
            },
            InstallFileSpec {
                kind: FileKind::Data,
                source: Some("{datadir}/source-only".to_string()),
                dest: None,
                mode: None,
            },
            InstallFileSpec {
                kind: FileKind::Data,
                source: None,
                dest: Some("{etcdir}/dest-only".to_string()),
                mode: Some("0644".to_string()),
            },
        ];
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::TarGz,
            "tar",
            vec!["user".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("linux", "x86_64");
        let layout = FsLayout::user(PathBuf::from("/tmp/home"));

        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "user",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        let comp_plan = &plan.components[0];

        assert_eq!(comp_plan.files.len(), 3);
        assert_eq!(
            comp_plan.files[0].source.as_deref(),
            Some("target/release/agentsight")
        );
        assert_eq!(
            comp_plan.files[0].dest.as_deref(),
            Some("{bindir}/agentsight")
        );
        assert_eq!(
            comp_plan.files[1].source.as_deref(),
            Some("{datadir}/source-only")
        );
        assert_eq!(comp_plan.files[1].dest, None);
        assert_eq!(comp_plan.files[2].source, None);
        assert_eq!(
            comp_plan.files[2].dest.as_deref(),
            Some("{etcdir}/dest-only")
        );
        assert!(comp_plan.resolved_files[0].ends_with("/.local/bin/agentsight"));
        assert!(comp_plan.resolved_files[1].ends_with("/.local/share/anolisa/source-only"));
        assert!(comp_plan.resolved_files[2].ends_with("/.config/anolisa/dest-only"));
    }

    #[test]
    fn render_files_accepts_minimal_schema_dir_aliases() {
        let layout = FsLayout::user(PathBuf::from("/tmp/home"));
        let files = vec![
            InstallFileSpec {
                kind: FileKind::Config,
                source: None,
                dest: Some("{sysconfdir}/app.conf".to_string()),
                mode: None,
            },
            InstallFileSpec {
                kind: FileKind::Config,
                source: None,
                dest: Some("{etcdir}/app.conf".to_string()),
                mode: None,
            },
            InstallFileSpec {
                kind: FileKind::Data,
                source: None,
                dest: Some("{sharedir}/asset".to_string()),
                mode: None,
            },
            InstallFileSpec {
                kind: FileKind::Data,
                source: None,
                dest: Some("{datadir}/asset".to_string()),
                mode: None,
            },
        ];
        let rendered = render_files(&files, &layout);
        // The minimal-schema `{sysconfdir}` resolves identically to the legacy
        // `{etcdir}`; both must point at the same on-disk config directory.
        assert_eq!(rendered[0], rendered[1]);
        // Likewise `{sharedir}` is an alias of the legacy `{datadir}`.
        assert_eq!(rendered[2], rendered[3]);
        assert!(
            !rendered[0].contains('{'),
            "alias left unexpanded: {rendered:?}"
        );
        assert!(rendered[0].ends_with("/anolisa/app.conf"));
        assert!(rendered[2].ends_with("/anolisa/asset"));
    }

    #[test]
    fn plan_preserves_install_capability_caps() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let mut comp = make_component("agentsight", "0.2.0", vec!["user".to_string()], Vec::new());
        comp.install.capabilities = vec![InstallCapabilitySpec {
            path: Some("{bindir}/agentsight".to_string()),
            caps: vec!["cap_bpf".to_string(), "cap_perfmon".to_string()],
        }];
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::TarGz,
            "tar",
            vec!["user".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("linux", "x86_64");
        let layout = FsLayout::user(PathBuf::from("/tmp/home"));

        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "user",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.components[0].capabilities.len(), 1);
        assert_eq!(
            plan.components[0].capabilities[0].path.as_deref(),
            Some("{bindir}/agentsight")
        );
        assert_eq!(
            plan.components[0].capabilities[0].caps,
            vec!["cap_bpf", "cap_perfmon"]
        );
    }

    /// Pin the "component contract driven" promise: a capability that
    /// lists two components must produce one [`ComponentPlan`] per
    /// component in source order, with no special-casing by component
    /// name. If a future refactor accidentally re-introduces a
    /// `match comp.component.name.as_str()` branch in the planner this
    /// test will fail.
    #[test]
    fn plan_iterates_every_component_in_capability_order() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string(), "second-sample".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let primary = make_component("agentsight", "0.2.0", vec!["user".to_string()], Vec::new());
        let secondary = ComponentManifest {
            schema_version: 2,
            component: ComponentMeta {
                name: "second-sample".to_string(),
                version: "1.0.0".to_string(),
                layer: "runtime".to_string(),
                domain: None,
                display_name: None,
                owner: None,
                license: None,
                repository: None,
            },
            contract: Default::default(),
            artifact: Default::default(),
            source: SourceSpec::default(),
            distribution_selectors: Vec::new(),
            build: BuildSpec::default(),
            install: InstallSpec {
                modes: vec!["user".to_string()],
                files: vec![InstallFileSpec {
                    kind: FileKind::Data,
                    source: None,
                    dest: Some("{bindir}/second".to_string()),
                    mode: None,
                }],
                services: Vec::new(),
                capabilities: Vec::new(),
            },
            env_requirements: EnvRequirements::default(),
            dependencies: DependenciesSpec::default(),
            features: Vec::<FeatureSpec>::new(),
            adapters: Vec::new(),
            health_check: None,
            health_checks: Vec::new(),
        };
        let catalog = make_catalog(vec![cap], vec![primary, secondary]);
        let index = make_index(vec![
            agentsight_entry(
                "0.2.0",
                ArtifactType::TarGz,
                "tar_gz",
                vec!["user".to_string()],
                "linux",
                "x86_64",
                Some("anolis23".to_string()),
            ),
            DistributionEntry {
                component: "second-sample".to_string(),
                version: "1.0.0".to_string(),
                channel: "stable".to_string(),
                artifact_type: ArtifactType::Binary,
                backend: "binary".to_string(),
                url: "file:///tmp/second-1.0.0".to_string(),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                libc: Some("glibc".to_string()),
                pkg_base: Some("anolis23".to_string()),
                install_modes: vec!["user".to_string()],
                sha256: Some("1".repeat(64)),
                signature: None,
                artifact_id: None,
                manifest_digest: None,
                size: None,
                signature_url: None,
                os_version: None,
                dependencies: Vec::new(),
            },
        ]);
        let env = make_env("linux", "x86_64");
        let layout = FsLayout::user(PathBuf::from("/tmp/home"));

        let plan = plan_enable(
            &catalog,
            &index,
            &env,
            "user",
            &layout,
            "agent-observability",
        )
        .expect("plan");

        assert_eq!(plan.components.len(), 2);
        assert_eq!(plan.components[0].name, "agentsight");
        assert_eq!(plan.components[1].name, "second-sample");
        // Per-component artifact resolution must be independent: both
        // components got their own resolved artifact, neither inherited
        // from the other.
        assert!(plan.components[0].artifact.is_some());
        assert!(plan.components[1].artifact.is_some());
        assert_eq!(
            plan.components[1].artifact.as_ref().unwrap().version,
            "1.0.0"
        );
    }

    /// `set_execute_gate(true, _)` must not pollute the plan's
    /// human-facing channels. Empty warnings stay empty; the planner's
    /// `next_actions` survive intact so consumers still see how to run.
    #[test]
    fn set_execute_gate_open_is_inert_on_a_clean_ready_plan() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::TarGz,
            "tar_gz",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let mut plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        let original_actions = plan.next_actions.clone();
        let original_warnings = plan.warnings.clone();
        plan.set_execute_gate(true, None);
        let gate = plan.execute_gate.as_ref().expect("gate populated");
        assert!(gate.allowed);
        assert!(gate.reason.is_none());
        assert_eq!(
            plan.next_actions, original_actions,
            "open gate must leave next_actions intact",
        );
        assert_eq!(
            plan.warnings, original_warnings,
            "open gate must leave warnings intact",
        );
    }

    /// On a structurally-Blocked plan, closing the gate must NOT erase
    /// the planner's "resolve the blocker first" hint — that hint
    /// describes a different problem (manifest/env/lint) the user has
    /// to fix regardless of CLI policy. The gate still records as
    /// closed and the warning is still injected so the wire surface
    /// stays consistent.
    #[test]
    fn set_execute_gate_closed_on_blocked_plan_preserves_blocker_next_actions() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        // rpm forces a structural Blocked via the unsupported-artifact lint.
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::Rpm,
            "rpm",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let mut plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");
        assert_eq!(plan.status, PlanStatus::Blocked);
        let blocker_actions = plan.next_actions.clone();

        plan.set_execute_gate(false, Some("closed".to_string()));
        // Gate populated.
        let gate = plan.execute_gate.as_ref().expect("gate populated");
        assert!(!gate.allowed);
        // Warning injected exactly once.
        assert_eq!(
            plan.warnings
                .iter()
                .filter(|w| w.starts_with("execute gate"))
                .count(),
            1,
        );
        // Structural blocker advice survived.
        assert_eq!(
            plan.next_actions, blocker_actions,
            "Blocked-status next_actions must not be overwritten by the gate",
        );
    }

    /// `set_execute_gate` is idempotent on repeat-close: a second
    /// closed call must not duplicate the "execute gate" warning.
    #[test]
    fn set_execute_gate_closed_twice_does_not_duplicate_warning() {
        let cap = make_cap(
            "agent-observability",
            vec!["agentsight".to_string()],
            vec!["linux".to_string()],
            vec!["x86_64".to_string()],
        );
        let comp = make_component(
            "agentsight",
            "0.2.0",
            vec!["system".to_string()],
            Vec::new(),
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = make_index(vec![agentsight_entry(
            "0.2.0",
            ArtifactType::TarGz,
            "tar_gz",
            vec!["system".to_string()],
            "linux",
            "x86_64",
            Some("anolis23".to_string()),
        )]);
        let env = make_env("linux", "x86_64");
        let layout = make_layout();
        let mut plan = plan_enable(
            &catalog,
            &index,
            &env,
            "system",
            &layout,
            "agent-observability",
        )
        .expect("plan");

        plan.set_execute_gate(false, Some("closed".to_string()));
        plan.set_execute_gate(false, Some("closed".to_string()));
        assert_eq!(
            plan.warnings
                .iter()
                .filter(|w| w.starts_with("execute gate"))
                .count(),
            1,
            "repeat-close must not duplicate the execute gate warning",
        );
    }
}
