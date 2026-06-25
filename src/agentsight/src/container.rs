//! Container ID extraction from `/proc/{pid}/cgroup`.
//!
//! Supports Docker (cgroup v1 & v2), containerd, and Kubernetes cgroup
//! layouts.  Returns `None` for non-container processes.

/// Read `/proc/{pid}/cgroup` and extract the container ID.
///
/// Returns `None` when the process is not running inside a container or
/// when the cgroup file cannot be read.
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
    // 1. Docker cgroup v1: .../docker/<64hex>
    if let Some(pos) = path.find("/docker/") {
        let candidate = &path[pos + "/docker/".len()..];
        // Strip any trailing path segments
        let candidate = candidate.split('/').next().unwrap_or(candidate);
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
        // The container ID is the last 64-hex segment.
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

/// Returns `true` when `s` is exactly 64 lowercase-hex characters.
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
}
