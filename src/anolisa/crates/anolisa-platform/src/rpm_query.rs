//! RPM/DNF backend for [`PackageQuery`].
//!
//! Uses `rpm -q` for installed packages and `dnf repoquery` for available
//! candidates. Output is parsed from a stable `--qf` pipe-delimited format
//! rather than the locale-sensitive default `nevra` string, so field
//! extraction does not depend on the host locale. The not-installed signal
//! still relies on an English message marker, which is pinned by
//! [`SystemCommandRunner`]'s `LC_ALL=C`.

use crate::command::{CommandOutput, CommandRunner, SystemCommandRunner};
use crate::pkg_query::{PackageInfo, PackageQuery, PackageQueryError, PackageVersion};

/// Pipe-delimited query format for `rpm -q` (installed packages).
///
/// Field order: NAME | EPOCH | VERSION | RELEASE | ARCH. `rpm -q` cannot
/// report a reponame, so installed queries leave [`PackageInfo::origin`] as
/// `None`; populating `source_repo` for observed packages is #958's job.
const INSTALLED_QF: &str = "%{NAME}|%{EPOCH}|%{VERSION}|%{RELEASE}|%{ARCH}\n";

/// Pipe-delimited query format for `dnf repoquery` (available candidates).
///
/// Adds `%{REPONAME}` over the installed format so available candidates carry
/// their source repo.
const AVAILABLE_QF: &str = "%{NAME}|%{EPOCH}|%{VERSION}|%{RELEASE}|%{ARCH}|%{REPONAME}\n";

/// Query format for the *installed* source repo (`dnf repoquery --installed`).
///
/// `rpm -q` cannot report a reponame, so `source_repo` for observed packages
/// must come from the dnf side; this yields just the bare reponame per line.
const REPONAME_QF: &str = "%{reponame}\n";

/// Query format for the provides reverse-lookup (`rpm -q --whatprovides`).
///
/// Takes the bare `%{NAME}` rather than the default NEVRA string so the result
/// is directly usable as a package name (the default `name-version-release.arch`
/// is not).
const PROVIDES_NAME_QF: &str = "%{NAME}\n";

const RPM: &str = "rpm";
const DNF: &str = "dnf";

/// RPM/DNF implementation of [`PackageQuery`].
///
/// Generic over the [`CommandRunner`] so tests can inject a fake; production
/// code uses [`RpmPackageQuery::system`]. The default type parameter keeps
/// call sites in production code parameter-free while staying zero-cost.
pub struct RpmPackageQuery<R: CommandRunner = SystemCommandRunner> {
    runner: R,
}

impl RpmPackageQuery<SystemCommandRunner> {
    /// Build a query that runs real `rpm`/`dnf` on the host.
    pub fn system() -> Self {
        Self {
            runner: SystemCommandRunner,
        }
    }
}

impl<R: CommandRunner> RpmPackageQuery<R> {
    /// Build a query backed by a custom runner (primarily for tests).
    pub fn with_runner(runner: R) -> Self {
        Self { runner }
    }

    /// List the absolute file paths an installed package owns (`rpm -q --list`).
    ///
    /// Used to discover the systemd unit files an RPM placed, so `anolisa
    /// restart` can drive the services a package owns even though the RPM
    /// install path records no `services` in state. Paths are returned verbatim
    /// (one per `rpm` output line, blank lines dropped); the caller classifies
    /// which are units and in which scope — this layer keeps no systemd
    /// knowledge. Not on [`PackageQuery`]: keeping it inherent avoids touching
    /// the trait's many fake implementations.
    ///
    /// # Errors
    /// - [`PackageQueryError::CommandMissing`] when `rpm` is absent.
    /// - [`PackageQueryError::QueryFailed`] on a non-zero exit. This includes
    ///   "package is not installed", which for a state-tracked component is
    ///   drift the caller should surface rather than silently treat as
    ///   "no units".
    pub fn list_files(&self, package: &str) -> Result<Vec<String>, PackageQueryError> {
        let out = self
            .runner
            .run(RPM, &["-q", "--list", package])
            .map_err(|e| map_spawn_error(e, RPM))?;

        if out.code != Some(0) {
            // rpm writes "is not installed" to stdout and hard errors to stderr;
            // surface whichever is non-empty so the caller sees the real cause.
            let detail = if out.stderr.trim().is_empty() {
                out.stdout.clone()
            } else {
                out.stderr.clone()
            };
            return Err(PackageQueryError::QueryFailed {
                command: RPM.to_string(),
                code: out.code,
                stderr: detail,
            });
        }

        Ok(out
            .stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect())
    }
}

