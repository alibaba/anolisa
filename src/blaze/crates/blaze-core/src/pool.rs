// SPDX-License-Identifier: Apache-2.0
//! Warm pool key/config/manager.
//!
//! v0.1: in-memory only. The daemon owns pool persistence indirectly
//! via [`crate::lifecycle::SandboxInstance::persist`] for each warm
//! instance — pool itself is rebuilt by scanning the state dir on
//! restart.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backend::BackendKind;
use crate::policy::{ResetMode, WorkloadClass};

/// One warm pool exists per `(backend, workload_class, image_digest)`
/// triple. Cross-backend mixing is forbidden.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PoolKey {
    pub backend: BackendKind,
    pub workload_class: WorkloadClass,
    pub image_digest: String,
}

impl PoolKey {
    pub fn new(backend: BackendKind, workload_class: WorkloadClass, image_digest: String) -> Self {
        Self {
            backend,
            workload_class,
            image_digest,
        }
    }
}

/// Per-pool sizing & reset config; usually derived from
/// [`crate::policy::PolicyPool`] at evaluate time and pinned for the
/// pool's lifetime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    pub enabled: bool,
    pub min: u32,
    pub target: u32,
    pub max: u32,
    /// Serialized as humantime-style string; manager treats it opaquely.
    #[serde(with = "duration_secs")]
    pub warm_ttl: Duration,
    pub reset_mode: ResetMode,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min: 0,
            target: 0,
            max: 0,
            warm_ttl: Duration::from_secs(30 * 60),
            reset_mode: ResetMode::default(),
        }
    }
}

mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_u64(d.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(de)?;
        Ok(Duration::from_secs(secs))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PoolStats {
    pub warm_count: u32,
    pub total_hits: u64,
    pub total_misses: u64,
    pub evict_count: u64,
}

#[derive(Debug, Default)]
struct PoolBucket {
    config: PoolConfig,
    /// FIFO of warm instances ready for reuse.
    warm: VecDeque<Uuid>,
    stats: PoolStats,
}

/// Coordinator for all warm pools on a single host.
#[derive(Debug, Default)]
pub struct PoolManager {
    pools: HashMap<PoolKey, PoolBucket>,
}

impl PoolManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Try to reuse a warm instance. Returns the instance id and bumps
    /// `total_hits`. On miss returns `None` and bumps `total_misses`.
    pub fn lookup(&mut self, key: &PoolKey) -> Option<Uuid> {
        let bucket = self.pools.entry(key.clone()).or_default();
        if let Some(id) = bucket.warm.pop_front() {
            bucket.stats.warm_count = bucket.stats.warm_count.saturating_sub(1);
            bucket.stats.total_hits += 1;
            tracing::info!(?key, %id, "pool hit");
            Some(id)
        } else {
            bucket.stats.total_misses += 1;
            tracing::info!(?key, "pool miss");
            None
        }
    }

    /// Push an instance back into its pool after reset.
    pub fn return_to_pool(&mut self, key: PoolKey, instance_id: Uuid) {
        let bucket = self.pools.entry(key.clone()).or_default();
        // enforce max if configured
        if bucket.config.max > 0 && bucket.warm.len() as u32 >= bucket.config.max {
            bucket.stats.evict_count += 1;
            tracing::warn!(?key, %instance_id, "pool full, evicting on return");
            return;
        }
        bucket.warm.push_back(instance_id);
        bucket.stats.warm_count = bucket.warm.len() as u32;
        tracing::info!(?key, %instance_id, "returned instance to pool");
    }

    /// Drain every pool whose `(backend, workload_class)` matches; used
    /// by `POST /v1/pools/{backend}/{class}/drain`. Returns the evicted
    /// instance ids so the caller can destroy them.
    pub fn drain(&mut self, backend: BackendKind, class: WorkloadClass) -> Vec<Uuid> {
        let mut drained = Vec::new();
        for (key, bucket) in self.pools.iter_mut() {
            if key.backend == backend && key.workload_class == class {
                let count = bucket.warm.len();
                drained.extend(bucket.warm.drain(..));
                bucket.stats.warm_count = 0;
                bucket.stats.evict_count += count as u64;
                tracing::info!(?key, drained = count, "drained pool");
            }
        }
        drained
    }

    /// Update sizing/reset config for a pool (creating it if missing).
    pub fn resize(&mut self, key: &PoolKey, config: PoolConfig) {
        let bucket = self.pools.entry(key.clone()).or_default();
        bucket.config = config;
        tracing::info!(?key, "pool sizing updated");
    }

    pub fn config(&self, key: &PoolKey) -> Option<&PoolConfig> {
        self.pools.get(key).map(|b| &b.config)
    }

    /// Snapshot of stats for a pool. Returns the zero value when the
    /// pool has not been registered yet.
    pub fn stats(&self, key: &PoolKey) -> PoolStats {
        self.pools
            .get(key)
            .map(|b| b.stats.clone())
            .unwrap_or_default()
    }

    /// Snapshot of every known pool — drives `GET /v1/pools`.
    pub fn list_pools(&self) -> Vec<(PoolKey, PoolStats)> {
        self.pools
            .iter()
            .map(|(k, b)| (k.clone(), b.stats.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> PoolKey {
        PoolKey::new(
            BackendKind::KataFc,
            WorkloadClass::AgentTool,
            "sha256:abc".into(),
        )
    }

    #[test]
    fn lookup_miss_then_return_then_hit() {
        let mut mgr = PoolManager::new();
        assert!(mgr.lookup(&key()).is_none());

        let id = Uuid::new_v4();
        mgr.return_to_pool(key(), id);
        assert_eq!(mgr.lookup(&key()), Some(id));
        assert!(mgr.lookup(&key()).is_none());

        let stats = mgr.stats(&key());
        assert_eq!(stats.total_hits, 1);
        assert_eq!(stats.total_misses, 2);
    }

    #[test]
    fn drain_clears_matching_pools() {
        let mut mgr = PoolManager::new();
        mgr.return_to_pool(key(), Uuid::new_v4());
        mgr.return_to_pool(key(), Uuid::new_v4());

        let drained = mgr.drain(BackendKind::KataFc, WorkloadClass::AgentTool);
        assert_eq!(drained.len(), 2);
        assert_eq!(mgr.stats(&key()).warm_count, 0);
    }

    #[test]
    fn resize_caps_growth() {
        let mut mgr = PoolManager::new();
        let cfg = PoolConfig {
            enabled: true,
            max: 1,
            ..PoolConfig::default()
        };
        mgr.resize(&key(), cfg);

        mgr.return_to_pool(key(), Uuid::new_v4());
        mgr.return_to_pool(key(), Uuid::new_v4()); // evicted by max
        let stats = mgr.stats(&key());
        assert_eq!(stats.warm_count, 1);
        assert_eq!(stats.evict_count, 1);
    }

    #[test]
    fn list_pools_returns_all() {
        let mut mgr = PoolManager::new();
        mgr.return_to_pool(key(), Uuid::new_v4());
        let listed = mgr.list_pools();
        assert_eq!(listed.len(), 1);
    }
}
