use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// SkillEvent
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum SkillEvent {
    /// New SKILL.md detected
    Created(PathBuf),
    /// Existing SKILL.md changed
    Modified(PathBuf),
    /// SKILL.md removed
    Deleted(PathBuf),
    /// New skill directory created
    DirCreated(PathBuf),
    /// Skill directory removed
    DirDeleted(PathBuf),
}

// ---------------------------------------------------------------------------
// WatchError
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum WatchError {
    #[error("notify error: {0}")]
    NotifyError(#[from] notify::Error),
    #[error("path not found: {0}")]
    PathNotFound(PathBuf),
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Start watching a source directory for SKILL.md changes.
///
/// Returns a channel receiver for skill events.
/// The watcher runs as a tokio task.
pub async fn watch_source(
    source: PathBuf,
    debounce_ms: u64,
) -> Result<mpsc::UnboundedReceiver<SkillEvent>, WatchError> {
    if !source.exists() {
        return Err(WatchError::PathNotFound(source));
    }

    let (tx, rx) = mpsc::unbounded_channel();

    // Spawn the watcher task
    let _handle = tokio::task::spawn(async move {
        if let Err(e) = run_watcher(source, debounce_ms, tx).await {
            tracing::error!("watcher error: {e}");
        }
    });

    Ok(rx)
}

async fn run_watcher(
    source: PathBuf,
    debounce_ms: u64,
    tx: mpsc::UnboundedSender<SkillEvent>,
) -> Result<(), WatchError> {
    use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
    use std::collections::HashMap;
    use std::time::Instant;

    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();

    let mut watcher = RecommendedWatcher::new(
        move |result: Result<notify::Event, notify::Error>| {
            if let Ok(event) = result {
                let _ = notify_tx.send(event);
            }
        },
        Config::default(),
    )?;

    watcher.watch(&source, RecursiveMode::Recursive)?;

    // Debounce state: path -> (last_event_time, last_event_kind)
    let debounce = std::time::Duration::from_millis(debounce_ms);
    let mut pending: HashMap<PathBuf, (Instant, notify::EventKind)> = HashMap::new();

    loop {
        tokio::select! {
            Some(event) = notify_rx.recv() => {
                for path in &event.paths {
                    pending.insert(path.clone(), (Instant::now(), event.kind));
                }
            }
            _ = tokio::time::sleep(debounce) => {
                let now = Instant::now();
                let ready: Vec<(PathBuf, notify::EventKind)> = pending
                    .iter()
                    .filter(|(_, (time, _))| now.duration_since(*time) >= debounce)
                    .map(|(path, (_, kind))| (path.clone(), *kind))
                    .collect();

                for (path, kind) in ready {
                    pending.remove(&path);
                    let Some(event) = classify_event(&source, &path, kind) else {
                        continue;
                    };
                    if tx.send(event).is_err() {
                        return Ok(()); // receiver dropped
                    }
                }
            }
        }
    }
}

/// Classify a filesystem event into a SkillEvent, filtering irrelevant files.
fn classify_event(source: &Path, path: &Path, kind: notify::EventKind) -> Option<SkillEvent> {
    use notify::EventKind;

    // Check if this is a SKILL.md file
    let is_skill_md = path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md");

    // Check if path is inside an immediate subdirectory of source
    let is_in_skill_dir = path
        .parent()
        .and_then(|p| p.parent())
        .map(|pp| pp == source)
        .unwrap_or(false);

    // Check if path IS an immediate subdirectory of source
    let is_immediate_child = path.parent().map(|p| p == source).unwrap_or(false);

    if is_skill_md && is_in_skill_dir {
        match kind {
            EventKind::Create(_) => Some(SkillEvent::Created(path.to_path_buf())),
            EventKind::Modify(_) => Some(SkillEvent::Modified(path.to_path_buf())),
            EventKind::Remove(_) => Some(SkillEvent::Deleted(path.to_path_buf())),
            _ => None,
        }
    } else if is_immediate_child && path.is_dir() {
        match kind {
            EventKind::Create(_) => Some(SkillEvent::DirCreated(path.to_path_buf())),
            EventKind::Remove(_) => Some(SkillEvent::DirDeleted(path.to_path_buf())),
            _ => None,
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    // Unit tests will be added in TDD red phase
}
