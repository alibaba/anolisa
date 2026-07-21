pub(super) mod commands;
pub(super) mod config;
pub(super) mod debug;
pub(super) mod extensions;
#[cfg(test)]
mod extensions_tests;
pub(super) mod hooks;
#[cfg(test)]
mod hooks_tests;
pub(super) mod notices;
pub(super) mod panel;
pub(super) mod parser;
pub(super) mod prompt;
pub(super) mod recommendations;
pub(crate) mod registry;
pub(super) mod runtime;
pub(super) mod session;
pub(super) mod skills;
#[cfg(test)]
mod skills_tests;
