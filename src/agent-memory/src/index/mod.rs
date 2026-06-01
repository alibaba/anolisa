//! Phase 4: Index Worker + Tier B structured search.
//!
//! - `BM25Store`: SQLite FTS5 wrapper, the only place that touches the DB
//! - `IndexWorker`: notify-driven background task that keeps the store in
//!   sync with the on-disk mount tree
//! - `IndexHandle`: thread-safe entry point handed to MemoryService /
//!   Tier B tools. Drop = stop worker + close DB.

pub mod extractor;
pub mod store;
pub mod worker;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::embedding::EmbeddingProvider;
use crate::error::Result;
use crate::ns::MountPoint;

pub use store::BM25Store;
pub use worker::IndexWorker;

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub path: String,
    pub snippet: String,
    pub score: f64,
    /// Whether the snippet contains prompt-injection patterns.  Callers
    /// in the adapter layer can use this flag to decide whether to
    /// surface the hit, surface it with extra isolation, or suppress it
    /// entirely.
    #[serde(default)]
    pub suspicious: bool,
}

/// Owning handle: spawn an IndexWorker that watches `mount`, expose
/// thread-safe search via the embedded BM25Store. Dropping the handle
/// shuts down the worker and closes the DB.
pub struct IndexHandle {
    store: Arc<Mutex<BM25Store>>,
    worker: Option<IndexWorker>,
    db_path: PathBuf,
    pub embedding: Option<Arc<dyn EmbeddingProvider>>,
}

impl IndexHandle {
    pub fn open(mount: &MountPoint, embedding: Option<Arc<dyn EmbeddingProvider>>) -> Result<Self> {
        let db_path = mount.meta_dir.join("index").join("bm25.db");
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let store = BM25Store::open(&db_path)?;
        let store = Arc::new(Mutex::new(store));

        // Initial full scan + watcher in one worker.
        let emb_clone = embedding.clone();
        let worker = IndexWorker::spawn(mount.clone_lite(), Arc::clone(&store), emb_clone)?;

        Ok(Self {
            store,
            worker: Some(worker),
            db_path,
            embedding,
        })
    }

    pub fn search(&self, query: &str, top_k: usize) -> Result<Vec<SearchHit>> {
        let store = self.store.lock().expect("index store poisoned");
        store.search(query, top_k)
    }

    pub fn search_vec(&self, query_vec: &[f32], top_k: usize) -> Result<Vec<SearchHit>> {
        let store = self.store.lock().expect("index store poisoned");
        let raw = store.search_vec(query_vec, top_k)?;
        Ok(raw
            .into_iter()
            .map(|(path, score)| SearchHit {
                path,
                snippet: String::new(),
                score,
                suspicious: false,
            })
            .collect())
    }

    pub fn search_hybrid(
        &self,
        query: &str,
        query_vec: &[f32],
        top_k: usize,
    ) -> Result<Vec<SearchHit>> {
        let store = self.store.lock().expect("index store poisoned");
        store.search_hybrid(query, query_vec, top_k)
    }

    pub fn db_path(&self) -> &std::path::Path {
        &self.db_path
    }

    pub fn count(&self) -> Result<usize> {
        let store = self.store.lock().expect("index store poisoned");
        store.count()
    }

    /// Synchronously wait until at least `expected_min` files are indexed,
    /// up to `deadline_ms` milliseconds. Test helper — production callers
    /// should not need this since search is best-effort eventually-consistent.
    #[doc(hidden)]
    pub fn wait_until_at_least(&self, expected_min: usize, deadline_ms: u64) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed().as_millis() < deadline_ms as u128 {
            if let Ok(n) = self.count() {
                if n >= expected_min {
                    return true;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }
}

impl Drop for IndexHandle {
    fn drop(&mut self) {
        if let Some(w) = self.worker.take() {
            w.shutdown_blocking();
        }
    }
}
