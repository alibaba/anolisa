// SPDX-License-Identifier: Apache-2.0
//! Template registry.
//!
//! v0.1: pure in-memory. No `/proc/anolisa/mm/template/` traffic — that
//! lands in Phase 3 of the roadmap.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AnvilError, Result};

/// Two kinds of shared kernel objects we track in v0.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateKind {
    MmTemplate,
    SharedBaseImage,
}

#[derive(Debug)]
pub struct TemplateEntry {
    pub id: Uuid,
    pub kind: TemplateKind,
    pub image_digest: String,
    pub kernel_version: String,
    pub refcnt: AtomicU32,
    pub created_at: DateTime<Utc>,
    pub last_used: DateTime<Utc>,
    pub invalidated: bool,
    /// Tracks which instances currently hold a reference, for idempotent
    /// register/unregister and to surface leaks during inspection.
    pub holders: HashSet<Uuid>,
}

impl Clone for TemplateEntry {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            kind: self.kind,
            image_digest: self.image_digest.clone(),
            kernel_version: self.kernel_version.clone(),
            refcnt: AtomicU32::new(self.refcnt.load(Ordering::SeqCst)),
            created_at: self.created_at,
            last_used: self.last_used,
            invalidated: self.invalidated,
            holders: self.holders.clone(),
        }
    }
}

/// Serializable view of a template entry — used by the daemon's UDS
/// `GET /v1/templates` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateView {
    pub id: Uuid,
    pub kind: TemplateKind,
    pub image_digest: String,
    pub kernel_version: String,
    pub refcnt: u32,
    pub created_at: DateTime<Utc>,
    pub last_used: DateTime<Utc>,
    pub invalidated: bool,
}

impl From<&TemplateEntry> for TemplateView {
    fn from(e: &TemplateEntry) -> Self {
        Self {
            id: e.id,
            kind: e.kind,
            image_digest: e.image_digest.clone(),
            kernel_version: e.kernel_version.clone(),
            refcnt: e.refcnt.load(Ordering::SeqCst),
            created_at: e.created_at,
            last_used: e.last_used,
            invalidated: e.invalidated,
        }
    }
}

#[derive(Debug, Default)]
pub struct TemplateRegistry {
    entries: HashMap<Uuid, TemplateEntry>,
}

