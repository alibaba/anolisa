//! Raw-backend runtime-dependency preflight.
//!
//! Before the raw backend lays any files, it probes each declared
//! [`RuntimeDependency`] and fails fast on a miss — turning "install succeeded,
//! service silently dead" into a clear remediation. The RPM backend never runs
//! this: dnf resolves `Requires` instead, so a dependency is never resolved
//! twice. Check-only by design: the resolver reports what to install but never
//! mutates the host.

use crate::manifest::{DependencyKind, RuntimeDependency};
use anolisa_platform::command::{CommandRunner, SystemCommandRunner};

/// Host facts the preflight needs, decoupled from `anolisa_env::EnvFacts` so
/// callers (and tests) supply only the relevant slice.
#[derive(Debug, Clone, Default)]
pub struct ResolverEnv {
    /// Kernel release (`uname -r`), e.g. `5.10.134-007.ali5000`. Gates
    /// `min_kernel` declarations.
    pub kernel: Option<String>,
    /// Coarse package-base family (`"rpm"` / `"deb"`) used for remediation
    /// commands and native package queries. `None` → unsupported package
    /// manager.
    pub pkg_base: Option<String>,
    /// Whether kernel BTF is available (`/sys/kernel/btf/vmlinux`).
    pub btf: Option<bool>,
    /// Whether `CAP_BPF` is available.
    pub cap_bpf: Option<bool>,
}

/// Outcome of probing one dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyStatus {
    /// Present, and any verifiable version constraint is satisfied.
    Resolved,
    /// Missing but installable: carries the command the user should run.
    Unresolved {
        /// Remediation command or instruction (e.g. `sudo dnf install
        /// btrfs-progs`).
        remediation: String,
    },
    /// Missing and not installable by any package manager (kernel/capability):
    /// the install cannot proceed on this host.
    Unresolvable {
        /// Why the host cannot satisfy this dependency.
        reason: String,
    },
}

/// Per-dependency preflight result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyResolution {
    /// Logical dependency name.
    pub name: String,
    /// Resolution bucket the dependency was dispatched through.
    pub kind: DependencyKind,
    /// Probe outcome.
    pub status: DependencyStatus,
    /// Optional human note (e.g. "version not verified against '>=20'").
    pub detail: Option<String>,
}

/// Aggregate preflight result over all declared dependencies.
#[derive(Debug, Clone, Default)]
pub struct ResolutionPlan {
    /// One entry per declared dependency, in declaration order.
    pub resolutions: Vec<DependencyResolution>,
    /// Non-fatal notes collected during resolution.
    pub warnings: Vec<String>,
}

impl ResolutionPlan {
    /// Whether every dependency resolved. `false` if any is `Unresolved` or
    /// `Unresolvable` — the install must not proceed.
    pub fn is_satisfied(&self) -> bool {
        self.resolutions
            .iter()
            .all(|r| matches!(r.status, DependencyStatus::Resolved))
    }

    /// One line per unsatisfied dependency, for a single fail-fast message
    /// listing every miss at once.
    pub fn unsatisfied_lines(&self) -> Vec<String> {
        self.resolutions
            .iter()
            .filter_map(|r| match &r.status {
                DependencyStatus::Resolved => None,
                DependencyStatus::Unresolved { remediation } => {
                    Some(format!("{} [{}]: {remediation}", r.name, r.kind.as_str()))
                }
                DependencyStatus::Unresolvable { reason } => {
                    Some(format!("{} [{}]: {reason}", r.name, r.kind.as_str()))
                }
            })
            .collect()
    }
}

/// Failure that means the contract itself is wrong. Never raised for a merely
/// missing dependency — that is a [`DependencyStatus`], not an error.
#[derive(Debug, thiserror::Error)]
pub enum ResolverError {
    /// A `platform-capability` dependency named a `check` the resolver does not
    /// implement.
    #[error("dependency '{name}' has unknown platform-capability check '{check}'")]
    UnknownCheck {
        /// Dependency name.
        name: String,
        /// The unrecognized check identifier.
        check: String,
    },
}

/// Probes declared runtime dependencies against the host. Generic over the
/// command runner so tests inject a fake; the default runs real commands.
pub struct DependencyResolver<R: CommandRunner = SystemCommandRunner> {
    runner: R,
}

