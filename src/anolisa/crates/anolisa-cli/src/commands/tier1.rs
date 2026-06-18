//! Primary commands — component lifecycle and operations.

pub mod adopt;
pub mod bug;
pub mod doctor;
pub mod env;
pub mod forget;
pub mod install;
pub mod list;
pub mod logs;
pub mod repair;
pub mod restart;
pub mod status;
pub mod uninstall;
pub mod update;

// Cross-command end-to-end MVP lifecycle coverage (#963); test-only.
#[cfg(test)]
mod mvp_lifecycle;
