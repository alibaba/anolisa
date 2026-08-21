// SPDX-License-Identifier: MIT
// Copyright (c) 2026 eunomia-bpf org.
//! ActPlane taint DSL compiler: parse the DSL (docs/rule-language.md) and lower it
//! to the kernel ABI (struct taint_config) the loader installs into BPF rodata.

pub mod ast;
pub mod lower;
pub mod parse;

pub use lower::{Compiled, RuleMeta, compile};

/// Parse + compile DSL source text to a kernel config blob + reason table.
pub fn compile_str(src: &str) -> Result<Compiled, String> {
    compile(&parse::parse(src)?)
}
