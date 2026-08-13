//! P0 structured rule frontend (design doc §4.2-§4.3): compiles PolicyRule
//! and CapabilityProfile documents into asc-policy-ir with jsonschema
//! validation and conflict detection. Compilation happens only inside the
//! PAP activation transaction, never on the decision hot path.
//!
//! Expressiveness is deliberately restricted: closed operator set, closed
//! attribute namespaces, no cross-event sequences (those belong to the P1
//! trace DSL). Compile errors carry span locations.

#![forbid(unsafe_code)]

pub mod compile;
pub mod schema;
