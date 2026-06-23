//! Skill Security extension seam.
//!
//! Layout:
//!
//! * [`policy::SecurityPolicy`] / [`policy::PermissivePolicy`] /
//!   [`policy::SkillMetaProtectionPolicy`] / [`policy::PathPolicy`] /
//!   [`policy::PolicyDecision`] — describe and decide on operations.
//! * [`event::SkillEvent`] / [`event::SkillEventKind`] /
//!   [`event::SkillEventSink`] — normalized records of FUSE-observed
//!   operations. The default sink ([`event::NoopEventSink`]) drops every
//!   event; tests can opt into [`event::InMemoryEventSink`].
//! * [`path::is_skill_meta_path`] — pure-lexical classifier for
//!   `.skill-meta/**` reserved metadata paths.
//! * [`audit::JsonlFileAuditSink`] — best-effort JSONL audit stream
//!   (Package S2). Off by default; opt in via
//!   [`crate::SkillFs::with_event_sink`].
//! * [`mode::SecurityModeConfig`] — Package M0 startup-time validation
//!   that the source/mountpoint pair satisfies the security guarantee an
//!   operator asked for. Disabled by default, so the existing normal vs.
//!   in-place mount UX is unchanged.
//! * [`drift::SourceDriftObserver`] — visibility-only seam that turns
//!   out-of-band source-tree changes into normalized
//!   [`event::SkillEventKind::SourceChanged`] records.
//! * [`lifecycle::is_reserved_lifecycle_name`] — Package S3 reservation of
//!   `.staging`, `.certified`, `.quarantine`, and `.archive` as lifecycle
//!   namespace names. S3 keeps the names hidden from ordinary
//!   `readdir`/`lookup` and rejects mutations targeting them with a
//!   deterministic permission errno; lifecycle state transitions,
//!   quarantine/scanner integration, and trusted-writer identity are all
//!   out of scope until later packages.
//! * [`lifecycle::LifecycleViewMode`] /
//!   [`lifecycle::classify_skill_name_with_mode`] — Package S3.1
//!   management-view contract. Defines the pure-API boundary between the
//!   ordinary agent-facing view (where S3 reservation applies) and a
//!   future management view that intentionally exposes the reserved
//!   roots. S3.1 only ships the contract: no FUSE callback selects
//!   [`lifecycle::LifecycleViewMode::Management`] today, and no CLI flag
//!   turns it on. Default mount behavior is exactly S3.
//! * [`drift_runtime`] — Package W1 runtime adapter that turns
//!   [`skillfs_core::watcher::SkillEvent`] notifications into
//!   [`drift::DriftEvent`] records and emits them through an injected
//!   [`drift::SourceDriftObserver`]. Coverage is intentionally narrow:
//!   the producer in `skillfs-core::watcher::classify_event` only surfaces
//!   `<source>/<skill>/SKILL.md` create/modify/delete and immediate
//!   skill-directory create/delete, so W1 only observes that subset.
//!   Default behavior is still no-op; nothing wires the watcher into the
//!   FUSE runtime unless an operator explicitly turns audit logging on
//!   (see the CLI `--audit-log` flag).
//!
//! Package S0 added the seam. Package S1 plugs the first real policy,
//! [`policy::SkillMetaProtectionPolicy`], into `SkillFs` as the default so
//! `.skill-meta/**` is read-visible but mutation-protected by default.
//! Package S2 adds the audit sink; the default `SkillFs` sink is still
//! [`event::NoopEventSink`]. Package M0 layers the security-mode gate on
//! top so audit/policy guarantees can be enforced by refusing to start a
//! non-in-place mount when the operator opts in. Package W1 connects the
//! existing `skillfs-core::watcher` to the W0 drift observer so out-of-band
//! source changes can surface as `SourceChanged` audit records.

pub mod audit;
pub mod drift;
pub mod drift_runtime;
pub mod event;
pub mod lifecycle;
pub mod mode;
pub mod path;
pub mod policy;