impl<R: CommandRunner> PackageQuery for RpmPackageQuery<R> {
    fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
        let out = self
            .runner
            .run(RPM, &["-q", "--qf", INSTALLED_QF, package])
            .map_err(|e| map_spawn_error(e, RPM))?;

        if out.code == Some(0) {
            return parse_installed(&out);
        }

        // Non-zero exit: distinguish "not installed" from a real failure.
        // rpm writes the not-installed notice to stdout (structural signal:
        // hard errors go to stderr with stdout empty), and under LC_ALL=C the
        // English marker is stable. Both signals agree here.
        if out.stdout.contains("is not installed") {
            return Ok(None);
        }

        Err(PackageQueryError::QueryFailed {
            command: RPM.to_string(),
            code: out.code,
            stderr: out.stderr,
        })
    }

    fn query_available(&self, package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
        let out = self
            .runner
            .run(
                DNF,
                &["repoquery", "--quiet", "--qf", AVAILABLE_QF, package],
            )
            .map_err(|e| map_spawn_error(e, DNF))?;

        if out.code != Some(0) {
            return Err(PackageQueryError::QueryFailed {
                command: DNF.to_string(),
                code: out.code,
                stderr: out.stderr,
            });
        }

        out.stdout
            .lines()
            .filter(|line| !line.is_empty())
            .map(parse_available_line)
            .collect()
    }

    fn installed_origin(&self, package: &str) -> Result<Option<String>, PackageQueryError> {
        let out = self
            .runner
            .run(
                DNF,
                &["repoquery", "--installed", "--qf", REPONAME_QF, package],
            )
            .map_err(|e| map_spawn_error(e, DNF))?;

        if out.code != Some(0) {
            return Err(PackageQueryError::QueryFailed {
                command: DNF.to_string(),
                code: out.code,
                stderr: out.stderr,
            });
        }

        // First non-empty reponame line is the source repo (`@System`,
        // `anolisa-release`, ...); no line means the origin is unknown, which
        // is a non-fatal `None` rather than an error (it is supplementary
        // metadata for the adopt record).
        Ok(out
            .stdout
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_string))
    }

    fn what_provides_installed(&self, capability: &str) -> Result<Vec<String>, PackageQueryError> {
        let out = self
            .runner
            .run(
                RPM,
                &["-q", "--whatprovides", "--qf", PROVIDES_NAME_QF, capability],
            )
            .map_err(|e| map_spawn_error(e, RPM))?;

        if out.code == Some(0) {
            // De-dup by name: one package can match through several Provides
            // lines. Insertion order is preserved; provider counts are tiny.
            let mut names: Vec<String> = Vec::new();
            for name in out.stdout.lines().map(str::trim).filter(|l| !l.is_empty()) {
                if !names.iter().any(|n| n == name) {
                    names.push(name.to_string());
                }
            }
            return Ok(names);
        }

        // "nothing provides it" is a normal empty result, mirroring the
        // not-installed branch: rpm writes the notice to stdout, pinned to
        // English by LC_ALL=C.
        if out.stdout.contains("no package provides") {
            return Ok(Vec::new());
        }

        Err(PackageQueryError::QueryFailed {
            command: RPM.to_string(),
            code: out.code,
            stderr: out.stderr,
        })
    }
}

/// Map a spawn-phase [`std::io::Error`] to a query error by [`std::io::ErrorKind`].
///
/// Permission detection relies on the spawn-layer `PermissionDenied` rather
/// than sniffing backend error strings, which are not stable across locales
/// and versions.
fn map_spawn_error(e: std::io::Error, command: &str) -> PackageQueryError {
    match e.kind() {
        std::io::ErrorKind::NotFound => PackageQueryError::CommandMissing {
            command: command.to_string(),
        },
        std::io::ErrorKind::PermissionDenied => PackageQueryError::PermissionDenied {
            command: command.to_string(),
        },
        _ => PackageQueryError::QueryFailed {
            command: command.to_string(),
            code: None,
            stderr: e.to_string(),
        },
    }
}