impl DependencyResolver<SystemCommandRunner> {
    /// Build a resolver that runs real host commands.
    pub fn system() -> Self {
        Self {
            runner: SystemCommandRunner,
        }
    }
}

impl<R: CommandRunner> DependencyResolver<R> {
    /// Build a resolver backed by a custom runner (primarily for tests).
    pub fn with_runner(runner: R) -> Self {
        Self { runner }
    }

    /// Probe every dependency and aggregate the outcome. Never mutates the host.
    ///
    /// # Errors
    /// Returns [`ResolverError`] only when a dependency declaration is itself
    /// invalid (e.g. an unknown platform-capability `check`); a missing
    /// dependency is reported as a [`DependencyStatus`], not an error.
    pub fn resolve(
        &self,
        deps: &[RuntimeDependency],
        env: &ResolverEnv,
    ) -> Result<ResolutionPlan, ResolverError> {
        let mut plan = ResolutionPlan::default();
        for dep in deps {
            let (status, detail) = match dep.kind {
                DependencyKind::SystemPackage => self.resolve_system_package(dep, env),
                DependencyKind::LanguageRuntime => self.resolve_language_runtime(dep),
                DependencyKind::PlatformCapability => resolve_platform_capability(dep, env)?,
            };
            plan.resolutions.push(DependencyResolution {
                name: dep.name.clone(),
                kind: dep.kind,
                status,
                detail,
            });
        }
        Ok(plan)
    }

    /// System package: present → resolved; missing → remediation command for
    /// the host package manager. Presence-first (no version gate in MVP).
    fn resolve_system_package(
        &self,
        dep: &RuntimeDependency,
        env: &ResolverEnv,
    ) -> (DependencyStatus, Option<String>) {
        let present = match &dep.probe {
            Some(probe) => matches!(self.run_probe(probe), ProbeOutcome::Present { .. }),
            None => self.native_package_present(dep, env),
        };
        if present {
            return (DependencyStatus::Resolved, None);
        }
        (
            DependencyStatus::Unresolved {
                remediation: system_package_remediation(dep, env),
            },
            None,
        )
    }

    /// Language runtime: probe presence, then a presence-first version check.
    /// Missing → manual-install hint (no vendoring in MVP).
    fn resolve_language_runtime(
        &self,
        dep: &RuntimeDependency,
    ) -> (DependencyStatus, Option<String>) {
        let probe = dep
            .probe
            .clone()
            .unwrap_or_else(|| format!("{} --version", dep.name));
        let ProbeOutcome::Present { stdout } = self.run_probe(&probe) else {
            return (
                DependencyStatus::Unresolved {
                    remediation: language_runtime_hint(dep),
                },
                None,
            );
        };
        match version_verdict(dep.version.as_deref(), &stdout) {
            VersionVerdict::Ok => (DependencyStatus::Resolved, None),
            VersionVerdict::NotVerified => (
                DependencyStatus::Resolved,
                dep.version
                    .as_deref()
                    .map(|v| format!("version not verified against '{v}'")),
            ),
            VersionVerdict::Mismatch { found } => (
                DependencyStatus::Unresolved {
                    remediation: language_runtime_hint(dep),
                },
                Some(format!(
                    "found {found}, need {}",
                    dep.version.as_deref().unwrap_or("")
                )),
            ),
        }
    }

    /// Run a `program arg arg` probe string. Present iff the command spawns and
    /// exits 0; a spawn failure (missing binary) counts as absent.
    fn run_probe(&self, probe: &str) -> ProbeOutcome {
        let mut parts = probe.split_whitespace();
        let Some(program) = parts.next() else {
            return ProbeOutcome::Absent;
        };
        let args: Vec<&str> = parts.collect();
        match self.runner.run(program, &args) {
            Ok(out) if out.code == Some(0) => ProbeOutcome::Present { stdout: out.stdout },
            _ => ProbeOutcome::Absent,
        }
    }

    /// Native presence query when a system package has no explicit probe:
    /// `rpm -q` / `dpkg -s`. Unknown package manager → treated as absent (the
    /// remediation then reports the unsupported manager).
    fn native_package_present(&self, dep: &RuntimeDependency, env: &ResolverEnv) -> bool {
        let (program, args): (&str, [&str; 2]) = match env.pkg_base.as_deref() {
            Some("rpm") => (
                "rpm",
                ["-q", dep.packages.rpm.as_deref().unwrap_or(&dep.name)],
            ),
            Some("deb") => (
                "dpkg",
                ["-s", dep.packages.deb.as_deref().unwrap_or(&dep.name)],
            ),
            _ => return false,
        };
        matches!(self.runner.run(program, &args), Ok(out) if out.code == Some(0))
    }
}

