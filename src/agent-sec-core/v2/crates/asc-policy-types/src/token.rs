//! `TokenAuthority` trait and `ResumeToken` for the STEP_UP/DEFER closed
//! loop (§8.3). The HMAC implementation lives in asc-pep; the MAC key is held
//! inside the implementation only and never crosses this boundary.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::primitives::{DecisionId, Digest, PolicyRevision, Timestamp};

/// One-shot resumption credential for STEP_UP/DEFER (§8.3). Binding rules:
/// pre-action intervention points bind `request_digest` to the action
/// argument digest (anti replay-with-swapped-args); `OsEventAsync` binds it
/// to the triggering event id (unfreeze authorization semantics).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeToken {
    /// Decision the token resumes.
    pub decision_id: DecisionId,
    /// Digest binding the original request; re-submission must match.
    pub request_digest: Digest,
    /// Revision at issuance; any policy change invalidates the token and
    /// forces re-adjudication.
    pub policy_revision: PolicyRevision,
    /// Expiry; expired tokens are rejected.
    pub expires_at: Timestamp,
    /// One-shot nonce; consumption is recorded transactionally (§8.3).
    pub nonce: [u8; 16],
    /// HMAC over all preceding fields; key held by the `TokenAuthority`
    /// implementation only, unforgeable by hook or agent.
    pub mac: [u8; 32],
}

/// Inputs for issuing a [`ResumeToken`] during obligation assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenIssueRequest {
    /// Decision the token will resume.
    pub decision_id: DecisionId,
    /// Digest binding the original request (see [`ResumeToken`]).
    pub request_digest: Digest,
    /// Revision the token is pinned to.
    pub policy_revision: PolicyRevision,
    /// Validity window from issuance.
    pub ttl: Duration,
}

/// Token verification failures. Any failure keeps the deny-biased baseline:
/// the pending action stays denied.
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    /// Token expired.
    #[error("resume token expired")]
    Expired,
    /// Re-submitted request digest does not match the bound one.
    #[error("request digest mismatch: token is bound to a different request")]
    DigestMismatch,
    /// Policy revision changed since issuance; re-adjudication required.
    #[error("policy revision mismatch: token issued under a stale revision")]
    RevisionMismatch,
    /// MAC verification failed.
    #[error("resume token MAC invalid")]
    MacInvalid,
    /// Nonce already consumed (one-shot semantics).
    #[error("resume token nonce already consumed")]
    NonceConsumed,
    /// Persistence failure in the nonce/approval ledger.
    #[error("token ledger storage failure: {0}")]
    Storage(String),
}

/// Issues and verifies [`ResumeToken`]s (§8.3). Implementations own the MAC
/// key and the persistent nonce-consumption ledger; verification and nonce
/// consumption must be atomic so one-shot semantics survive restarts.
pub trait TokenAuthority: Send + Sync {
    /// Issues a token bound to the given request.
    ///
    /// # Errors
    /// Returns [`TokenError::Storage`] when the pending-approval ledger
    /// cannot be written.
    fn issue(&self, request: &TokenIssueRequest) -> Result<ResumeToken, TokenError>;

    /// Verifies a token against the re-submitted request digest and the
    /// currently active revision, consuming its nonce on success.
    ///
    /// # Errors
    /// Returns the specific [`TokenError`] for digest/revision/TTL/MAC/nonce
    /// failures; all failures leave the action denied.
    fn verify(
        &self,
        token: &ResumeToken,
        request_digest: &Digest,
        active_revision: &PolicyRevision,
    ) -> Result<(), TokenError>;
}
