#[path = "display.rs"]
mod display;

#[path = "governance.rs"]
mod governance;

pub use governance::{govern_agent_events, govern_agent_events_with_language, GovernanceOutput};
