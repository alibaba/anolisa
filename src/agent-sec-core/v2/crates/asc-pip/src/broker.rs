//! ContextBroker: collects attribute requirements per rule set, fetches
//! providers concurrently, caches by freshness, and aggregates missing
//! attributes into the verdict. Inline context (per-request, never stored)
//! is folded in with assurance fixed at P1 (§7.3).