/// Parse a successful `rpm -q` output into at most one [`PackageInfo`].
///
/// Enforces the single-instance invariant: multiple non-empty rows mean the
/// same package name has several installed versions, which is a drift state
/// for component-scoped queries and must not be silently collapsed to the
/// first row.
fn parse_installed(out: &CommandOutput) -> Result<Option<PackageInfo>, PackageQueryError> {
    let count = out.stdout.lines().filter(|l| !l.is_empty()).count();
    match count {
        0 => Err(PackageQueryError::UnexpectedOutput {
            command: RPM.to_string(),
            detail: "0 installed versions".to_string(),
        }),
        1 => {
            let line = out.stdout.lines().next().unwrap_or("");
            parse_installed_line(line).map(Some)
        }
        n => Err(PackageQueryError::UnexpectedOutput {
            command: RPM.to_string(),
            detail: format!("{n} installed versions"),
        }),
    }
}

/// Parse a single installed-package `--qf` line (5 pipe-delimited fields).
fn parse_installed_line(line: &str) -> Result<PackageInfo, PackageQueryError> {
    let parts: Vec<&str> = line.split('|').collect();
    if parts.len() != 5 {
        return Err(PackageQueryError::UnexpectedOutput {
            command: RPM.to_string(),
            detail: format!("expected 5 fields, got {}", parts.len()),
        });
    }
    Ok(PackageInfo {
        name: parts[0].to_string(),
        version: parse_version(parts[1], parts[2], parts[3]),
        arch: parts[4].to_string(),
        origin: None,
    })
}

/// Parse a single available-candidate `--qf` line (6 pipe-delimited fields).
fn parse_available_line(line: &str) -> Result<PackageInfo, PackageQueryError> {
    let parts: Vec<&str> = line.split('|').collect();
    if parts.len() != 6 {
        return Err(PackageQueryError::UnexpectedOutput {
            command: DNF.to_string(),
            detail: format!("expected 6 fields, got {}", parts.len()),
        });
    }
    Ok(PackageInfo {
        name: parts[0].to_string(),
        version: parse_version(parts[1], parts[2], parts[3]),
        arch: parts[4].to_string(),
        origin: Some(parts[5].to_string()),
    })
}

/// Build a [`PackageVersion`] from raw `--qf` epoch/version/release fields.
fn parse_version(epoch: &str, version: &str, release: &str) -> PackageVersion {
    PackageVersion {
        epoch: parse_epoch(epoch),
        version: version.to_string(),
        release: parse_release(release),
    }
}

/// Normalize epoch to `None` for the equivalent "no epoch" spellings.
///
/// `rpm -q` emits `(none)` for packages without an epoch while
/// `dnf repoquery` emits `0` for the same packages; RPM treats an absent
/// epoch as `0`, so the two are semantically identical. Collapsing both
/// (plus the empty string) to `None` keeps the installed and available
/// representations of the same package equal, so version comparisons do not
/// mistake an equivalent pair for drift.
fn parse_epoch(s: &str) -> Option<String> {
    match s {
        "(none)" | "" | "0" => None,
        other => Some(other.to_string()),
    }
}

