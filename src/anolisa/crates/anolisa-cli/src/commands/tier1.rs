//! Primary commands — component lifecycle and operations.

pub mod adopt;
pub mod bug;
pub mod doctor;
pub mod env;
pub mod forget;
pub mod install;
pub mod list;
pub mod logs;
pub(crate) mod recovery;
pub mod repair;
pub mod restart;
pub(crate) mod rpm_install;
pub mod status;
pub mod uninstall;
pub mod update;
pub mod upgrade;
