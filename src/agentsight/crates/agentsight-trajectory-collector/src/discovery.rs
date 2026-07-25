//! QoderWork / Qoder session file discovery.
//!
//! Scans `<home>/.qoderwork/projects` and `<home>/.qoder/projects` for JSONL
//! session files. Ported from AgentOpt's collector crate; home directories are
//! enumerated explicitly (`/root` + `/home/*`) because the collector runs
//! inside the root-owned `agentsight trace` process, not the agent's shell.

use std::path::{Path, PathBuf};

/// A discovered session file.
#[derive(Debug, Clone)]
pub struct DiscoveredSession {
    /// Path to the JSONL file.
    pub path: PathBuf,
    /// Decoded project name (e.g., "data-skillopt").
    pub project: String,
    /// Session identifier (filename UUID; subagents use
    /// `<parent>:subagent:<uuid>` to stay unique per file).
    pub session_id: String,
    /// Whether this is a sub-agent session.
    pub is_subagent: bool,
    /// Which product wrote the file: "qoder" or "qoderwork".
    pub source: String,
}

/// Projects directory names relative to each home directory.
const PROJECT_DIR_NAMES: &[&str] = &[".qoderwork/projects", ".qoder/projects"];

/// Discover all QoderWork/Qoder sessions.
///
/// `scan_dirs` overrides the default scan roots; each entry is treated as a
/// projects directory. When `None`, `/root` and every `/home/*` entry are
/// probed for `.qoderwork/projects` / `.qoder/projects`.
pub fn discover_sessions(scan_dirs: Option<&[PathBuf]>) -> Vec<DiscoveredSession> {
    let dirs = match scan_dirs {
        Some(list) => list.to_vec(),
        None => default_projects_dirs(),
    };

    let mut sessions = Vec::new();
    for dir in dirs {
        if !dir.exists() {
            log::debug!(
                "Trajectory scan: skipping non-existent dir {}",
                dir.display()
            );
            continue;
        }
        let source = source_from_path(&dir);
        sessions.extend(discover_in_projects_dir(&dir, &source));
    }

    // Sort by path for stable output
    sessions.sort_by(|a, b| a.path.cmp(&b.path));
    sessions
}

/// Default projects directories: `/root` + `/home/*`, each probed for the
/// known Qoder/QoderWork layout.
fn default_projects_dirs() -> Vec<PathBuf> {
    let mut homes = vec![PathBuf::from("/root")];
    if let Ok(entries) = std::fs::read_dir("/home") {
        for entry in entries.flatten() {
            homes.push(entry.path());
        }
    }

    let mut dirs = Vec::new();
    for home in homes {
        for name in PROJECT_DIR_NAMES {
            let p = home.join(name);
            if p.exists() {
                dirs.push(p);
            }
        }
    }
    dirs
}

/// Derive the source label from a projects directory path.
fn source_from_path(dir: &Path) -> String {
    if dir.to_string_lossy().contains(".qoderwork") {
        "qoderwork".to_string()
    } else {
        "qoder".to_string()
    }
}

