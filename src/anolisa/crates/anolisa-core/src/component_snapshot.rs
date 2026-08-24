//! Shared component observations used by lifecycle consumers.
//!
//! A snapshot records which probes were requested separately from the evidence
//! they produced, so unavailable observations cannot be confused with absence.

use std::collections::BTreeSet;
use std::path::PathBuf;

use thiserror::Error;

use crate::domain::{Installation, InstallationScope, NativePm, Observation};
use crate::state::ObjectKind;
use crate::state_migration::QuarantineReason;

/// A component fact that may not have been requested or observable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeEvidence<T, P> {
    /// The snapshot request did not include this probe.
    NotRequested,
    /// The probe was attempted, but its source could not provide a result.
    Unavailable {
        /// Source consulted by the failed probe.
        provenance: P,
        /// Developer-facing explanation of the failure.
        reason: String,
    },
    /// The source was consulted and confirmed that the fact does not exist.
    Absent {
        /// Source that established absence.
        provenance: P,
    },
    /// The source was consulted and returned a value.
    Present {
        /// Source that produced the value.
        provenance: P,
        /// Observed value.
        value: T,
    },
}

impl<T, P> ProbeEvidence<T, P> {
    fn is_not_requested(&self) -> bool {
        matches!(self, Self::NotRequested)
    }
}

/// Probe kinds supported by the initial component snapshot contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SnapshotProbe {
    /// Read the ANOLISA installation state.
    State,
    /// Verify the files owned by the active or quarantined component record.
    OwnedFiles,
    /// Query the native package authority. Valid only in system scope.
    NativePackage,
    /// Inspect the transaction journal directory.
    PendingJournal,
}

/// Explicit set of observations needed by a lifecycle consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentSnapshotRequest {
    component: String,
    scope: InstallationScope,
    probes: BTreeSet<SnapshotProbe>,
}

impl ComponentSnapshotRequest {
    /// Creates a request for a resolved component and installation scope.
    pub fn new(
        component: impl Into<String>,
        scope: InstallationScope,
        probes: impl IntoIterator<Item = SnapshotProbe>,
    ) -> Self {
        Self {
            component: component.into(),
            scope,
            probes: probes.into_iter().collect(),
        }
    }

    /// Returns the resolved component name.
    pub fn component(&self) -> &str {
        &self.component
    }

    /// Returns the installation scope being observed.
    pub fn scope(&self) -> InstallationScope {
        self.scope
    }

    /// Returns whether the request includes a probe.
    pub fn requests(&self, probe: SnapshotProbe) -> bool {
        self.probes.contains(&probe)
    }

    /// Returns the requested probes in deterministic order.
    pub fn probes(&self) -> impl Iterator<Item = SnapshotProbe> + '_ {
        self.probes.iter().copied()
    }
}

/// Source of an ANOLISA installation-state observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateProvenance {
    /// State file that was consulted.
    pub path: PathBuf,
}

/// Source of a native package observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePackageProvenance {
    /// Native authority that was queried.
    pub manager: NativePm,
    /// Package name used for the query.
    pub package: String,
}

/// Source of an owned-file integrity observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedFilesProvenance {
    /// State file whose owned-file contract selected the paths to verify.
    pub state_path: PathBuf,
}

/// Source of a pending-journal observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalProvenance {
    /// Transaction journal directory that was inspected.
    pub directory: PathBuf,
}

/// Installation-state values returned by the state probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateSnapshot {
    /// A valid installation is active in state.
    Active(Box<Installation>),
    /// A preserved legacy record cannot safely participate in lifecycle actions.
    Quarantined(QuarantineReason),
}

/// Native package values returned by the native authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativePackageSnapshot {
    /// Exactly one installed package version was observed.
    Installed(Observation),
    /// More than one installed version makes the package observation ambiguous.
    MultipleVersions,
}

