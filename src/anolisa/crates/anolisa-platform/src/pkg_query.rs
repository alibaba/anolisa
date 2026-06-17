//! Backend-neutral package query contract and shared types.
//!
//! [`PackageQuery`] is object-safe and backend-agnostic: RPM and (future) apt
//! backends both fit it. This module only declares the contract and output
//! types; concrete backends live in sibling modules (e.g. [`crate::rpm_query`]).
//!
//! "Not installed" is a normal branch ([`Option::None`]), not an error: the
//! observe/repair/update consumers treat absence as expected control flow, so
//! [`PackageQueryError`] is reserved for genuinely anomalous conditions.

use std::fmt;

use thiserror::Error;

/// Version triple isomorphic to both RPM EVR and dpkg version.
///
/// Both backends share the `[epoch:]version[-release]` shape, so one neutral
/// type plus [`fmt::Display`] carries either; `release` maps to dpkg's
/// `debian_revision` and is `None` for native packages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageVersion {
    /// Epoch; `None` for the equivalent "no epoch" spellings (`(none)`, empty, or `0`).
    pub epoch: Option<String>,
    /// Upstream version.
    pub version: String,
    /// Release / debian_revision; `None` for native packages with no release.
    pub release: Option<String>,
}

impl fmt::Display for PackageVersion {
    /// Renders `[epoch:]version[-release]` — the EVR form for RPM and the full
    /// version string for dpkg.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(epoch) = &self.epoch {
            write!(f, "{epoch}:")?;
        }
        write!(f, "{}", self.version)?;
        if let Some(release) = &self.release {
            write!(f, "-{release}")?;
        }
        Ok(())
    }
}

/// A package's identity, version, and origin (shared by installed/available queries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageInfo {
    /// Package name as reported by the backend (e.g. rpm `%{NAME}`).
    pub name: String,
    /// Resolved version triple.
    pub version: PackageVersion,
    /// Architecture (e.g. `x86_64`, `noarch`).
    pub arch: String,
    /// Source repo/origin; installed queries typically yield `None` (or a
    /// backend-specific marker like `@System`).
    pub origin: Option<String>,
}

/// Errors raised by [`PackageQuery`] backends.
#[derive(Debug, Error)]
pub enum PackageQueryError {
    /// The backend binary could not be found (spawn `NotFound`).
    #[error("command not found: {command}")]
    CommandMissing {
        /// Backend binary that could not be found.
        command: String,
    },
    /// The backend binary existed but could not be executed (`PermissionDenied`).
    #[error("permission denied running {command}")]
    PermissionDenied {
        /// Backend binary that could not be executed.
        command: String,
    },
    /// The command ran but reported a hard failure (non-zero exit that is not
    /// the backend's "not installed" signal).
    #[error("{command} failed (code {code:?}): {stderr}")]
    QueryFailed {
        /// Backend binary that exited with a failure.
        command: String,
        /// Exit code; `None` if the process was killed by a signal.
        code: Option<i32>,
        /// Captured standard error from the failed command.
        stderr: String,
    },
    /// Output could not be parsed (wrong field count), or the single-instance
    /// invariant was violated ([`PackageQuery::query_installed`] got multiple
    /// rows = same-name package with several installed versions). `detail`
    /// describes the shape so callers can decide how to handle the drift.
    #[error("unexpected {command} output: {detail}")]
    UnexpectedOutput {
        /// Backend binary whose output was unexpected.
        command: String,
        /// Description of the malformed or invariant-violating output shape.
        detail: String,
    },
}

/// Backend-neutral package query contract.
///
/// All methods take `&self` and return concrete types, so the trait is
/// object-safe and any backend can be held as `Box<dyn PackageQuery>`.
pub trait PackageQuery {
    /// Query an installed package; not installed returns `Ok(None)`.
    ///
    /// # Errors
    /// See [`PackageQueryError`] for the failure conditions; absence of the
    /// package is **not** an error.
    fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError>;

    /// Whether the package is installed.
    ///
    /// Default implementation delegates to [`query_installed`](Self::query_installed)
    /// so backends need not repeat it.
    fn is_installed(&self, package: &str) -> Result<bool, PackageQueryError> {
        Ok(self.query_installed(package)?.is_some())
    }

    /// Query available candidates in repos; no candidates yields an empty `Vec`.
    ///
    /// # Errors
    /// See [`PackageQueryError`].
    fn query_available(&self, package: &str) -> Result<Vec<PackageInfo>, PackageQueryError>;

    /// Source repo of an *installed* package (e.g. `@System`, `anolisa-release`);
    /// `None` when it cannot be determined.
    ///
    /// [`query_installed`](Self::query_installed) cannot report this (its
    /// `rpm -q` path yields no reponame, hence [`PackageInfo::origin`] is always
    /// `None` there), so adopt/observe callers query the origin separately to
    /// populate `source_repo`. The default returns `None` so backends without
    /// origin support still satisfy the trait.
    ///
    /// # Errors
    /// See [`PackageQueryError`]; "no origin" is `Ok(None)`, not an error.
    fn installed_origin(&self, _package: &str) -> Result<Option<String>, PackageQueryError> {
        Ok(None)
    }

    /// Names of *installed* packages that provide `capability` (an RPM virtual
    /// provide such as `anolisa-component(<name>)`), de-duplicated by name.
    ///
    /// No provider yields an empty `Vec` (a normal branch, not an error). The
    /// default returns empty so backends without provides lookup still satisfy
    /// the trait. Callers treat ≥2 distinct names as an ambiguous match.
    ///
    /// # Errors
    /// See [`PackageQueryError`]; "nothing provides it" is `Ok(vec![])`.
    fn what_provides_installed(&self, _capability: &str) -> Result<Vec<String>, PackageQueryError> {
        Ok(Vec::new())
    }
}
