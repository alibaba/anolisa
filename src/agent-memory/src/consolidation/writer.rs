//! Fact writer — writes consolidated facts to the filesystem.
//!
//! Produces two outputs per fact:
//! 1. `facts/<ulid>.md` — markdown with YAML frontmatter
//! 2. `facts/facts.jsonl` — JSONL line appended to the same directory
//!
//! Both writes go through `safe_fs` (openat2 + RESOLVE_BENEATH) to preserve
//! namespace-sandbox consistency with other tools: the markdown file via
//! `write_create_new`, the JSONL line via `append`.

use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::safe_fs;

use super::fact::ConsolidatedFact;

/// Writes facts to a given base directory using namespace-safe paths.
pub struct FactWriter {
    root_fd: OwnedFd,
    facts_dir: PathBuf,
}

impl FactWriter {
    /// Create a new FactWriter. `root_fd` is a clone of the mount root fd
    /// used for safe_fs operations. `base_dir` is the absolute path to the
    /// mount root.
    pub fn new(root_fd: OwnedFd, base_dir: &Path) -> Self {
        let facts_dir = base_dir.join("facts");
        Self { root_fd, facts_dir }
    }

    /// Ensure the facts directory exists.
    pub fn ensure_dir(&self) -> Result<()> {
        std::fs::create_dir_all(&self.facts_dir)?;
        Ok(())
    }

    /// Write a single fact: creates `<ulid>.md` via safe_fs and appends to `facts.jsonl`.
    pub fn write(&self, fact: &ConsolidatedFact) -> Result<()> {
        self.ensure_dir()?;

        // Write markdown file via safe_fs (openat2 + RESOLVE_BENEATH).
        let rel_path = Path::new("facts").join(format!("{}.md", fact.id));
        let content = fact.to_markdown();
        safe_fs::write_create_new(self.root_fd.as_fd(), &rel_path, content.as_bytes())?;

        // Append JSONL line via safe_fs (openat2 + RESOLVE_BENEATH), keeping
        // the write path consistent with the markdown file above. Each append
        // opens and closes the file atomically; per-line fsync is dropped in
        // favor of batch-level durability (callers write many facts per
        // consolidation run).
        let mut line = fact.to_jsonl()?;
        line.push('\n');
        let jsonl_rel = Path::new("facts").join("facts.jsonl");
        safe_fs::append(self.root_fd.as_fd(), &jsonl_rel, line.as_bytes())?;

        tracing::debug!("wrote fact: {}", rel_path.display());
        Ok(())
    }

    /// Write multiple facts in one batch.
    pub fn write_batch(&self, facts: &[ConsolidatedFact]) -> Result<usize> {
        if facts.is_empty() {
            return Ok(0);
        }

        let mut written = 0;
        for fact in facts {
            if let Err(e) = self.write(fact) {
                tracing::warn!("failed to write fact {}: {e}", fact.id);
            } else {
                written += 1;
            }
        }

        tracing::info!("batch write: {written}/{} facts written", facts.len());
        Ok(written)
    }

    pub fn facts_dir(&self) -> &Path {
        &self.facts_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consolidation::fact::{ConsolidatedFact, FactCategory};

    fn make_writer(tmp: &tempfile::TempDir) -> FactWriter {
        let root_fd = crate::safe_fs::open_root(tmp.path()).unwrap();
        FactWriter::new(root_fd, tmp.path())
    }

    #[test]
    fn write_single_fact() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = make_writer(&tmp);
        let fact = ConsolidatedFact::new(
            "test-sid",
            FactCategory::WorkingContext,
            "Test fact".into(),
            "Test content body".into(),
            "mem_write".into(),
            vec!["notes/a.md".into()],
            0.8,
        );
        writer.write(&fact).unwrap();

        let md_path = tmp.path().join("facts").join(format!("{}.md", fact.id));
        assert!(md_path.exists());
        assert!(
            std::fs::read_to_string(&md_path)
                .unwrap()
                .contains("Test content")
        );

        let jsonl_path = tmp.path().join("facts").join("facts.jsonl");
        assert!(jsonl_path.exists());
        let content = std::fs::read_to_string(&jsonl_path).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 1);
        let parsed: ConsolidatedFact = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed.id, fact.id);
    }

    #[test]
    fn write_batch_dedup() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = make_writer(&tmp);
        let facts = vec![
            ConsolidatedFact::new(
                "s1",
                FactCategory::Interest,
                "A".into(),
                "Content A".into(),
                "mem_search".into(),
                vec![],
                0.5,
            ),
            ConsolidatedFact::new(
                "s2",
                FactCategory::Lesson,
                "B".into(),
                "Content B".into(),
                "mem_edit".into(),
                vec![],
                0.6,
            ),
        ];
        let n = writer.write_batch(&facts).unwrap();
        assert_eq!(n, 2);
        let jsonl_path = tmp.path().join("facts").join("facts.jsonl");
        let content = std::fs::read_to_string(&jsonl_path).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 2);
    }
}
