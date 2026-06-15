//! ATIF (Agent Trajectory Interchange Format) module
//!
//! Provides ATIF v1.6 data structures and conversion logic for exporting
//! AgentSight GenAI data to the standardized trajectory format.
//!
//! This module is independent from the `genai` module — it only depends on
//! storage query result types and `genai::semantic` types for deserialization.

pub mod converter;
pub mod schema;

pub use converter::{convert_session_to_atif, convert_trace_to_atif};
pub use schema::{
    AtifAgent, AtifDocument, AtifFinalMetrics, AtifObservation, AtifObservationResult, AtifStep,
    AtifStepMetrics, AtifToolCall, SCHEMA_VERSION,
};
