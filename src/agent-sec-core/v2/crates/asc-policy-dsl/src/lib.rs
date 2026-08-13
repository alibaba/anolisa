//! P1 trace DSL frontend (design doc §4.4): Invariant-compat trajectory
//! policies compiled to trace automaton IR. Temporal (`precedes`) and
//! provenance (`caused_by`/`carries_taint`/...) operators are distinct;
//! rules with only temporal relations may at most drive audit/step_up
//! (compiler enforced), deny requires a provenance relation meeting its
//! assurance floor.
//!
//! Parser is handwritten recursive descent (no generator dependency, span
//! locations and error recovery under our control) and is a fuzz target.

#![forbid(unsafe_code)]

pub mod lower;
pub mod parser;
pub mod typecheck;
