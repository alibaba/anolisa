#[path = "display.rs"]
mod display;

#[path = "governance.rs"]
mod governance;

#[cfg(test)]
#[path = "governance_tests.rs"]
mod governance_tests;

pub use governance::{govern_agent_events, govern_agent_events_with_language, GovernanceOutput};