pub use audit::{
    AuditConfig, AuditPathError, AuditRuntimeConfig, DEFAULT_AUDIT_QUEUE_CAPACITY,
    JsonlFileAuditSink, event_action_str, event_kind_str, event_to_json, serialize_event_jsonl,
};
pub use drift::{
    DriftChangeKind, DriftEvent, DriftScope, SourceDriftObserver, classify_drift_path,
};
pub use drift_runtime::{
    DriftWatcherHandle, core_event_to_drift_event, drive_drift_watcher, spawn_drift_watcher,
};
pub use event::{
    InMemoryEventSink, NoopEventSink, SkillEvent, SkillEventAction, SkillEventKind, SkillEventSink,
};
pub use lifecycle::{
    LIFECYCLE_ARCHIVE, LIFECYCLE_CERTIFIED, LIFECYCLE_QUARANTINE, LIFECYCLE_RESERVED_NAMES,
    LIFECYCLE_STAGING, LifecycleAccess, LifecycleNameClass, LifecycleViewMode,
    classify_skill_name as classify_lifecycle_skill_name,
    classify_skill_name_with_mode as classify_lifecycle_skill_name_with_mode,
    is_lifecycle_name_mutable, is_lifecycle_name_visible, is_reserved_lifecycle_name,
};
pub use mode::{SecurityModeConfig, SecurityModeError};
pub use path::{SKILL_META_DIR, is_skill_meta_path};
pub use policy::{
    PathPolicy, PermissivePolicy, PolicyDecision, SecurityPolicy, SkillMetaProtectionPolicy,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn permissive_policy_allows_every_kind() {
        let policy = PermissivePolicy;
        for kind in [
            SkillEventKind::Open,
            SkillEventKind::Read,
            SkillEventKind::Write,
            SkillEventKind::Create,
            SkillEventKind::Delete,
            SkillEventKind::Rename,
            SkillEventKind::Metadata,
            SkillEventKind::Readlink,
            SkillEventKind::SymlinkAttempt,
            SkillEventKind::HardlinkAttempt,
            SkillEventKind::PolicyDecision,
            SkillEventKind::PolicyDenied,
            SkillEventKind::SourceChanged,
        ] {
            let ctx = PathPolicy::new(kind)
                .with_skill_name(Some("alpha"))
                .with_relative_path(Some(Path::new("scripts/run.sh")));
            assert_eq!(policy.check_path(&ctx), PolicyDecision::Allow);
            assert!(policy.check_path(&ctx).is_allowed());
        }
    }

    #[test]
    fn policy_decision_constructors() {
        assert_eq!(PolicyDecision::allow(), PolicyDecision::Allow);
        let deny = PolicyDecision::deny(libc::EACCES, "test");
        assert!(!deny.is_allowed());
        match deny {
            PolicyDecision::Deny { errno, reason } => {
                assert_eq!(errno, libc::EACCES);
                assert_eq!(reason, "test");
            }
            _ => panic!("expected Deny"),
        }
    }

    #[test]
    fn noop_sink_does_not_panic_or_record() {
        let sink = NoopEventSink;
        let event = SkillEvent::new(SkillEventKind::Read)
            .with_skill_name("alpha")
            .with_bytes(64);
        // Repeated emit must not fail.
        for _ in 0..16 {
            sink.emit(&event);
        }
    }

    #[test]
    fn in_memory_sink_records_events() {
        let sink = InMemoryEventSink::new();
        assert!(sink.is_empty());
        sink.emit(
            &SkillEvent::new(SkillEventKind::Readlink)
                .with_skill_name("alpha")
                .with_relative_path("link")
                .with_action(SkillEventAction::Allowed),
        );
        sink.emit(
            &SkillEvent::new(SkillEventKind::SymlinkAttempt)
                .with_skill_name("alpha")
                .with_action(SkillEventAction::Rejected)
                .with_errno(libc::EROFS),
        );

        assert_eq!(sink.len(), 2);
        let recorded = sink.events();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].kind, SkillEventKind::Readlink);
        assert_eq!(recorded[1].kind, SkillEventKind::SymlinkAttempt);
        assert_eq!(recorded[1].errno, Some(libc::EROFS));
        assert_eq!(recorded[1].action, Some(SkillEventAction::Rejected));

        let symlink_attempts = sink.of_kind(SkillEventKind::SymlinkAttempt);
        assert_eq!(symlink_attempts.len(), 1);
        assert_eq!(symlink_attempts[0].skill_name.as_deref(), Some("alpha"));
    }

    #[test]
    fn event_normalization_preserves_skill_name_and_path() {
        let event = SkillEvent::new(SkillEventKind::Delete)
            .with_optional_skill_name(Some("alpha"))
            .with_optional_relative_path(Some(Path::new("scripts/run.sh")))
            .with_caller(1000, 1000)
            .with_errno(libc::ENOENT);

        assert_eq!(event.skill_name.as_deref(), Some("alpha"));
        assert_eq!(
            event.relative_path.as_deref(),
            Some(Path::new("scripts/run.sh"))
        );
        assert_eq!(event.uid, Some(1000));
        assert_eq!(event.gid, Some(1000));
        assert_eq!(event.errno, Some(libc::ENOENT));
        assert_eq!(event.bytes, None);
    }

    #[test]
    fn event_optional_setters_can_clear_or_skip() {
        let event = SkillEvent::new(SkillEventKind::Metadata)
            .with_optional_skill_name::<String>(None)
            .with_optional_relative_path::<&Path>(None);
        assert!(event.skill_name.is_none());
        assert!(event.relative_path.is_none());
    }
}
