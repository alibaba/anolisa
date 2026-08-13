//! HMAC `TokenAuthority` implementation (§8.3): ResumeToken issue/verify
//! with digest, revision, TTL and one-shot nonce checks. The MAC key stays
//! inside this module; the nonce consumption ledger and pending approvals
//! persist in rusqlite (WAL) so one-shot semantics survive restarts.