/// Probe result, carrying stdout for an optional version parse.
enum ProbeOutcome {
    Present { stdout: String },
    Absent,
}

/// Platform capability: gate `min_kernel` first, then evaluate the built-in
/// `check`. Never installs; a miss is `Unresolvable`.
fn resolve_platform_capability(
    dep: &RuntimeDependency,
    env: &ResolverEnv,
) -> Result<(DependencyStatus, Option<String>), ResolverError> {
    if let Some(min) = &dep.min_kernel {
        match kernel_satisfies(env.kernel.as_deref(), min) {
            KernelCheck::Satisfied => {}
            KernelCheck::Below { have } => {
                return Ok((
                    DependencyStatus::Unresolvable {
                        reason: format!("requires kernel >= {min}, host is {have}"),
                    },
                    None,
                ));
            }
            KernelCheck::Unknown => {
                return Ok((
                    DependencyStatus::Unresolvable {
                        reason: format!(
                            "requires kernel >= {min}, but the host kernel could not be determined"
                        ),
                    },
                    None,
                ));
            }
        }
    }

    if let Some(check) = &dep.check {
        let result = evaluate_check(check, env).ok_or_else(|| ResolverError::UnknownCheck {
            name: dep.name.clone(),
            check: check.clone(),
        })?;
        return Ok(match result {
            CheckResult::Supported => (DependencyStatus::Resolved, None),
            CheckResult::Unsupported { reason } => {
                (DependencyStatus::Unresolvable { reason }, None)
            }
        });
    }

    // A min_kernel-only declaration that passed the gate has nothing left to
    // verify.
    Ok((DependencyStatus::Resolved, None))
}

/// Remediation command for a missing system package, by host package format.
fn system_package_remediation(dep: &RuntimeDependency, env: &ResolverEnv) -> String {
    match env.pkg_base.as_deref() {
        Some("rpm") => format!(
            "sudo dnf install {}",
            dep.packages.rpm.as_deref().unwrap_or(&dep.name)
        ),
        Some("deb") => format!(
            "sudo apt install {}",
            dep.packages.deb.as_deref().unwrap_or(&dep.name)
        ),
        _ => format!(
            "unsupported package manager — install '{}' with the host package manager",
            dep.name
        ),
    }
}

/// Manual-install hint for a missing language runtime (vendoring is a later
/// phase, so MVP only tells the user what to install).
fn language_runtime_hint(dep: &RuntimeDependency) -> String {
    let version = dep
        .version
        .as_deref()
        .map(|v| format!(" {v}"))
        .unwrap_or_default();
    let source = dep
        .source
        .as_deref()
        .map(|s| format!(" (source: {s})"))
        .unwrap_or_default();
    format!("install {}{version} manually{source}", dep.name)
}

/// Result of a built-in platform-capability check.
enum CheckResult {
    Supported,
    Unsupported { reason: String },
}

/// Dispatch a built-in `check` identifier. `None` → unknown identifier (a
/// contract bug the caller turns into [`ResolverError::UnknownCheck`]). The set
/// is intentionally small; extend it deliberately.
fn evaluate_check(check: &str, env: &ResolverEnv) -> Option<CheckResult> {
    match check {
        "btf" => Some(bool_fact(env.btf, "kernel BTF (/sys/kernel/btf/vmlinux)")),
        "cap_bpf" => Some(bool_fact(env.cap_bpf, "CAP_BPF capability")),
        "btrfs" => Some(btrfs_supported()),
        _ => None,
    }
}

/// Map an `Option<bool>` env fact to a check result. `None` is conservative —
/// "could not determine" fails the preflight, since the bug we're fixing is a
/// silent miss.
fn bool_fact(fact: Option<bool>, label: &str) -> CheckResult {
    match fact {
        Some(true) => CheckResult::Supported,
        Some(false) => CheckResult::Unsupported {
            reason: format!("{label} is not available on this host"),
        },
        None => CheckResult::Unsupported {
            reason: format!("{label} could not be determined on this host"),
        },
    }
}

