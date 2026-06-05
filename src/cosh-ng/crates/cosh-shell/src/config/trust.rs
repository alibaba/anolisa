use std::path::{Path, PathBuf};

use super::load::dirs_next_or_home;
use super::CoshConfig;

pub fn trust_project_root(root: &Path) -> Result<(), String> {
    let path = project_trust_store_path()
        .ok_or_else(|| "HOME is not set; cannot persist trust".to_string())?;
    add_trusted_project_root_to_store_path(&path, root)
}

pub fn untrust_project_root(root: &Path) -> Result<(), String> {
    let path = project_trust_store_path()
        .ok_or_else(|| "HOME is not set; cannot persist trust".to_string())?;
    remove_trusted_project_root_from_store_path(&path, root)
}

pub fn clear_project_trust_store() -> Result<(), String> {
    let path = project_trust_store_path()
        .ok_or_else(|| "HOME is not set; cannot persist trust".to_string())?;
    write_trusted_project_roots_to_store_path(&path, &[])
}

pub(super) fn project_trust_store_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("COSH_SHELL_PROJECT_TRUST_STORE") {
        return Some(PathBuf::from(path));
    }
    dirs_next_or_home().map(|d| d.join(".config/cosh/trusted-project-hooks"))
}

pub(super) fn load_project_trust_store(config: &mut CoshConfig, path: &Path) {
    config
        .trusted_project_roots
        .extend(read_trusted_project_roots_from_store_path(path));
}

pub(super) fn read_trusted_project_roots_from_store_path(path: &Path) -> Vec<PathBuf> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(PathBuf::from)
        .collect()
}

pub(super) fn add_trusted_project_root_to_store_path(
    path: &Path,
    root: &Path,
) -> Result<(), String> {
    let root = canonical_project_root(root);
    let mut roots = read_trusted_project_roots_from_store_path(path)
        .into_iter()
        .map(|root| canonical_project_root(&root))
        .collect::<Vec<_>>();
    if !roots.iter().any(|existing| existing == &root) {
        roots.push(root);
    }
    write_trusted_project_roots_to_store_path(path, &roots)
}

pub(super) fn remove_trusted_project_root_from_store_path(
    path: &Path,
    root: &Path,
) -> Result<(), String> {
    let root = canonical_project_root(root);
    let mut roots = read_trusted_project_roots_from_store_path(path)
        .into_iter()
        .map(|root| canonical_project_root(&root))
        .collect::<Vec<_>>();
    roots.retain(|existing| existing != &root);
    write_trusted_project_roots_to_store_path(path, &roots)
}

pub(super) fn write_trusted_project_roots_to_store_path(
    path: &Path,
    roots: &[PathBuf],
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("create trust store directory failed: {err}"))?;
    }
    let mut content = String::new();
    content.push_str("# cosh-shell trusted project hook roots\n");
    for root in roots {
        content.push_str(&root.to_string_lossy());
        content.push('\n');
    }
    std::fs::write(path, content).map_err(|err| format!("write project trust store failed: {err}"))
}

fn canonical_project_root(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}
