//! Policy decision point (design doc §6): stateless decision kernel,
//! incremental trace automaton executor, stateful adjudicator plugin slots
//! (Intent Firewall mount point) and the merge stage.
//!
//! Hot-path constraints: no global locks (policy revision swaps via
//! `ArcSwap`), per-session serialized queues for `observe`/`decide`, no
//! `await` across Tier B evaluation, and any panic at the entry boundary is
//! converted to Deny (hard domains) or audit downgrade (advisory domains).

#![forbid(unsafe_code)]

pub mod adjudicator;
pub mod automaton;
pub mod cache;
pub mod engine;
