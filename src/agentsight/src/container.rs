//! Container ID extraction from `/proc/{pid}/cgroup`.
//!
//! Standalone module (Footprint Ladder Level 3) because container detection
//! will expand to cover cgroup v2 unified hierarchy, runtime-specific
//! parsing, and optional container-name resolution via containerd API.
//! Keeping it separate from `ffi.rs` avoids bloating the FFI boundary file.
//!
//! Supports Docker (cgroup v1 & v2), containerd, and Kubernetes cgroup
//! layouts.  Returns `None` for non-container processes.

use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Maximum number of entries in the container-ID cache.
const CACHE_CAPACITY: usize = 256;

/// Time-to-live for a cache entry.
const CACHE_TTL: Duration = Duration::from_secs(60);

struct CacheEntry {
    container_id: Option<String>,
    inserted_at: Instant,
}

static CONTAINER_ID_CACHE: LazyLock<Mutex<LruCache<u32, CacheEntry>>> = LazyLock::new(|| {
    Mutex::new(LruCache::new(
        NonZeroUsize::new(CACHE_CAPACITY).expect("CACHE_CAPACITY > 0"),
    ))
});

/// Cached wrapper around [`extract_container_id`].
///
/// Returns the cached value if present and less than 60 seconds old.
/// On miss or expiry, calls `extract_container_id` and inserts the result.
/// Uses `lru::LruCache` for O(1) eviction, consistent with other agentsight
/// caches (HTTP aggregator, response map, id resolver).
pub fn extract_container_id_cached(pid: u32) -> Option<String> {
    let mut cache = match CONTAINER_ID_CACHE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    if let Some(entry) = cache.get(&pid) {
        if entry.inserted_at.elapsed() < CACHE_TTL {
            return entry.container_id.clone();
        }
    }

    let result = extract_container_id(pid);

    cache.put(
        pid,
        CacheEntry {
            container_id: result.clone(),
            inserted_at: Instant::now(),
        },
    );

    result
}

/// Read `/proc/{pid}/cgroup` and extract the container ID.
///
/// Returns `None` when the process is not running inside a container or
/// when the cgroup file cannot be read.
///
/// # Panic safety
///
/// This function and all callees are guaranteed no-panic: only infallible
/// string operations and `Option`-returning methods are used.  Safe to call
/// from FFI (`build_llm_data`).
pub fn extract_container_id(pid: u32) -> Option<String> {
    let path = format!("/proc/{pid}/cgroup");
    match std::fs::read_to_string(&path) {
        Ok(content) => parse_container_id_from_cgroup(&content),
        Err(e) => {
            log::debug!("failed to read {path}: {e}");
            None
        }
    }
}

/// Pure function: extract a 64-char hex container ID from raw cgroup
/// file content.
///
/// Recognised layouts (checked in order):
///
/// 1. Docker cgroup v1 — `.../docker/<64hex>`
/// 2. Docker cgroup v2 — `docker-<64hex>.scope`
/// 3. Kubernetes       — `/kubepods/.../<64hex>`
/// 4. containerd       — last path segment is exactly 64 hex chars
pub fn parse_container_id_from_cgroup(content: &str) -> Option<String> {
    for line in content.lines() {
        // The third colon-separated field is the cgroup path.
        let cgroup_path = match line.splitn(3, ':').nth(2) {
            Some(p) => p,
            None => continue,
        };

        if let Some(id) = try_extract_from_path(cgroup_path) {
            return Some(id);
        }
    }
    None
}

