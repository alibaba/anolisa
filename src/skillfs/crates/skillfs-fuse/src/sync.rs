//! Background store-sync worker: receives debounced events from the
//! FUSE write path and re-parses affected `SKILL.md` files into the
//! shared store.

use std::collections::HashMap;
use std::path::PathBuf;

use skillfs_core::{SharedSkillStore, parser};
use tracing::{info, warn};

/// Events sent from FUSE write callbacks to the background sync task.
#[derive(Debug)]
pub(crate) enum SyncEvent {
    /// Re-parse a skill's SKILL.md after write/create.
    Reparse { skill_name: String },
}

/// Spawn the background store-sync worker thread.
///
/// Collects events from the FUSE write path, batches them with a 50 ms
/// debounce window, then re-parses the affected SKILL.md files and updates
/// the shared store.
pub(crate) fn spawn_sync_worker(
    rx: std::sync::mpsc::Receiver<SyncEvent>,
    store: SharedSkillStore,
    source_base: PathBuf,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            // Collect more events within a 50 ms window (debounce).
            let mut pending: HashMap<String, SyncEvent> = HashMap::new();
            match &first {
                SyncEvent::Reparse { skill_name } => {
                    pending.insert(skill_name.clone(), first);
                }
            }
            while let Ok(ev) = rx.recv_timeout(std::time::Duration::from_millis(50)) {
                match &ev {
                    SyncEvent::Reparse { skill_name } => {
                        pending.insert(skill_name.clone(), ev);
                    }
                }
            }

            // Process the batch.
            for (_skill_name, event) in pending {
                match event {
                    SyncEvent::Reparse { ref skill_name } => {
                        let md_path = source_base.join(skill_name).join("SKILL.md");
                        match parser::parse_skill_file(&md_path) {
                            Ok(mut entry) => {
                                // The directory name is the authoritative store key.
                                // Override metadata.name so that a stale frontmatter
                                // `name:` field (e.g. after a rename) can never
                                // re-insert an entry under the old name.
                                entry.metadata.name = skill_name.clone();
                                info!(
                                    name = %skill_name,
                                    "sync: re-parsed SKILL.md"
                                );
                                store.write().upsert(entry);
                            }
                            Err(e) => {
                                warn!(
                                    name = %skill_name,
                                    error = %e,
                                    "sync: re-parse failed"
                                );
                            }
                        }
                    }
                }
            }
        }
        info!("sync worker exiting");
    })
}
