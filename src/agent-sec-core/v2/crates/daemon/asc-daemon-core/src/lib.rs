//! Transport-independent daemon application capabilities.

#![forbid(unsafe_code)]

pub mod policy;

pub use policy::{PolicyError, PolicyService};
