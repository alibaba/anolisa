// SPDX-License-Identifier: MIT
//! ActPlane IFC (Information-Flow Control) policy compiler.
//!
//! Parses the ActPlane taint DSL and compiles it to the kernel ABI
//! (struct taint_config) for BPF rodata installation.

pub mod dsl;

pub use dsl::{Compiled, RuleMeta, compile, compile_str};

/// Compiled policy blob size in bytes — ABI contract with ebpf-ifc-engine.
pub const COMPILED_CONFIG_BLOB_SIZE: usize = std::mem::size_of::<dsl::lower::CConfig>();
