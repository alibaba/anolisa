//! Platform-facing helpers for ANOLISA install layout and OS integration.
//!
//! This crate stays below the CLI/core orchestration layers: it resolves
//! filesystem roots and provides thin bridges to host package/service
//! managers without importing CLI vocabulary.

pub mod command;
pub mod fs_layout;
pub mod package_manager;
pub mod pkg_query;
pub mod privilege;
pub mod rpm_query;
pub mod systemd;