/// Integrity of the files declared by an owned component record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedFilesSnapshot {
    /// Every verifiable owned path satisfies its recorded contract.
    Verified,
    /// At least one owned path has a decisive integrity failure.
    Drifted,
}

/// Pending transaction selected for the requested component.
///
/// A collector may inspect a multi-entry journal inventory, but each snapshot
/// carries the single entry that determines this component's lifecycle decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingJournalSnapshot {
    /// Path to the selected pending journal file.
    pub path: PathBuf,
}

/// Consistent component observations for one explicit request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentSnapshot {
    request: ComponentSnapshotRequest,
    state: ProbeEvidence<StateSnapshot, StateProvenance>,
    owned_files: ProbeEvidence<OwnedFilesSnapshot, OwnedFilesProvenance>,
    native_package: ProbeEvidence<NativePackageSnapshot, NativePackageProvenance>,
    pending_journal: ProbeEvidence<PendingJournalSnapshot, JournalProvenance>,
}

impl ComponentSnapshot {
    /// Builds a snapshot after checking evidence against the requested probes.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotContractError`] when a probe is unsupported in the
    /// requested scope, evidence disagrees with the request, or active state
    /// belongs to a different component identity.
    pub fn from_parts(
        request: ComponentSnapshotRequest,
        state: ProbeEvidence<StateSnapshot, StateProvenance>,
        native_package: ProbeEvidence<NativePackageSnapshot, NativePackageProvenance>,
        pending_journal: ProbeEvidence<PendingJournalSnapshot, JournalProvenance>,
    ) -> Result<Self, SnapshotContractError> {
        Self::from_parts_with_owned_files(
            request,
            state,
            ProbeEvidence::NotRequested,
            native_package,
            pending_journal,
        )
    }

    /// Builds a snapshot that may include owned-file integrity evidence.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotContractError`] under the same conditions as
    /// [`Self::from_parts`], including mismatched owned-file probe evidence.
    pub fn from_parts_with_owned_files(
        request: ComponentSnapshotRequest,
        state: ProbeEvidence<StateSnapshot, StateProvenance>,
        owned_files: ProbeEvidence<OwnedFilesSnapshot, OwnedFilesProvenance>,
        native_package: ProbeEvidence<NativePackageSnapshot, NativePackageProvenance>,
        pending_journal: ProbeEvidence<PendingJournalSnapshot, JournalProvenance>,
    ) -> Result<Self, SnapshotContractError> {
        if matches!(request.scope, InstallationScope::User { .. })
            && request.requests(SnapshotProbe::NativePackage)
        {
            return Err(SnapshotContractError::UnsupportedProbeScope {
                probe: SnapshotProbe::NativePackage,
                scope: request.scope,
            });
        }
        validate_evidence(&request, SnapshotProbe::State, state.is_not_requested())?;
        validate_state_target(&request, &state)?;
        validate_evidence(
            &request,
            SnapshotProbe::OwnedFiles,
            owned_files.is_not_requested(),
        )?;
        validate_evidence(
            &request,
            SnapshotProbe::NativePackage,
            native_package.is_not_requested(),
        )?;
        validate_evidence(
            &request,
            SnapshotProbe::PendingJournal,
            pending_journal.is_not_requested(),
        )?;

        Ok(Self {
            request,
            state,
            owned_files,
            native_package,
            pending_journal,
        })
    }

    /// Returns the request that defines this snapshot.
    pub fn request(&self) -> &ComponentSnapshotRequest {
        &self.request
    }

    /// Returns the installation-state evidence.
    pub fn state(&self) -> &ProbeEvidence<StateSnapshot, StateProvenance> {
        &self.state
    }

    /// Returns the owned-file integrity evidence.
    pub fn owned_files(&self) -> &ProbeEvidence<OwnedFilesSnapshot, OwnedFilesProvenance> {
        &self.owned_files
    }