/// Whether the running kernel supports btrfs, read from `/proc/filesystems`.
fn btrfs_supported() -> CheckResult {
    match std::fs::read_to_string("/proc/filesystems") {
        Ok(contents) if fs_supported(&contents, "btrfs") => CheckResult::Supported,
        Ok(_) => CheckResult::Unsupported {
            reason: "btrfs is not supported by the running kernel (absent from /proc/filesystems)"
                .to_string(),
        },
        Err(_) => CheckResult::Unsupported {
            reason: "could not read /proc/filesystems to verify btrfs support".to_string(),
        },
    }
}

/// Whether `proc_filesystems` content lists `fs`. Each line is `nodev\t<fs>` or
/// `\t<fs>`; the filesystem name is the trailing token.
fn fs_supported(proc_filesystems: &str, fs: &str) -> bool {
    proc_filesystems
        .lines()
        .any(|line| line.split_whitespace().last() == Some(fs))
}

/// Result of comparing the host kernel against a `min_kernel` requirement.
enum KernelCheck {
    Satisfied,
    Below { have: String },
    Unknown,
}

/// Compare the host kernel release against a minimum. Either side unparseable
/// → `Unknown` (conservative: the preflight then refuses rather than guessing).
fn kernel_satisfies(have: Option<&str>, min: &str) -> KernelCheck {
    let Some(have_raw) = have else {
        return KernelCheck::Unknown;
    };
    match (parse_kernel(have_raw), parse_kernel(min)) {
        (Some(have_v), Some(min_v)) if have_v >= min_v => KernelCheck::Satisfied,
        (Some(_), Some(_)) => KernelCheck::Below {
            have: have_raw.to_string(),
        },
        _ => KernelCheck::Unknown,
    }
}

/// Parse a kernel release's leading `MAJOR.MINOR[.PATCH]` into a semver
/// `Version`, ignoring any `-suffix`: `5.10.134-007.ali5000` → `5.10.134`,
/// `5.4` → `5.4.0`.
fn parse_kernel(s: &str) -> Option<semver::Version> {
    let head = s.split('-').next()?.trim();
    let mut parts = head.split('.');
    let major = leading_u64(parts.next()?)?;
    let minor = parts.next().and_then(leading_u64).unwrap_or(0);
    let patch = parts.next().and_then(leading_u64).unwrap_or(0);
    Some(semver::Version::new(major, minor, patch))
}

