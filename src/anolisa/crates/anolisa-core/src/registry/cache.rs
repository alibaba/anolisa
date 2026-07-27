//! On-disk cache for the distribution index.
//!
//! Layout under the cache root (`~/.cache/anolisa/registry/`):
//! - `index.toml` — last successfully fetched index.
//! - `index.toml.fetched_at` — RFC3339 UTC timestamp of that fetch.
//!
//! Freshness is derived from `now - fetched_at` versus the configured TTL.
//! A missing or unparseable stamp is treated as stale (force refetch), never
//! as an error, so a corrupt cache self-heals on the next online fetch.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};

use super::error::RegistryError;
use crate::distribution::DistributionIndex;

/// Filename of the cached index.
const INDEX_FILE: &str = "index.toml";
/// Filename of the fetch-time stamp sibling.
const STAMP_FILE: &str = "index.toml.fetched_at";

/// Cache for the distribution index, rooted at a single directory.
pub(super) struct RegistryCache {
    root: PathBuf,
}

impl RegistryCache {
    /// Create a cache handle. The directory is created lazily on first
    /// [`store`](Self::store); construction touches no filesystem.
    pub(super) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn index_path(&self) -> PathBuf {
        self.root.join(INDEX_FILE)
    }

    fn stamp_path(&self) -> PathBuf {
        self.root.join(STAMP_FILE)
    }

    /// Whether a cached index file exists (regardless of freshness).
    pub(super) fn has_index(&self) -> bool {
        self.index_path().is_file()
    }

    /// Whether the cached index is within `ttl` of its fetch time.
    ///
    /// Returns `false` (stale) when the index or stamp is missing, the stamp
    /// is unparseable, or the age meets/exceeds the TTL. A zero TTL therefore
    /// always reports stale, forcing a refetch.
    pub(super) fn is_fresh(&self, ttl: Duration) -> bool {
        if !self.has_index() {
            return false;
        }
        let Ok(stamp) = std::fs::read_to_string(self.stamp_path()) else {
            return false;
        };
        let Ok(fetched_at) = DateTime::parse_from_rfc3339(stamp.trim()) else {
            return false;
        };
        let age = Utc::now().signed_duration_since(fetched_at.with_timezone(&Utc));
        // Negative age (clock skew / future stamp) counts as fresh.
        match age.to_std() {
            Ok(age) => age < ttl,
            Err(_) => true,
        }
    }

    /// Parse the cached index from disk.
    ///
    /// # Errors
    /// [`RegistryError::Io`] if the file cannot be read, [`RegistryError::Parse`]
    /// if its TOML is invalid.
    pub(super) fn load_index(&self) -> Result<DistributionIndex, RegistryError> {
        let path = self.index_path();
        let text = read_to_string(&path)?;
        DistributionIndex::from_toml_str(&text).map_err(|reason| RegistryError::Parse { reason })
    }

    /// Persist a freshly fetched index plus a `now` stamp, creating the cache
    /// directory if needed. Writes go through a temp sibling + rename so a
    /// concurrent reader never sees a half-written index.
    ///
    /// # Errors
    /// [`RegistryError::Io`] on any filesystem failure.
    pub(super) fn store(&self, toml_text: &str) -> Result<(), RegistryError> {
        std::fs::create_dir_all(&self.root).map_err(|source| RegistryError::Io {
            path: self.root.clone(),
            source,
        })?;
        atomic_write(&self.index_path(), toml_text.as_bytes())?;
        let stamp = Utc::now().to_rfc3339();
        atomic_write(&self.stamp_path(), stamp.as_bytes())?;
        Ok(())
    }

    /// Cache path for a component version's `meta.toml`.
    ///
    /// Path-separator characters in the (registry-supplied) component/version
    /// are neutralized to `_` so a crafted index row cannot escape the cache
    /// directory.
    fn meta_path(&self, component: &str, version: &str) -> PathBuf {
        let safe = |s: &str| s.replace(['/', '\\'], "_");
        self.root
            .join("artifacts")
            .join(format!("{}-{}-meta.toml", safe(component), safe(version)))
    }

    /// Read a cached `meta.toml` if present. Meta is immutable per version, so
    /// a cache hit needs no freshness check.
    ///
    /// # Errors
    /// [`RegistryError::Io`] on a read failure other than absence.
    pub(super) fn read_meta(
        &self,
        component: &str,
        version: &str,
    ) -> Result<Option<String>, RegistryError> {
        let path = self.meta_path(component, version);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(RegistryError::Io { path, source }),
        }
    }

    /// Persist a fetched `meta.toml` under `artifacts/`.
    ///
    /// # Errors
    /// [`RegistryError::Io`] on any filesystem failure.
    pub(super) fn write_meta(
        &self,
        component: &str,
        version: &str,
        text: &str,
    ) -> Result<(), RegistryError> {
        let path = self.meta_path(component, version);
        let dir = self.root.join("artifacts");
        std::fs::create_dir_all(&dir).map_err(|source| RegistryError::Io {
            path: dir.clone(),
            source,
        })?;
        atomic_write(&path, text.as_bytes())
    }
}

fn read_to_string(path: &Path) -> Result<String, RegistryError> {
    std::fs::read_to_string(path).map_err(|source| RegistryError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Write `bytes` to `path` via a `<path>.tmp` sibling then rename, so readers
/// observe either the old or the new file, never a partial one.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), RegistryError> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|source| RegistryError::Io {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| RegistryError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const SAMPLE_INDEX: &str = r#"
        schema_version = 1
        channel = "stable"
        [[entries]]
        component = "tokenless"
        version = "0.5.0"
        channel = "stable"
        artifact_type = "tar_gz"
        backend = "local-file"
        url = "http://127.0.0.1:8080/x.tar.gz"
        os = "linux"
        arch = "x86_64"
    "#;

    #[test]
    fn empty_cache_is_not_fresh_and_has_no_index() {
        let dir = TempDir::new().unwrap();
        let cache = RegistryCache::new(dir.path().join("registry"));
        assert!(!cache.has_index());
        assert!(!cache.is_fresh(Duration::from_secs(3600)));
    }

    #[test]
    fn store_then_load_roundtrips_and_is_fresh() {
        let dir = TempDir::new().unwrap();
        let cache = RegistryCache::new(dir.path().join("registry"));
        cache.store(SAMPLE_INDEX).expect("store");
        assert!(cache.has_index());
        assert!(cache.is_fresh(Duration::from_secs(3600)));
        let idx = cache.load_index().expect("load");
        assert_eq!(idx.entries.len(), 1);
    }

    #[test]
    fn zero_ttl_is_always_stale() {
        let dir = TempDir::new().unwrap();
        let cache = RegistryCache::new(dir.path().join("registry"));
        cache.store(SAMPLE_INDEX).expect("store");
        assert!(!cache.is_fresh(Duration::ZERO));
    }

    #[test]
    fn corrupt_stamp_is_treated_as_stale() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("registry");
        let cache = RegistryCache::new(root.clone());
        cache.store(SAMPLE_INDEX).expect("store");
        std::fs::write(root.join(STAMP_FILE), "not-a-timestamp").unwrap();
        assert!(!cache.is_fresh(Duration::from_secs(3600)));
    }
}