/// Normalize release: empty means no release (native packages).
fn parse_release(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandOutput;
    use std::io;

    /// Preset result for the fake runner: either a captured output or a
    /// spawn-phase error kind to replay.
    enum FakeOutcome {
        Ok(CommandOutput),
        Err(io::ErrorKind),
    }

    /// Fake runner keyed by program name. Returns the canned outcome on each
    /// call; a program with no preset yields `NotFound` (surfacing as
    /// [`PackageQueryError::CommandMissing`]) rather than panicking.
    #[derive(Default)]
    struct FakeCommandRunner {
        rpm: Option<FakeOutcome>,
        dnf: Option<FakeOutcome>,
        expected_package: String,
    }

    impl CommandRunner for FakeCommandRunner {
        fn run(&self, program: &str, args: &[&str]) -> io::Result<CommandOutput> {
            assert_call_contract(program, args, &self.expected_package);
            let outcome = match program {
                RPM => self.rpm.as_ref(),
                DNF => self.dnf.as_ref(),
                _ => None,
            };
            match outcome {
                Some(FakeOutcome::Ok(o)) => Ok(o.clone()),
                Some(FakeOutcome::Err(kind)) => {
                    Err(io::Error::new(*kind, format!("fake {program} failure")))
                }
                None => Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no fake preset for {program}"),
                )),
            }
        }
    }

    /// Assert the implementation invokes each backend with the documented args.
    ///
    /// The fake returns canned output without inspecting `args`, so without
    /// these checks a regression that drops `--qf`, swaps the repoquery
    /// subcommand, omits the package argument, or passes the wrong package
    /// would still pass the output-based assertions.
    fn assert_call_contract(program: &str, args: &[&str], expected_package: &str) {
        match program {
            // `rpm -q --whatprovides --qf <fmt> <cap>` (what_provides_installed)
            RPM if args.get(1) == Some(&"--whatprovides") => {
                assert_eq!(
                    args.len(),
                    5,
                    "rpm whatprovides needs [-q, --whatprovides, --qf, <fmt>, <cap>]: {args:?}"
                );
                assert_eq!(args[0], "-q");
                assert_eq!(args[2], "--qf");
                assert_eq!(
                    args[3], PROVIDES_NAME_QF,
                    "rpm whatprovides --qf drifted from PROVIDES_NAME_QF: {args:?}"
                );
                assert_eq!(
                    args[4], expected_package,
                    "rpm capability argument must be last: {args:?}"
                );
            }
            // `rpm -q --list <pkg>` (list_files)
            RPM if args.get(1) == Some(&"--list") => {
                assert_eq!(
                    args.len(),
                    3,
                    "rpm list needs [-q, --list, <pkg>]: {args:?}"
                );
                assert_eq!(args[0], "-q");
                assert_eq!(
                    args[2], expected_package,
                    "rpm package argument must be last: {args:?}"
                );
            }
            // `rpm -q --qf <fmt> <pkg>` (query_installed)
            RPM => {
                assert_eq!(
                    args.len(),
                    4,
                    "rpm needs [-q, --qf, <fmt>, <pkg>]: {args:?}"
                );
                assert_eq!(args[0], "-q");
                assert_eq!(args[1], "--qf");
                assert_eq!(
                    args[2], INSTALLED_QF,
                    "rpm --qf format string drifted from INSTALLED_QF: {args:?}"
                );
                assert_eq!(
                    args[3], expected_package,
                    "rpm package argument must be last: {args:?}"
                );
            }
            // `dnf repoquery --installed --qf <fmt> <pkg>` (installed_origin)
            DNF if args.get(1) == Some(&"--installed") => {
                assert_eq!(
                    args.len(),
                    5,
                    "dnf installed-origin needs [repoquery, --installed, --qf, <fmt>, <pkg>]: {args:?}"
                );
                assert_eq!(args[0], "repoquery");
                assert_eq!(args[2], "--qf");
                assert_eq!(
                    args[3], REPONAME_QF,
                    "dnf installed --qf drifted from REPONAME_QF: {args:?}"
                );
                assert_eq!(
                    args[4], expected_package,
                    "dnf package argument must be last: {args:?}"
                );
            }
            // `dnf repoquery --quiet --qf <fmt> <pkg>` (query_available)
            DNF => {
                assert_eq!(
                    args.len(),
                    5,
                    "dnf needs [repoquery, --quiet, --qf, <fmt>, <pkg>]: {args:?}"
                );
                assert_eq!(args[0], "repoquery");
                assert_eq!(args[1], "--quiet");
                assert_eq!(args[2], "--qf");
                assert_eq!(
                    args[3], AVAILABLE_QF,
                    "dnf --qf format string drifted from AVAILABLE_QF: {args:?}"
                );
                assert_eq!(
                    args[4], expected_package,
                    "dnf package argument must be last: {args:?}"
                );
            }
            _ => {}
        }
    }

    fn ok_out(code: Option<i32>, stdout: &str, stderr: &str) -> FakeOutcome {
        FakeOutcome::Ok(CommandOutput {
            code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        })
    }

    fn query_with_rpm(
        expected_package: &str,
        outcome: FakeOutcome,
    ) -> RpmPackageQuery<FakeCommandRunner> {
        RpmPackageQuery::with_runner(FakeCommandRunner {
            rpm: Some(outcome),
            dnf: None,
            expected_package: expected_package.to_string(),
        })
    }

    fn query_with_dnf(
        expected_package: &str,
        outcome: FakeOutcome,
    ) -> RpmPackageQuery<FakeCommandRunner> {
        RpmPackageQuery::with_runner(FakeCommandRunner {
            rpm: None,
            dnf: Some(outcome),
            expected_package: expected_package.to_string(),
        })
    }

    #[test]
    fn installed_returns_info() {
        let q = query_with_rpm(
            "tokenless",
            ok_out(Some(0), "tokenless|(none)|2.0.1|1.al8|x86_64", ""),
        );
        let info = q
            .query_installed("tokenless")
            .unwrap()
            .expect("installed package should yield Some");
        assert_eq!(info.name, "tokenless");
        assert_eq!(info.version.epoch, None);
        assert_eq!(info.version.version, "2.0.1");
        assert_eq!(info.version.release.as_deref(), Some("1.al8"));
        assert_eq!(info.arch, "x86_64");
        assert_eq!(info.origin, None);
        assert_eq!(info.version.to_string(), "2.0.1-1.al8");
        assert!(q.is_installed("tokenless").unwrap());
    }

    #[test]
    fn not_installed_returns_none() {
        let q = query_with_rpm(
            "tokenless",
            ok_out(Some(1), "package tokenless is not installed", ""),
        );
        assert_eq!(q.query_installed("tokenless").unwrap(), None);
        assert!(!q.is_installed("tokenless").unwrap());
    }

    #[test]
    fn command_missing_maps_to_error() {
        let q = query_with_rpm("x", FakeOutcome::Err(io::ErrorKind::NotFound));
        let err = q.query_installed("x").unwrap_err();
        assert!(matches!(
            err,
            PackageQueryError::CommandMissing { command } if command == RPM
        ));
    }

    #[test]
    fn permission_denied_maps_to_error() {
        let q = query_with_rpm("x", FakeOutcome::Err(io::ErrorKind::PermissionDenied));
        let err = q.query_installed("x").unwrap_err();
        assert!(matches!(
            err,
            PackageQueryError::PermissionDenied { command } if command == RPM
        ));
    }

    #[test]
    fn query_failure_maps_to_error() {
        // stdout empty (no not-installed marker) + stderr error => hard failure.
        let q = query_with_rpm("x", ok_out(Some(1), "", "error: rpmdb open failed"));
        let err = q.query_installed("x").unwrap_err();
        match err {
            PackageQueryError::QueryFailed {
                command,
                code,
                stderr,
            } => {
                assert_eq!(command, RPM);
                assert_eq!(code, Some(1));
                assert!(stderr.contains("rpmdb"));
            }
            other => panic!("expected QueryFailed, got {other:?}"),
        }
    }

    #[test]
    fn unexpected_field_count_maps_to_error() {
        let q = query_with_rpm("tokenless", ok_out(Some(0), "tokenless|2.0.1", ""));
        let err = q.query_installed("tokenless").unwrap_err();
        assert!(matches!(err, PackageQueryError::UnexpectedOutput { .. }));
    }

    #[test]
    fn multiple_installed_is_unexpected() {
        let two = "tokenless|(none)|2.0.1|1.al8|x86_64\n\
                   tokenless|(none)|2.0.2|1.al8|x86_64\n";
        let q = query_with_rpm("tokenless", ok_out(Some(0), two, ""));
        let err = q.query_installed("tokenless").unwrap_err();
        match err {
            PackageQueryError::UnexpectedOutput { detail, .. } => {
                assert!(
                    detail.contains('2'),
                    "detail should mention version count: {detail}"
                );
            }
            other => panic!("expected UnexpectedOutput, got {other:?}"),
        }
    }

    #[test]
    fn epoch_none_normalizes() {
        let q = query_with_rpm("pkg", ok_out(Some(0), "pkg|(none)|2.3|4|x86_64", ""));
        let info = q.query_installed("pkg").unwrap().unwrap();
        assert_eq!(info.version.epoch, None);
        assert_eq!(info.version.to_string(), "2.3-4");
    }

    #[test]
    fn epoch_set_renders_evr() {
        let q = query_with_rpm("pkg", ok_out(Some(0), "pkg|1|2.3|4|x86_64", ""));
        let info = q.query_installed("pkg").unwrap().unwrap();
        assert_eq!(info.version.epoch.as_deref(), Some("1"));
        assert_eq!(info.version.to_string(), "1:2.3-4");
    }

    #[test]
    fn epoch_zero_normalizes_like_none() {
        // dnf repoquery emits "0" where rpm -q emits "(none)"; both must
        // normalize to None so the same package compares equal across
        // installed/available and is not mistaken for drift.
        let q = query_with_rpm("pkg", ok_out(Some(0), "pkg|0|2.3|4|x86_64", ""));
        let info = q.query_installed("pkg").unwrap().unwrap();
        assert_eq!(info.version.epoch, None);
        assert_eq!(info.version.to_string(), "2.3-4");
    }

    #[test]
    fn available_returns_candidates() {
        let out = "tokenless|(none)|2.0.1|1.al8|x86_64|anolisa\n\
                   tokenless|(none)|2.0.1|1.al8|aarch64|baseos\n";
        let q = query_with_dnf("tokenless", ok_out(Some(0), out, ""));
        let candidates = q.query_available("tokenless").unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].origin.as_deref(), Some("anolisa"));
        assert_eq!(candidates[0].arch, "x86_64");
        assert_eq!(candidates[1].origin.as_deref(), Some("baseos"));
        assert_eq!(candidates[1].arch, "aarch64");
        assert_eq!(candidates[0].version.to_string(), "2.0.1-1.al8");
    }

    #[test]
    fn available_epoch_zero_normalizes() {
        // dnf repoquery reports epoch "0" for no-epoch packages; it must
        // normalize to None, matching the rpm -q "(none)" representation.
        let q = query_with_dnf("pkg", ok_out(Some(0), "pkg|0|2.3|4|x86_64|anolisa\n", ""));
        let candidates = q.query_available("pkg").unwrap();
        let info = candidates.first().unwrap();
        assert_eq!(info.version.epoch, None);
        assert_eq!(info.version.to_string(), "2.3-4");
    }

    #[test]
    fn available_empty_returns_empty_vec() {
        let q = query_with_dnf("nothing", ok_out(Some(0), "", ""));
        let candidates = q.query_available("nothing").unwrap();
        assert!(candidates.is_empty());
    }

    #[test]
    fn available_failure_maps_to_error() {
        let q = query_with_dnf("x", ok_out(Some(1), "", "error: repo not found"));
        let err = q.query_available("x").unwrap_err();
        assert!(matches!(
            err,
            PackageQueryError::QueryFailed { command, .. } if command == DNF
        ));
    }

    #[test]
    fn available_bad_fields_maps_to_error() {
        let q = query_with_dnf("pkg", ok_out(Some(0), "pkg|2.0.1\n", ""));
        let err = q.query_available("pkg").unwrap_err();
        assert!(matches!(err, PackageQueryError::UnexpectedOutput { .. }));
    }

    #[test]
    fn installed_origin_returns_reponame() {
        let q = query_with_dnf("tokenless", ok_out(Some(0), "@System\n", ""));
        assert_eq!(
            q.installed_origin("tokenless").unwrap().as_deref(),
            Some("@System")
        );
    }

    #[test]
    fn installed_origin_empty_is_none() {
        // A package with no reported reponame yields an empty repoquery result;
        // origin is supplementary, so absence is a non-fatal `None`.
        let q = query_with_dnf("tokenless", ok_out(Some(0), "", ""));
        assert_eq!(q.installed_origin("tokenless").unwrap(), None);
    }

    #[test]
    fn installed_origin_failure_maps_to_error() {
        let q = query_with_dnf("x", ok_out(Some(1), "", "error: rpmdb open failed"));
        let err = q.installed_origin("x").unwrap_err();
        assert!(matches!(
            err,
            PackageQueryError::QueryFailed { command, .. } if command == DNF
        ));
    }

    #[test]
    fn installed_origin_command_missing_maps_to_error() {
        let q = query_with_dnf("x", FakeOutcome::Err(io::ErrorKind::NotFound));
        let err = q.installed_origin("x").unwrap_err();
        assert!(matches!(
            err,
            PackageQueryError::CommandMissing { command } if command == DNF
        ));
    }

    #[test]
    fn what_provides_returns_single_name() {
        let q = query_with_rpm(
            "anolisa-component(tokenless)",
            ok_out(Some(0), "tokenless\n", ""),
        );
        let names = q
            .what_provides_installed("anolisa-component(tokenless)")
            .unwrap();
        assert_eq!(names, vec!["tokenless".to_string()]);
    }

    #[test]
    fn what_provides_dedups_by_name() {
        // One package can satisfy a capability through several Provides lines;
        // the same name must collapse to a single entry.
        let q = query_with_rpm(
            "anolisa-component(tokenless)",
            ok_out(Some(0), "tokenless\ntokenless\n", ""),
        );
        let names = q
            .what_provides_installed("anolisa-component(tokenless)")
            .unwrap();
        assert_eq!(names, vec!["tokenless".to_string()]);
    }

    #[test]
    fn what_provides_keeps_distinct_names() {
        // Two different packages providing the same capability is the ambiguous
        // case callers must detect; both names are preserved in order.
        let q = query_with_rpm(
            "anolisa-component(tokenless)",
            ok_out(Some(0), "tokenless\nvendor-tokenless\n", ""),
        );
        let names = q
            .what_provides_installed("anolisa-component(tokenless)")
            .unwrap();
        assert_eq!(
            names,
            vec!["tokenless".to_string(), "vendor-tokenless".to_string()]
        );
    }

    #[test]
    fn what_provides_not_provided_is_empty() {
        // rpm writes "no package provides <cap>" to stdout with a non-zero exit;
        // that is the normal "nothing matches" branch, not an error.
        let q = query_with_rpm(
            "anolisa-component(absent)",
            ok_out(
                Some(1),
                "no package provides anolisa-component(absent)\n",
                "",
            ),
        );
        let names = q
            .what_provides_installed("anolisa-component(absent)")
            .unwrap();
        assert!(names.is_empty());
    }

    #[test]
    fn what_provides_failure_maps_to_error() {
        // Non-zero exit without the not-provided marker is a hard failure.
        let q = query_with_rpm("x", ok_out(Some(1), "", "error: rpmdb open failed"));
        let err = q.what_provides_installed("x").unwrap_err();
        assert!(matches!(
            err,
            PackageQueryError::QueryFailed { command, .. } if command == RPM
        ));
    }

    #[test]
    fn list_files_returns_paths() {
        // Mixed manifest: a binary, a system unit, and a blank line. The blank
        // line is dropped; every real path is returned verbatim (the caller,
        // not this layer, decides which are units).
        let manifest = "/usr/local/bin/agentsight\n\
                        /usr/lib/systemd/system/agentsight.service\n\
                        \n";
        let q = query_with_rpm("agentsight", ok_out(Some(0), manifest, ""));
        let files = q.list_files("agentsight").unwrap();
        assert_eq!(
            files,
            vec![
                "/usr/local/bin/agentsight".to_string(),
                "/usr/lib/systemd/system/agentsight.service".to_string(),
            ]
        );
    }

    #[test]
    fn list_files_not_installed_maps_to_error() {
        // `rpm -ql` on an absent package exits non-zero with the notice on
        // stdout; for a tracked component that is drift, so it must surface as
        // an error (carrying the stdout detail), not an empty file list.
        let q = query_with_rpm(
            "ghost",
            ok_out(Some(1), "package ghost is not installed", ""),
        );
        let err = q.list_files("ghost").unwrap_err();
        match err {
            PackageQueryError::QueryFailed {
                command, stderr, ..
            } => {
                assert_eq!(command, RPM);
                assert!(
                    stderr.contains("not installed"),
                    "detail should carry the rpm notice: {stderr}"
                );
            }
            other => panic!("expected QueryFailed, got {other:?}"),
        }
    }

    #[test]
    fn list_files_command_missing_maps_to_error() {
        let q = query_with_rpm("x", FakeOutcome::Err(io::ErrorKind::NotFound));
        let err = q.list_files("x").unwrap_err();
        assert!(matches!(
            err,
            PackageQueryError::CommandMissing { command } if command == RPM
        ));
    }
}