    /// Returns the native package evidence.
    pub fn native_package(&self) -> &ProbeEvidence<NativePackageSnapshot, NativePackageProvenance> {
        &self.native_package
    }

    /// Returns the pending-journal evidence.
    pub fn pending_journal(&self) -> &ProbeEvidence<PendingJournalSnapshot, JournalProvenance> {
        &self.pending_journal
    }
}

/// Contract violation between a snapshot request and its evidence.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum SnapshotContractError {
    /// A requested probe was represented as not requested.
    #[error("requested probe {probe:?} has no evidence")]
    MissingRequestedEvidence {
        /// Probe whose evidence is missing.
        probe: SnapshotProbe,
    },
    /// An unrequested probe unexpectedly contains evidence.
    #[error("unrequested probe {probe:?} contains evidence")]
    UnexpectedUnrequestedEvidence {
        /// Probe that unexpectedly contains evidence.
        probe: SnapshotProbe,
    },
    /// A probe cannot produce facts for the requested installation scope.
    #[error("probe {probe:?} is not supported for scope {scope:?}")]
    UnsupportedProbeScope {
        /// Probe that is unavailable in the requested scope.
        probe: SnapshotProbe,
        /// Installation scope that cannot use the probe.
        scope: InstallationScope,
    },
    /// Active state belongs to a different component identity.
    #[error(
        "active state target mismatch: expected Component {expected_component:?} in \
         {expected_scope:?}, got {actual_kind:?} {actual_name:?} in {actual_scope:?}"
    )]
    ActiveStateTargetMismatch {
        /// Component name requested by the snapshot.
        expected_component: String,
        /// Installation scope requested by the snapshot.
        expected_scope: InstallationScope,
        /// Object kind carried by the active state.
        actual_kind: ObjectKind,
        /// Object name carried by the active state.
        actual_name: String,
        /// Installation scope carried by the active state.
        actual_scope: InstallationScope,
    },
}

fn validate_evidence(
    request: &ComponentSnapshotRequest,
    probe: SnapshotProbe,
    not_requested: bool,
) -> Result<(), SnapshotContractError> {
    match (request.requests(probe), not_requested) {
        (true, true) => Err(SnapshotContractError::MissingRequestedEvidence { probe }),
        (false, false) => Err(SnapshotContractError::UnexpectedUnrequestedEvidence { probe }),
        _ => Ok(()),
    }
}

