//! Incremental trace automaton executor (§6.3): per-session partitioned,
//! bounded instances (default 256) with TTL windows. Backpressure splits by
//! accepting effect: advisory automata degrade to audit, deny-effect automata
//! fail closed — resource exhaustion never silently drops deny semantics.