/// Try to extract a container ID from a single cgroup path string.
fn try_extract_from_path(path: &str) -> Option<String> {
    // 1. Docker cgroup v1: .../docker/<64hex>  (skip overlay2 layer paths)
    if let Some(pos) = path.find("/docker/") {
        let candidate = &path[pos + "/docker/".len()..];
        // split('/').next() always returns Some for non-empty input
        let candidate = candidate.split('/').next().unwrap_or("");
        if is_64_hex(candidate) {
            return Some(candidate.to_string());
        }
    }

    // 2. Docker cgroup v2: docker-<64hex>.scope
    for segment in path.rsplit('/') {
        if let Some(rest) = segment.strip_prefix("docker-") {
            if let Some(hex) = rest.strip_suffix(".scope") {
                if is_64_hex(hex) {
                    return Some(hex.to_string());
                }
            }
        }
    }

    // 3. Kubernetes: /kubepods/.../<64hex>
    if path.contains("/kubepods") {
        // rsplit('/').next() always returns Some (at least the full string)
        if let Some(segment) = path.rsplit('/').next() {
            if is_64_hex(segment) {
                return Some(segment.to_string());
            }
        }
    }

    // 4. containerd / generic: last path segment is exactly 64 hex chars.
    if let Some(segment) = path.rsplit('/').next() {
        if is_64_hex(segment) {
            return Some(segment.to_string());
        }
    }

    None
}

/// Returns `true` when `s` is exactly 64 hex characters (case-insensitive).
fn is_64_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_cgroup_v1() {
        let content =
            "12:devices:/docker/a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2\n";
        let id = parse_container_id_from_cgroup(content).unwrap();
        assert_eq!(
            id,
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
        );
    }

    #[test]
    fn docker_cgroup_v2() {
        let content = "0::/system.slice/docker-a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2.scope\n";
        let id = parse_container_id_from_cgroup(content).unwrap();
        assert_eq!(
            id,
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
        );
    }

    #[test]
    fn containerd() {
        let content =
            "0::/default/a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2\n";
        let id = parse_container_id_from_cgroup(content).unwrap();
        assert_eq!(
            id,
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
        );
    }

    #[test]
    fn kubernetes() {
        let content = "11:memory:/kubepods/burstable/pod1234/a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2\n";
        let id = parse_container_id_from_cgroup(content).unwrap();
        assert_eq!(
            id,
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
        );
    }

    #[test]
    fn non_container_host_process() {
        let content = "12:devices:/user.slice/user-1000.slice/session-1.scope\n\
                        11:memory:/user.slice\n\
                        0::/init.scope\n";
        assert!(parse_container_id_from_cgroup(content).is_none());
    }

    #[test]
    fn empty_content() {
        assert!(parse_container_id_from_cgroup("").is_none());
    }

    #[test]
    fn multiline_picks_first_match() {
        let content = "12:devices:/\n\
                        11:memory:/docker/a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2\n\
                        0::/system.slice\n";
        let id = parse_container_id_from_cgroup(content).unwrap();
        assert_eq!(
            id,
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
        );
    }

    #[test]
    fn short_hex_is_not_container_id() {
        // 32 chars — too short for a container ID
        let content = "0::/docker/a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4\n";
        assert!(parse_container_id_from_cgroup(content).is_none());
    }

    #[test]
    fn overlay2_is_not_container_id() {
        // overlay2 layer IDs are long hex but sit under /docker/overlay2/, not /docker/<id>
        let content = "0::/system.slice/docker-abcdef.scope/docker/overlay2/a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2/merged\n";
        // Should match the docker-abcdef.scope pattern (if 64 hex), NOT the overlay2 layer
        // In this case docker-abcdef.scope only has 6 hex chars, so no match at all
        assert!(parse_container_id_from_cgroup(content).is_none());
    }

    #[test]
    fn uppercase_hex_accepted() {
        let content =
            "0::/docker/A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2\n";
        let id = parse_container_id_from_cgroup(content).unwrap();
        assert_eq!(
            id,
            "A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2"
        );
    }

    #[test]
    fn no_trailing_newline() {
        let content = "0::/docker/a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        let id = parse_container_id_from_cgroup(content).unwrap();
        assert_eq!(
            id,
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
        );
    }

    #[test]
    fn test_cache_returns_same_value_on_second_call() {
        // Call twice with the same pid — both should return the same value
        // and neither should panic.
        let pid = std::process::id();
        let first = extract_container_id_cached(pid);
        let second = extract_container_id_cached(pid);
        assert_eq!(first, second);
    }

    #[test]
    fn test_cache_none_for_nonexistent_pid() {
        // pid 999999 almost certainly does not exist; should return None
        // without panicking.
        let result = extract_container_id_cached(999_999);
        assert!(result.is_none());
    }
}