fn validate_state_target(
    request: &ComponentSnapshotRequest,
    state: &ProbeEvidence<StateSnapshot, StateProvenance>,
) -> Result<(), SnapshotContractError> {
    let ProbeEvidence::Present {
        value: StateSnapshot::Active(installation),
        ..
    } = state
    else {
        return Ok(());
    };

    if installation.kind == ObjectKind::Component
        && installation.name == request.component
        && installation.scope == request.scope
    {
        return Ok(());
    }

    Err(SnapshotContractError::ActiveStateTargetMismatch {
        expected_component: request.component.clone(),
        expected_scope: request.scope,
        actual_kind: installation.kind,
        actual_name: installation.name.clone(),
        actual_scope: installation.scope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{LifecycleStatus, ManagementRelation, PackageIdentity, ProviderBinding};
    use crate::state::SubscriptionScope;
    use crate::state_migration::QuarantineReason;

    fn active_installation() -> Installation {
        Installation {
            kind: ObjectKind::Component,
            name: "tokenless".to_string(),
            scope: InstallationScope::System,
            binding: ProviderBinding::Delegated {
                pm: NativePm::Rpm,
                package: PackageIdentity::Resolved {
                    name: "tokenless".to_string(),
                },
                relation: ManagementRelation::Observed,
                last_observed: None,
            },
            status: LifecycleStatus::Installed,
            installed_at: "2026-08-19T00:00:00Z".to_string(),
            last_operation_id: None,
            subscription_scope: SubscriptionScope::None,
            enabled_features: Vec::new(),
            health: Vec::new(),
        }
    }

    fn state_source() -> StateProvenance {
        StateProvenance {
            path: PathBuf::from("/var/lib/anolisa/state.json"),
        }
    }

    fn native_source() -> NativePackageProvenance {
        NativePackageProvenance {
            manager: NativePm::Rpm,
            package: "tokenless".to_string(),
        }
    }

    fn journal_source() -> JournalProvenance {
        JournalProvenance {
            directory: PathBuf::from("/var/lib/anolisa/transactions"),
        }
    }

    fn all_probes_request() -> ComponentSnapshotRequest {
        ComponentSnapshotRequest::new(
            "tokenless",
            InstallationScope::System,
            [
                SnapshotProbe::State,
                SnapshotProbe::NativePackage,
                SnapshotProbe::PendingJournal,
            ],
        )
    }

    #[test]
    fn request_deduplicates_probes_in_deterministic_order() {
        let request = ComponentSnapshotRequest::new(
            "tokenless",
            InstallationScope::System,
            [
                SnapshotProbe::PendingJournal,
                SnapshotProbe::State,
                SnapshotProbe::State,
            ],
        );

        assert_eq!(request.component(), "tokenless");
        assert_eq!(request.scope(), InstallationScope::System);
        assert_eq!(
            request.probes().collect::<Vec<_>>(),
            vec![SnapshotProbe::State, SnapshotProbe::PendingJournal]
        );
    }

    #[test]
    fn snapshot_preserves_typed_values_and_provenance() {
        let installation = active_installation();
        let observation = Observation {
            version: "0.7.7".to_string(),
            evr: Some("0:0.7.7-1".to_string()),
            arch: Some("x86_64".to_string()),
            source_repo: Some("anolisa".to_string()),
            observed_at: "2026-08-19T00:00:00Z".to_string(),
        };
        let snapshot = ComponentSnapshot::from_parts(
            all_probes_request(),
            ProbeEvidence::Present {
                provenance: state_source(),
                value: StateSnapshot::Active(Box::new(installation.clone())),
            },
            ProbeEvidence::Present {
                provenance: native_source(),
                value: NativePackageSnapshot::Installed(observation.clone()),
            },
            ProbeEvidence::Present {
                provenance: journal_source(),
                value: PendingJournalSnapshot {
                    path: PathBuf::from("/var/lib/anolisa/transactions/pending.json"),
                },
            },
        )
        .unwrap();

        assert_eq!(
            snapshot.state(),
            &ProbeEvidence::Present {
                provenance: state_source(),
                value: StateSnapshot::Active(Box::new(installation)),
            }
        );
        assert_eq!(
            snapshot.native_package(),
            &ProbeEvidence::Present {
                provenance: native_source(),
                value: NativePackageSnapshot::Installed(observation),
            }
        );
        assert_eq!(
            snapshot.pending_journal(),
            &ProbeEvidence::Present {
                provenance: journal_source(),
                value: PendingJournalSnapshot {
                    path: PathBuf::from("/var/lib/anolisa/transactions/pending.json"),
                },
            }
        );
    }

    #[test]
    fn snapshot_distinguishes_unavailable_absent_and_not_requested() {
        let request = ComponentSnapshotRequest::new(
            "tokenless",
            InstallationScope::System,
            [SnapshotProbe::State, SnapshotProbe::NativePackage],
        );
        let snapshot = ComponentSnapshot::from_parts(
            request,
            ProbeEvidence::Unavailable {
                provenance: state_source(),
                reason: "permission denied".to_string(),
            },
            ProbeEvidence::Absent {
                provenance: native_source(),
            },
            ProbeEvidence::NotRequested,
        )
        .unwrap();

        assert!(matches!(
            snapshot.state(),
            ProbeEvidence::Unavailable { reason, .. } if reason == "permission denied"
        ));
        assert!(matches!(
            snapshot.native_package(),
            ProbeEvidence::Absent { .. }
        ));
        assert_eq!(snapshot.pending_journal(), &ProbeEvidence::NotRequested);
    }

    #[test]
    fn snapshot_rejects_missing_requested_evidence() {
        let request = ComponentSnapshotRequest::new(
            "tokenless",
            InstallationScope::System,
            [SnapshotProbe::State],
        );

        let error = ComponentSnapshot::from_parts(
            request,
            ProbeEvidence::NotRequested,
            ProbeEvidence::NotRequested,
            ProbeEvidence::NotRequested,
        )
        .unwrap_err();

        assert_eq!(
            error,
            SnapshotContractError::MissingRequestedEvidence {
                probe: SnapshotProbe::State,
            }
        );
    }

    #[test]
    fn snapshot_rejects_unrequested_evidence() {
        let request = ComponentSnapshotRequest::new(
            "tokenless",
            InstallationScope::System,
            [SnapshotProbe::State],
        );

        let error = ComponentSnapshot::from_parts(
            request,
            ProbeEvidence::Absent {
                provenance: state_source(),
            },
            ProbeEvidence::Absent {
                provenance: native_source(),
            },
            ProbeEvidence::NotRequested,
        )
        .unwrap_err();

        assert_eq!(
            error,
            SnapshotContractError::UnexpectedUnrequestedEvidence {
                probe: SnapshotProbe::NativePackage,
            }
        );
    }

    #[test]
    fn snapshot_rejects_active_state_for_a_different_target() {
        let mismatches = [
            (
                ObjectKind::Adapter,
                "tokenless".to_string(),
                InstallationScope::System,
            ),
            (
                ObjectKind::Component,
                "cosh".to_string(),
                InstallationScope::System,
            ),
            (
                ObjectKind::Component,
                "tokenless".to_string(),
                InstallationScope::User { uid: 1000 },
            ),
        ];

        for (kind, name, scope) in mismatches {
            let mut installation = active_installation();
            installation.kind = kind;
            installation.name.clone_from(&name);
            installation.scope = scope;

            let error = ComponentSnapshot::from_parts(
                ComponentSnapshotRequest::new(
                    "tokenless",
                    InstallationScope::System,
                    [SnapshotProbe::State],
                ),
                ProbeEvidence::Present {
                    provenance: state_source(),
                    value: StateSnapshot::Active(Box::new(installation)),
                },
                ProbeEvidence::NotRequested,
                ProbeEvidence::NotRequested,
            )
            .unwrap_err();

            assert_eq!(
                error,
                SnapshotContractError::ActiveStateTargetMismatch {
                    expected_component: "tokenless".to_string(),
                    expected_scope: InstallationScope::System,
                    actual_kind: kind,
                    actual_name: name,
                    actual_scope: scope,
                }
            );
        }
    }

    #[test]
    fn snapshot_rejects_native_package_probe_in_user_scope() {
        let error = ComponentSnapshot::from_parts(
            ComponentSnapshotRequest::new(
                "tokenless",
                InstallationScope::User { uid: 1000 },
                [SnapshotProbe::NativePackage],
            ),
            ProbeEvidence::NotRequested,
            ProbeEvidence::Absent {
                provenance: native_source(),
            },
            ProbeEvidence::NotRequested,
        )
        .unwrap_err();

        assert_eq!(
            error,
            SnapshotContractError::UnsupportedProbeScope {
                probe: SnapshotProbe::NativePackage,
                scope: InstallationScope::User { uid: 1000 },
            }
        );
    }

    #[test]
    fn state_and_native_variants_cover_ambiguous_observations() {
        let quarantined = StateSnapshot::Quarantined(QuarantineReason::NoEvidence);
        let multiple_versions = NativePackageSnapshot::MultipleVersions;

        assert!(matches!(
            quarantined,
            StateSnapshot::Quarantined(QuarantineReason::NoEvidence)
        ));
        assert_eq!(multiple_versions, NativePackageSnapshot::MultipleVersions);
    }
}
