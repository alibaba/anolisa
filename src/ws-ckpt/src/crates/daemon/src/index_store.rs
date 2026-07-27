use anyhow::Context;
use std::path::Path;
use ws_ckpt_common::{SnapshotIndex, INDEX_FILE};

/// Save a SnapshotIndex to its index.json file on disk.
pub async fn save(ws_dir: &Path, index: &SnapshotIndex) -> anyhow::Result<()> {
    let index_path = ws_dir.join(INDEX_FILE);
    let tmp_path = ws_dir.join(format!("{}.tmp", INDEX_FILE));
    let content =
        serde_json::to_string_pretty(index).context("Failed to serialize SnapshotIndex")?;
    // Write to temp file first, then rename for atomicity
    tokio::fs::write(&tmp_path, &content)
        .await
        .with_context(|| format!("Failed to write {:?}", tmp_path))?;
    tokio::fs::rename(&tmp_path, &index_path)
        .await
        .with_context(|| format!("Failed to rename {:?} -> {:?}", tmp_path, index_path))?;
    Ok(())
}

/// Load a SnapshotIndex from the index.json file on disk.
pub async fn load(ws_dir: &Path) -> anyhow::Result<SnapshotIndex> {
    let index_path = ws_dir.join(INDEX_FILE);
    let content = tokio::fs::read_to_string(&index_path)
        .await
        .with_context(|| format!("Failed to read {:?}", index_path))?;
    let index: SnapshotIndex = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse {:?}", index_path))?;
    Ok(index)
}

/// Rebuild a SnapshotIndex from the filesystem directory structure.
/// Scans for all subdirectories (excluding hidden dirs and known non-snapshot files).
pub async fn rebuild_from_fs(
    ws_dir: &Path,
    workspace_path: std::path::PathBuf,
) -> anyhow::Result<SnapshotIndex> {
    use ws_ckpt_common::SnapshotMeta;
    let mut index = SnapshotIndex::new(workspace_path);
    let mut entries = tokio::fs::read_dir(ws_dir)
        .await
        .with_context(|| format!("Failed to read directory {:?}", ws_dir))?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip non-directories, hidden directories, and known non-snapshot files
        if !entry.file_type().await?.is_dir() || name.starts_with('.') || name == INDEX_FILE {
            continue;
        }
        // Rebuild with minimal metadata (message lost)
        let meta = SnapshotMeta {
            message: None,
            metadata: None,
            pinned: false,
            created_at: chrono::Utc::now(),
            missing: false,
            parent_id: None,
            child_ids: vec![],
        };
        index.snapshots.insert(name, meta);
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use ws_ckpt_common::{SnapshotIndex, SnapshotMeta};

    #[tokio::test]
    async fn save_and_load_round_trip() {
        let dir = tempdir().unwrap();
        let mut index = SnapshotIndex::new(PathBuf::from("/tmp/test-ws"));
        index.snapshots.insert(
            "abcdef1234567890abcdef1234567890abcdef12".to_string(),
            SnapshotMeta {
                message: Some("initial snapshot".to_string()),
                metadata: Some(serde_json::json!({"event": "init"})),
                pinned: true,
                created_at: chrono::Utc::now(),
                missing: false,
                parent_id: None,
                child_ids: vec![],
            },
        );

        // Save
        save(dir.path(), &index).await.expect("save failed");

        // Load
        let loaded = load(dir.path()).await.expect("load failed");
        assert_eq!(loaded.workspace_path, index.workspace_path);
        assert_eq!(loaded.snapshots.len(), 1);
        assert!(loaded
            .snapshots
            .contains_key("abcdef1234567890abcdef1234567890abcdef12"));
        let meta = &loaded.snapshots["abcdef1234567890abcdef1234567890abcdef12"];
        assert_eq!(meta.message.as_deref(), Some("initial snapshot"));
        assert!(meta.pinned);
    }

    #[tokio::test]
    async fn save_atomicity_no_tmp_residue() {
        // After save, index.json should exist but index.json.tmp should NOT
        let dir = tempdir().unwrap();
        let index = SnapshotIndex::new(PathBuf::from("/ws"));
        save(dir.path(), &index).await.expect("save failed");

        assert!(dir.path().join(INDEX_FILE).exists());
        assert!(
            !dir.path().join(format!("{}.tmp", INDEX_FILE)).exists(),
            "tmp file should not remain after save"
        );
    }

    #[tokio::test]
    async fn load_nonexistent_file_returns_error() {
        let dir = tempdir().unwrap();
        let result = load(dir.path()).await;
        assert!(result.is_err(), "loading from empty dir should fail");
    }

    #[tokio::test]
    async fn rebuild_from_fs_finds_all_snapshot_dirs() {
        let dir = tempdir().unwrap();
        // Create directories with various naming patterns
        std::fs::create_dir(dir.path().join("abcdef1234567890abcdef1234567890abcdef12")).unwrap();
        std::fs::create_dir(dir.path().join("1111111111111111111111111111111111111111")).unwrap();
        std::fs::create_dir(dir.path().join("msg1-step0")).unwrap();
        std::fs::create_dir(dir.path().join("my-snapshot")).unwrap();

        let index = rebuild_from_fs(dir.path(), PathBuf::from("/ws"))
            .await
            .expect("rebuild_from_fs failed");

        assert_eq!(index.snapshots.len(), 4);
        assert!(index
            .snapshots
            .contains_key("abcdef1234567890abcdef1234567890abcdef12"));
        assert!(index
            .snapshots
            .contains_key("1111111111111111111111111111111111111111"));
        assert!(index.snapshots.contains_key("msg1-step0"));
        assert!(index.snapshots.contains_key("my-snapshot"));
    }

    #[tokio::test]
    async fn rebuild_from_fs_ignores_hidden_dirs_and_files() {
        let dir = tempdir().unwrap();
        // Create matching and non-matching entries
        std::fs::create_dir(dir.path().join("abcdef1234567890abcdef1234567890abcdef12")).unwrap();
        std::fs::create_dir(dir.path().join("msg1-step0")).unwrap();
        std::fs::create_dir(dir.path().join("my-snapshot")).unwrap();
        // Hidden directory should be ignored
        std::fs::create_dir(dir.path().join(".hidden")).unwrap();
        // Regular file should be ignored
        std::fs::write(dir.path().join("index.json"), "{}").unwrap();

        let index = rebuild_from_fs(dir.path(), PathBuf::from("/ws"))
            .await
            .expect("rebuild_from_fs failed");

        assert_eq!(index.snapshots.len(), 3);
        assert!(index
            .snapshots
            .contains_key("abcdef1234567890abcdef1234567890abcdef12"));
        assert!(index.snapshots.contains_key("msg1-step0"));
        assert!(index.snapshots.contains_key("my-snapshot"));
    }
}
