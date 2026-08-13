//! Per-record hash chain and tail verification on daemon restart; P1 adds
//! signed checkpoints as transparency-log anchors (§9.5).

/// Rolling hash-chain state over an append-only record stream. Each record
/// carries the previous chain hash as `prev_record_hash`; the daemon
/// verifies the chain tail on restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashChain {
    last: [u8; 32],
}

impl HashChain {
    /// Starts a chain from a seed; all zeros for a fresh log file.
    pub fn new(seed: [u8; 32]) -> Self {
        Self { last: seed }
    }

    /// Hash of the most recently linked record.
    pub fn last(&self) -> [u8; 32] {
        self.last
    }

    /// Links a serialized record into the chain and returns the new chain
    /// hash, which the next record must carry as `prev_record_hash`.
    pub fn advance(&mut self, record_bytes: &[u8]) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&self.last);
        hasher.update(record_bytes);
        self.last = *hasher.finalize().as_bytes();
        self.last
    }
}
