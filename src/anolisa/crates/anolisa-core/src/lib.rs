//! Core planning, manifest, state, and lifecycle primitives for ANOLISA.
//!
//! The crate is deliberately CLI-agnostic: callers provide catalogs,
//! distribution indexes, environment facts, and filesystem layout, then use
//! these APIs to plan, execute, audit, and roll back lifecycle operations.

pub mod adapter;
pub mod backup;
pub mod catalog;
pub mod central_log;
pub mod component;
pub mod dependency;
pub mod distribution;
pub mod download;
pub mod feature_flags;
pub mod health;
pub mod hooks;
pub mod install_runner;
pub mod integrity;
pub mod lifecycle;
pub mod lock;
pub mod manifest;
pub mod path_safety;
pub mod process;
pub mod register;
pub mod registry;
pub mod sandbox_install;
pub mod self_update;
pub mod service;
pub mod state;
pub mod transaction;
pub mod upload;

pub use adapter::{AdapterError, DetectResult, detect_framework, expand_layout_placeholders};
pub use backup::{BackupEntry, BackupSet};
pub use catalog::{Catalog, CatalogError, CatalogLayers};
pub use central_log::{
    CentralLog, CentralLogError, LogFilter, LogKind, LogRecord, LogStatus, Severity,
};
pub use component::{Component, ComponentMeta, ComponentStatus};
pub use distribution::{
    ArtifactType, DistributionEntry, DistributionError, DistributionIndex, ResolveError,
    ResolveQuery,
};
pub use download::{DownloadCache, DownloadError, DownloadedArtifact};
pub use feature_flags::FeatureStore;
pub use health::{CheckEnv, CheckOutcome, CheckSpec, CheckStatus, Protocol, run_check};
pub use hooks::{
    HookOutcome, HookPhase, HookRunResult, HookSkipReason, HookSpec, discover_component_phase_hook,
    run_hook, run_hooks, run_phase_hooks,
};
pub use install_runner::{
    InstallError, InstallOutcome, InstallRunner, InstalledFile, ResolvedInstallFile,
};
pub use integrity::{IntegrityStatus, check_owned_file};
pub use lifecycle::{
    ComponentLifecyclePlan, FileAction, FileActionKind, FileOwner as LifecycleFileOwner,
    HookAction, LifecycleError, LifecycleMode, LifecycleOperation, LifecycleOutcome,
    LifecyclePhase, LifecyclePlan, LifecycleTargetKind, RiskLevel, ServiceAction,
    ServiceActionKind, execute_plan,
};
pub use lock::{InstallLock, LockError};
pub use manifest::{AdapterSpec, ComponentManifest, DistributionSelector, FileKind, HealthSpec};
pub use register::{
    ConsentState, ProductType, RegisterRecord, RegisterSource, RegisterState, RegistrationManager,
    SubscriptionError, current_operator, require_root,
};
pub use registry::{
    FetchFailure, FetchedMeta, HttpFetch, IndexFreshness, Registry, RegistryClient, RegistryConfig,
    RegistryError, UreqFetch,
};
pub use self_update::{
    ReleaseArtifact, ReleaseManifest, SelfUpdateError, SelfUpdateOutcome, check_and_update,
    check_update, update_url,
};
pub use service::{
    FakeServiceManager, NotSupportedServiceManager, ServiceError, ServiceManager, ServiceOp,
    ServiceOutcome, ServiceState, SystemdServiceManager,
    for_install_mode as service_for_install_mode,
};
pub use state::{
    BackupRecord, ExternalModifiedFile, FileOwner, HealthEntry, InstallMode, InstalledObject,
    InstalledState, ObjectKind, ObjectStatus, OperationRecord, OwnedFile, STATE_SCHEMA_VERSION,
    ServiceRef, StateError, SubscriptionScope,
};
pub use transaction::{
    JOURNAL_SCHEMA_VERSION, RollbackAction, RollbackActionKind, Transaction, TransactionError,
    TransactionOutcome, TransactionOutcomeStatus, TransactionStep, TransactionStepStatus,
};
pub use upload::{UploadConfig, UploadError, UploadStarter, validate_sls_account_id};
