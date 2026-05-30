//! Core component trait and metadata types.

use anolisa_env::EnvFacts;
use std::collections::HashMap;

/// Metadata describing an ANOLISA component.
#[derive(Debug, Clone)]
pub struct ComponentMeta {
    pub name: String,
    pub version: String,
    pub layer: Layer,
    pub domain: Domain,
    pub description: String,
}

/// Architecture layer a component belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layer {
    Osbase,
    Runtime,
    Encapsulation,
}

/// Capability domain within the runtime layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Domain {
    Tools,
    State,
    Cost,
    Security,
    Observability,
}

/// Feature definition for a component.
#[derive(Debug, Clone)]
pub struct FeatureDef {
    pub name: String,
    pub label: String,
    pub default: bool,
    pub requires_env: HashMap<String, String>,
    pub conflicts_with: Vec<String>,
}

/// Health/status of a component.
#[derive(Debug, Clone)]
pub enum ComponentStatus {
    Ok,
    Degraded { reason: String },
    Stopped,
    NotInstalled,
    Error { message: String },
}

/// Pre-check result for environment compatibility.
#[derive(Debug)]
pub enum PreCheckResult {
    Compatible,
    Partial { reason: String, advice: String },
    Incompatible { reason: String, advice: String },
}

/// The core trait every installable component implements.
pub trait Component {
    fn metadata(&self) -> &ComponentMeta;
    fn check_env(&self, facts: &EnvFacts) -> PreCheckResult;
    fn features(&self) -> &[FeatureDef];
    fn status(&self) -> ComponentStatus;
}