/// Leading run of ASCII digits parsed as `u64` (`"134"` from `"134abc"`).
fn leading_u64(s: &str) -> Option<u64> {
    let digits: String = s
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Outcome of a presence-first version check.
enum VersionVerdict {
    /// No constraint, or constraint satisfied.
    Ok,
    /// Constraint set but unverifiable (either side unparseable). Not a failure.
    NotVerified,
    /// Constraint set, a version was found, and it does not satisfy.
    Mismatch { found: String },
}

/// Presence-first version check: only a confidently-parsed mismatch fails;
/// anything ambiguous is `NotVerified` (never silently downgraded).
fn version_verdict(constraint: Option<&str>, stdout: &str) -> VersionVerdict {
    let Some(constraint) = constraint else {
        return VersionVerdict::Ok;
    };
    let Ok(req) = semver::VersionReq::parse(constraint) else {
        return VersionVerdict::NotVerified;
    };
    let Some(found) = extract_semver(stdout) else {
        return VersionVerdict::NotVerified;
    };
    if req.matches(&found) {
        VersionVerdict::Ok
    } else {
        VersionVerdict::Mismatch {
            found: found.to_string(),
        }
    }
}

/// First whitespace-delimited token in `stdout` that parses as semver
/// (tolerating a leading `v`): `node --version` → `v20.3.1` → `20.3.1`.
fn extract_semver(stdout: &str) -> Option<semver::Version> {
    stdout.split_whitespace().find_map(|tok| {
        let t = tok.strip_prefix('v').unwrap_or(tok);
        semver::Version::parse(t).ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anolisa_platform::command::CommandOutput;
    use std::collections::HashMap;
    use std::io;

    /// Canned outcome for one program under the fake runner.
    enum Fake {
        Ok(CommandOutput),
        Spawn(io::ErrorKind),
    }

    /// Maps a program name to a canned result. An unmapped program spawns
    /// `NotFound`, mirroring a missing binary on the host.
    #[derive(Default)]
    struct FakeRunner {
        map: HashMap<String, Fake>,
    }

    impl FakeRunner {
        fn ok(mut self, program: &str, code: i32, stdout: &str) -> Self {
            self.map.insert(
                program.to_string(),
                Fake::Ok(CommandOutput {
                    code: Some(code),
                    stdout: stdout.to_string(),
                    stderr: String::new(),
                }),
            );
            self
        }
        fn missing(mut self, program: &str) -> Self {
            self.map
                .insert(program.to_string(), Fake::Spawn(io::ErrorKind::NotFound));
            self
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, program: &str, _args: &[&str]) -> io::Result<CommandOutput> {
            match self.map.get(program) {
                Some(Fake::Ok(out)) => Ok(out.clone()),
                Some(Fake::Spawn(kind)) => Err(io::Error::new(*kind, "fake spawn failure")),
                None => Err(io::Error::new(io::ErrorKind::NotFound, "missing binary")),
            }
        }
    }

    fn dep(name: &str, kind: DependencyKind) -> RuntimeDependency {
        RuntimeDependency {
            name: name.to_string(),
            kind,
            version: None,
            probe: None,
            source: None,
            packages: crate::manifest::PackageNames::default(),
            check: None,
            min_kernel: None,
        }
    }

    fn rpm_env() -> ResolverEnv {
        ResolverEnv {
            pkg_base: Some("rpm".to_string()),
            ..Default::default()
        }
    }

    fn resolve_one(
        runner: FakeRunner,
        d: RuntimeDependency,
        env: &ResolverEnv,
    ) -> DependencyResolution {
        let plan = DependencyResolver::with_runner(runner)
            .resolve(&[d], env)
            .expect("resolve");
        plan.resolutions.into_iter().next().expect("one resolution")
    }

    #[test]
    fn system_package_present_is_resolved() {
        let mut d = dep("btrfs-progs", DependencyKind::SystemPackage);
        d.probe = Some("btrfs version".to_string());
        // Only the probe binary is faked — if native `rpm -q` were used instead,
        // it would be missing and the dep would be unresolved. Resolving proves
        // the explicit probe is preferred.
        let r = resolve_one(
            FakeRunner::default().ok("btrfs", 0, "btrfs-progs v6.6"),
            d,
            &rpm_env(),
        );
        assert_eq!(r.status, DependencyStatus::Resolved);
    }

    #[test]
    fn system_package_missing_rpm_remediation() {
        let mut d = dep("btrfs-progs", DependencyKind::SystemPackage);
        d.probe = Some("btrfs version".to_string());
        d.packages.rpm = Some("btrfs-progs".to_string());
        let r = resolve_one(FakeRunner::default().missing("btrfs"), d, &rpm_env());
        assert_eq!(
            r.status,
            DependencyStatus::Unresolved {
                remediation: "sudo dnf install btrfs-progs".to_string()
            }
        );
    }

    #[test]
    fn system_package_missing_deb_remediation() {
        let mut d = dep("btrfs-progs", DependencyKind::SystemPackage);
        d.probe = Some("btrfs version".to_string());
        d.packages.deb = Some("btrfs-progs".to_string());
        let env = ResolverEnv {
            pkg_base: Some("deb".to_string()),
            ..Default::default()
        };
        let r = resolve_one(FakeRunner::default().missing("btrfs"), d, &env);
        assert_eq!(
            r.status,
            DependencyStatus::Unresolved {
                remediation: "sudo apt install btrfs-progs".to_string()
            }
        );
    }

    #[test]
    fn system_package_unknown_pkg_base_manual_hint() {
        let mut d = dep("btrfs-progs", DependencyKind::SystemPackage);
        d.probe = Some("btrfs version".to_string());
        let env = ResolverEnv::default(); // pkg_base = None
        let r = resolve_one(FakeRunner::default().missing("btrfs"), d, &env);
        match r.status {
            DependencyStatus::Unresolved { remediation } => {
                assert!(
                    remediation.contains("unsupported package manager"),
                    "{remediation}"
                );
            }
            other => panic!("expected unresolved, got {other:?}"),
        }
    }

    #[test]
    fn system_package_native_query_present_when_no_probe() {
        let d = dep("btrfs-progs", DependencyKind::SystemPackage);
        // No probe → native `rpm -q` path; exit 0 means installed.
        let r = resolve_one(FakeRunner::default().ok("rpm", 0, ""), d, &rpm_env());
        assert_eq!(r.status, DependencyStatus::Resolved);
    }

    #[test]
    fn system_package_native_query_absent_when_no_probe() {
        let d = dep("btrfs-progs", DependencyKind::SystemPackage);
        let r = resolve_one(FakeRunner::default().ok("rpm", 1, ""), d, &rpm_env());
        assert!(matches!(r.status, DependencyStatus::Unresolved { .. }));
    }

    #[test]
    fn platform_capability_min_kernel_below_is_unresolvable() {
        let mut d = dep("btrfs", DependencyKind::PlatformCapability);
        d.min_kernel = Some("5.4".to_string());
        let env = ResolverEnv {
            kernel: Some("3.10.0-1160.el7".to_string()),
            ..Default::default()
        };
        let r = resolve_one(FakeRunner::default(), d, &env);
        match r.status {
            DependencyStatus::Unresolvable { reason } => {
                assert!(reason.contains("requires kernel >= 5.4"), "{reason}");
            }
            other => panic!("expected unresolvable, got {other:?}"),
        }
    }

    #[test]
    fn platform_capability_min_kernel_satisfied_is_resolved() {
        let mut d = dep("btrfs", DependencyKind::PlatformCapability);
        d.min_kernel = Some("5.4".to_string());
        let env = ResolverEnv {
            kernel: Some("5.10.134-007.ali5000.al8.x86_64".to_string()),
            ..Default::default()
        };
        let r = resolve_one(FakeRunner::default(), d, &env);
        assert_eq!(r.status, DependencyStatus::Resolved);
    }

    #[test]
    fn platform_capability_min_kernel_unknown_host_is_unresolvable() {
        let mut d = dep("btrfs", DependencyKind::PlatformCapability);
        d.min_kernel = Some("5.4".to_string());
        let env = ResolverEnv::default(); // kernel = None
        let r = resolve_one(FakeRunner::default(), d, &env);
        assert!(matches!(r.status, DependencyStatus::Unresolvable { .. }));
    }

    #[test]
    fn platform_capability_btf_supported_and_missing() {
        let mut d = dep("ebpf", DependencyKind::PlatformCapability);
        d.check = Some("btf".to_string());
        let yes = ResolverEnv {
            btf: Some(true),
            ..Default::default()
        };
        let no = ResolverEnv {
            btf: Some(false),
            ..Default::default()
        };
        let none = ResolverEnv {
            btf: None,
            ..Default::default()
        };
        assert_eq!(
            resolve_one(FakeRunner::default(), d.clone(), &yes).status,
            DependencyStatus::Resolved
        );
        assert!(matches!(
            resolve_one(FakeRunner::default(), d.clone(), &no).status,
            DependencyStatus::Unresolvable { .. }
        ));
        assert!(matches!(
            resolve_one(FakeRunner::default(), d, &none).status,
            DependencyStatus::Unresolvable { .. }
        ));
    }

    #[test]
    fn platform_capability_unknown_check_is_error() {
        let mut d = dep("frob", DependencyKind::PlatformCapability);
        d.check = Some("frobnicate".to_string());
        let err = DependencyResolver::with_runner(FakeRunner::default())
            .resolve(&[d], &ResolverEnv::default())
            .expect_err("unknown check must error");
        assert!(matches!(err, ResolverError::UnknownCheck { .. }));
    }

    #[test]
    fn language_runtime_present_satisfies_version() {
        let mut d = dep("node", DependencyKind::LanguageRuntime);
        d.version = Some(">=20".to_string());
        let r = resolve_one(
            FakeRunner::default().ok("node", 0, "v20.3.1"),
            d,
            &ResolverEnv::default(),
        );
        assert_eq!(r.status, DependencyStatus::Resolved);
    }

    #[test]
    fn language_runtime_version_mismatch_is_unresolved() {
        let mut d = dep("node", DependencyKind::LanguageRuntime);
        d.version = Some(">=20".to_string());
        let r = resolve_one(
            FakeRunner::default().ok("node", 0, "v18.19.0"),
            d,
            &ResolverEnv::default(),
        );
        assert!(matches!(r.status, DependencyStatus::Unresolved { .. }));
        assert!(r.detail.unwrap().contains("found 18.19.0"));
    }

    #[test]
    fn language_runtime_missing_reports_manual_hint_no_install_cmd() {
        let mut d = dep("node", DependencyKind::LanguageRuntime);
        d.version = Some(">=20".to_string());
        d.source = Some("nodejs-official".to_string());
        let r = resolve_one(
            FakeRunner::default().missing("node"),
            d,
            &ResolverEnv::default(),
        );
        match r.status {
            DependencyStatus::Unresolved { remediation } => {
                assert!(
                    remediation.contains("install node >=20 manually"),
                    "{remediation}"
                );
                assert!(remediation.contains("nodejs-official"), "{remediation}");
                // No package-manager install is issued for a language runtime.
                assert!(
                    !remediation.contains("dnf") && !remediation.contains("apt"),
                    "{remediation}"
                );
            }
            other => panic!("expected unresolved, got {other:?}"),
        }
    }

    #[test]
    fn language_runtime_unparseable_version_does_not_fail() {
        let mut d = dep("node", DependencyKind::LanguageRuntime);
        d.version = Some("latest".to_string()); // not a semver req
        let r = resolve_one(
            FakeRunner::default().ok("node", 0, "v20.3.1"),
            d,
            &ResolverEnv::default(),
        );
        assert_eq!(r.status, DependencyStatus::Resolved);
        assert!(r.detail.unwrap().contains("not verified"));
    }

    #[test]
    fn aggregate_fails_and_lists_all_missing() {
        let mut present = dep("present-pkg", DependencyKind::SystemPackage);
        present.probe = Some("present-tool x".to_string());
        let mut missing = dep("btrfs-progs", DependencyKind::SystemPackage);
        missing.probe = Some("btrfs version".to_string());
        missing.packages.rpm = Some("btrfs-progs".to_string());
        let mut cap = dep("btrfs", DependencyKind::PlatformCapability);
        cap.min_kernel = Some("5.4".to_string());

        let env = ResolverEnv {
            pkg_base: Some("rpm".to_string()),
            kernel: Some("3.10.0-1160".to_string()),
            ..Default::default()
        };
        let plan = DependencyResolver::with_runner(
            FakeRunner::default()
                .ok("present-tool", 0, "ok")
                .missing("btrfs"),
        )
        .resolve(&[present, missing, cap], &env)
        .expect("resolve");

        assert!(!plan.is_satisfied());
        let lines = plan.unsatisfied_lines();
        assert_eq!(lines.len(), 2, "both misses listed: {lines:?}");
        assert!(lines.iter().any(|l| l.contains("btrfs-progs")));
        assert!(
            lines
                .iter()
                .any(|l| l.contains("btrfs [platform-capability]"))
        );
    }

    #[test]
    fn aggregate_all_present_is_satisfied() {
        let mut a = dep("a", DependencyKind::SystemPackage);
        a.probe = Some("a v".to_string());
        let mut b = dep("b", DependencyKind::PlatformCapability);
        b.check = Some("btf".to_string());
        let env = ResolverEnv {
            btf: Some(true),
            ..Default::default()
        };
        let plan = DependencyResolver::with_runner(FakeRunner::default().ok("a", 0, "1"))
            .resolve(&[a, b], &env)
            .expect("resolve");
        assert!(plan.is_satisfied());
        assert!(plan.unsatisfied_lines().is_empty());
    }

    #[test]
    fn fs_supported_matches_trailing_token() {
        let procfs = "nodev\tsysfs\nnodev\ttmpfs\n\text4\n\tbtrfs\n";
        assert!(fs_supported(procfs, "btrfs"));
        assert!(fs_supported(procfs, "ext4"));
        assert!(!fs_supported(procfs, "xfs"));
    }

    #[test]
    fn parse_kernel_handles_vendor_suffixes() {
        assert_eq!(
            parse_kernel("5.10.134-007.ali5000.al8.x86_64"),
            Some(semver::Version::new(5, 10, 134))
        );
        assert_eq!(parse_kernel("5.4"), Some(semver::Version::new(5, 4, 0)));
        assert_eq!(
            parse_kernel("3.10.0-1160.el7"),
            Some(semver::Version::new(3, 10, 0))
        );
    }
}