/// Discover sessions within a single projects directory.
fn discover_in_projects_dir(projects_dir: &Path, source: &str) -> Vec<DiscoveredSession> {
    let mut sessions = Vec::new();

    let entries = match std::fs::read_dir(projects_dir) {
        Ok(e) => e,
        Err(err) => {
            log::warn!(
                "Trajectory scan: failed to read {}: {err}",
                projects_dir.display()
            );
            return sessions;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let project = decode_project_dir(&entry.file_name().to_string_lossy());
        sessions.extend(discover_in_project_dir(&path, &project, source));
    }

    sessions
}

/// Discover sessions within a single project directory.
fn discover_in_project_dir(
    project_dir: &Path,
    project: &str,
    source: &str,
) -> Vec<DiscoveredSession> {
    let mut sessions = Vec::new();

    let entries = match std::fs::read_dir(project_dir) {
        Ok(e) => e,
        Err(err) => {
            log::debug!(
                "Trajectory scan: failed to read {}: {err}",
                project_dir.display()
            );
            return sessions;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);

        // Main session files: <uuid>.jsonl
        if !is_dir && name.ends_with(".jsonl") {
            let stem = name.trim_end_matches(".jsonl");
            // Skip agent-* files (sub-agents are stored under subagents/)
            if stem.starts_with("agent-") || !is_valid_session_id(stem) {
                continue;
            }
            sessions.push(DiscoveredSession {
                path: path.clone(),
                project: project.to_string(),
                session_id: stem.to_string(),
                is_subagent: false,
                source: source.to_string(),
            });
            continue;
        }

        // Sub-agent sessions: <uuid>/subagents/agent-<uuid>.jsonl
        if is_dir && is_valid_session_id(&name) {
            let subagents_dir = path.join("subagents");
            let Ok(sub_entries) = std::fs::read_dir(&subagents_dir) else {
                continue;
            };
            for sub_entry in sub_entries.flatten() {
                let sub_name = sub_entry.file_name().to_string_lossy().to_string();
                if !sub_name.ends_with(".jsonl") || !sub_name.starts_with("agent-") {
                    continue;
                }
                let sub_stem = sub_name
                    .trim_end_matches(".jsonl")
                    .trim_start_matches("agent-");
                if is_valid_session_id(sub_stem) {
                    sessions.push(DiscoveredSession {
                        path: sub_entry.path(),
                        project: project.to_string(),
                        session_id: format!("{name}:subagent:{sub_stem}"),
                        is_subagent: true,
                        source: source.to_string(),
                    });
                }
            }
        }
    }

    sessions
}

/// Decode a QoderWork/Qoder project directory name.
///
/// Examples:
/// - `-data-skillopt` → `data-skillopt`
/// - `-Users-john-projects-myapp` → `myapp`
/// - `myproject` → `myproject`
pub fn decode_project_dir(encoded: &str) -> String {
    if !encoded.starts_with('-') {
        return normalize_name(encoded);
    }

    let parts: Vec<&str> = encoded.split('-').filter(|s| !s.is_empty()).collect();

    // Look for known parent directory markers (use the LAST match)
    let parent_markers = [
        "code",
        "coding",
        "dev",
        "development",
        "projects",
        "repos",
        "src",
        "work",
        "workspace",
    ];

    let mut best_match: Option<usize> = None;
    for (i, part) in parts.iter().enumerate() {
        if parent_markers.contains(&part.to_lowercase().as_str()) {
            best_match = Some(i);
        }
    }

    if let Some(i) = best_match {
        if i + 1 < parts.len() {
            let project = parts[i + 1..].join("-");
            if !project.is_empty() {
                return normalize_name(&project);
            }
        }
    }

    // No marker found — return all parts joined (just strip leading dash)
    normalize_name(&parts.join("-"))
}

/// Simple name normalization.
fn normalize_name(s: &str) -> String {
    s.trim_start_matches(['-', '.']).to_string()
}

/// Check if a string looks like a valid session ID (UUID format).
fn is_valid_session_id(s: &str) -> bool {
    // UUID format: 8-4-4-4-12 hex digits
    if s.len() != 36 {
        return false;
    }
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let expected_lens = [8, 4, 4, 4, 12];
    for (part, &expected_len) in parts.iter().zip(expected_lens.iter()) {
        if part.len() != expected_len || !part.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID_A: &str = "be0aa488-4e56-4604-bdf0-e12cc387392d";
    const UUID_B: &str = "11111111-2222-3333-4444-555555555555";

    fn tmp_projects_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("traj-disc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_decode_project_dir() {
        assert_eq!(decode_project_dir("-data-skillopt"), "data-skillopt");
        assert_eq!(decode_project_dir("-Users-john-projects-myapp"), "myapp");
        assert_eq!(decode_project_dir("myproject"), "myproject");
        assert_eq!(decode_project_dir("-data-work-repos-foo"), "foo");
    }

    #[test]
    fn test_is_valid_session_id() {
        assert!(is_valid_session_id(UUID_A));
        assert!(!is_valid_session_id("agent-123"));
        assert!(!is_valid_session_id("not-a-uuid"));
        assert!(!is_valid_session_id(""));
    }

    #[test]
    fn test_discover_filters_and_subagents() {
        let projects = tmp_projects_dir("main");
        let proj = projects.join("-data-myapp");
        std::fs::create_dir_all(&proj).unwrap();

        // Valid main session
        std::fs::write(proj.join(format!("{UUID_A}.jsonl")), "{}\n").unwrap();
        // Invalid names must be skipped
        std::fs::write(proj.join("notes.jsonl"), "{}\n").unwrap();
        std::fs::write(proj.join(format!("agent-{UUID_B}.jsonl")), "{}\n").unwrap();
        // Sub-agent session under <uuid>/subagents/
        let sub = proj.join(UUID_A).join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(format!("agent-{UUID_B}.jsonl")), "{}\n").unwrap();

        let sessions = discover_sessions(Some(std::slice::from_ref(&projects)));
        assert_eq!(sessions.len(), 2, "one main + one subagent: {sessions:?}");

        let main = sessions.iter().find(|s| !s.is_subagent).unwrap();
        assert_eq!(main.session_id, UUID_A);
        assert_eq!(main.project, "data-myapp");
        assert_eq!(main.source, "qoder");

        let sub = sessions.iter().find(|s| s.is_subagent).unwrap();
        assert_eq!(sub.session_id, format!("{UUID_A}:subagent:{UUID_B}"));

        let _ = std::fs::remove_dir_all(&projects);
    }

    #[test]
    fn test_source_from_path() {
        assert_eq!(
            source_from_path(Path::new("/root/.qoderwork/projects")),
            "qoderwork"
        );
        assert_eq!(
            source_from_path(Path::new("/home/u/.qoder/projects")),
            "qoder"
        );
    }
}