impl TemplateRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_template(
        &mut self,
        kind: TemplateKind,
        image_digest: String,
        kernel_version: String,
    ) -> Uuid {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let entry = TemplateEntry {
            id,
            kind,
            image_digest,
            kernel_version,
            refcnt: AtomicU32::new(0),
            created_at: now,
            last_used: now,
            invalidated: false,
            holders: HashSet::new(),
        };
        self.entries.insert(id, entry);
        tracing::info!(template = %id, ?kind, "created template");
        id
    }

    pub fn register(&mut self, template_id: Uuid, instance_id: Uuid) -> Result<()> {
        let entry =
            self.entries
                .get_mut(&template_id)
                .ok_or_else(|| AnvilError::TemplateError {
                    msg: format!("template {template_id} not found"),
                })?;
        if entry.invalidated {
            return Err(AnvilError::TemplateError {
                msg: format!("template {template_id} is invalidated"),
            });
        }
        if entry.holders.insert(instance_id) {
            entry.refcnt.fetch_add(1, Ordering::SeqCst);
        }
        entry.last_used = Utc::now();
        tracing::info!(template = %template_id, instance = %instance_id, "registered template");
        Ok(())
    }

    pub fn unregister(&mut self, template_id: Uuid, instance_id: Uuid) -> Result<()> {
        let entry =
            self.entries
                .get_mut(&template_id)
                .ok_or_else(|| AnvilError::TemplateError {
                    msg: format!("template {template_id} not found"),
                })?;
        if entry.holders.remove(&instance_id) {
            entry.refcnt.fetch_sub(1, Ordering::SeqCst);
        }
        entry.last_used = Utc::now();
        tracing::info!(template = %template_id, instance = %instance_id, "unregistered template");
        Ok(())
    }

    /// Mark a template as unusable. It will be reaped by the next
    /// [`Self::gc_unused`] once `refcnt` drops to zero.
    pub fn invalidate(&mut self, template_id: Uuid) -> Result<()> {
        let entry =
            self.entries
                .get_mut(&template_id)
                .ok_or_else(|| AnvilError::TemplateError {
                    msg: format!("template {template_id} not found"),
                })?;
        entry.invalidated = true;
        tracing::warn!(template = %template_id, "invalidated template");
        Ok(())
    }

    /// Reap templates whose refcnt is 0 *and* whose `last_used` is older
    /// than `idle_ttl` (or that are invalidated, regardless of TTL).
    /// Returns the ids that were removed.
    pub fn gc_unused(&mut self, idle_ttl: Duration) -> Vec<Uuid> {
        // TODO(Phase 3): gc_unused() currently only removes templates from the in-memory registry.
        // Once Kernel Coordinator lands, this must invoke kernel.deactivate() (or equivalent)
        // to release physical pages held by mm-template; otherwise kernel-side frames leak.
        let now = Utc::now();
        let ttl = chrono::Duration::from_std(idle_ttl).unwrap_or(chrono::Duration::zero());
        let stale: Vec<Uuid> = self
            .entries
            .iter()
            .filter(|(_, e)| {
                let zero_refs = e.refcnt.load(Ordering::SeqCst) == 0;
                if !zero_refs {
                    return false;
                }
                if e.invalidated {
                    return true;
                }
                now.signed_duration_since(e.last_used) >= ttl
            })
            .map(|(id, _)| *id)
            .collect();
        for id in &stale {
            self.entries.remove(id);
            tracing::info!(template = %id, "gc'd template");
        }
        stale
    }

    pub fn list(&self) -> Vec<TemplateView> {
        self.entries.values().map(TemplateView::from).collect()
    }

    pub fn inspect(&self, id: Uuid) -> Option<TemplateView> {
        self.entries.get(&id).map(TemplateView::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refcnt_round_trip() {
        let mut reg = TemplateRegistry::new();
        let tid = reg.create_template(
            TemplateKind::MmTemplate,
            "sha256:img".into(),
            "6.6.0".into(),
        );
        let i1 = Uuid::new_v4();
        let i2 = Uuid::new_v4();
        reg.register(tid, i1).expect("ok");
        reg.register(tid, i2).expect("ok");
        // duplicate register is idempotent
        reg.register(tid, i1).expect("ok");
        let view = reg.inspect(tid).expect("exists");
        assert_eq!(view.refcnt, 2);

        reg.unregister(tid, i1).expect("ok");
        reg.unregister(tid, i2).expect("ok");
        assert_eq!(reg.inspect(tid).expect("exists").refcnt, 0);
    }

    #[test]
    fn invalidate_blocks_register() {
        let mut reg = TemplateRegistry::new();
        let tid = reg.create_template(
            TemplateKind::SharedBaseImage,
            "sha256:img".into(),
            "6.6.0".into(),
        );
        reg.invalidate(tid).expect("ok");
        let err = reg.register(tid, Uuid::new_v4()).expect_err("blocked");
        assert!(matches!(err, AnvilError::TemplateError { .. }));
    }

    #[test]
    fn gc_collects_invalidated_with_zero_ref_immediately() {
        let mut reg = TemplateRegistry::new();
        let tid = reg.create_template(
            TemplateKind::MmTemplate,
            "sha256:img".into(),
            "6.6.0".into(),
        );
        reg.invalidate(tid).expect("ok");
        let collected = reg.gc_unused(Duration::from_secs(3600));
        assert_eq!(collected, vec![tid]);
        assert!(reg.inspect(tid).is_none());
    }

    #[test]
    fn gc_skips_in_use() {
        let mut reg = TemplateRegistry::new();
        let tid = reg.create_template(
            TemplateKind::MmTemplate,
            "sha256:img".into(),
            "6.6.0".into(),
        );
        reg.register(tid, Uuid::new_v4()).expect("ok");
        let collected = reg.gc_unused(Duration::from_secs(0));
        assert!(collected.is_empty());
    }

    #[test]
    fn list_returns_all() {
        let mut reg = TemplateRegistry::new();
        reg.create_template(TemplateKind::MmTemplate, "sha256:a".into(), "6.6".into());
        reg.create_template(
            TemplateKind::SharedBaseImage,
            "sha256:b".into(),
            "6.6".into(),
        );
        assert_eq!(reg.list().len(), 2);
    }
}
