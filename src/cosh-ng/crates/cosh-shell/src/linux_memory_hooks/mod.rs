use crate::hook_engine::BuiltinHook;
use crate::hook_types::*;

mod parser;
mod presentation;

use parser::*;
use presentation::{
    high_memory_finding, interactive_top_guidance_finding, memory_pressure_finding,
};

#[derive(Debug, Clone, PartialEq)]
struct ProcessMemoryRow {
    pid: String,
    command: String,
    mem_pct: f64,
    rss_kib: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
struct MemoryMetrics {
    total_mib: f64,
    available_mib: f64,
    swap_total_mib: Option<f64>,
    swap_used_mib: Option<f64>,
    confidence: MetricsConfidence,
}

#[derive(Debug, Clone, Copy)]
struct MemoryUnitFactors {
    no_suffix: f64,
    bare_suffix_mib: f64,
    bare_suffix_step: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MetricsConfidence {
    Low,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnSemantic {
    Pid,
    MemPct,
    Rss,
    Res,
    Command,
    Total,
    Available,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ColumnBinding {
    semantic: ColumnSemantic,
    index: usize,
    raw_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TableSchema {
    columns: Vec<ColumnBinding>,
}

#[derive(Debug, Clone, Copy)]
struct ColumnSpec<'a> {
    semantic: ColumnSemantic,
    aliases: &'a [&'a str],
    prefer_last: bool,
}

pub struct HighMemoryProcessHook {
    matcher: HookMatcher,
}

impl Default for HighMemoryProcessHook {
    fn default() -> Self {
        Self::new()
    }
}

impl HighMemoryProcessHook {
    pub fn new() -> Self {
        Self {
            matcher: HookMatcher {
                id: "high-memory-process".into(),
                commands: vec!["ps".into(), "top".into(), "env".into()],
                command_patterns: Vec::new(),
                command_regex: None,
                exit_codes: Some(vec![0]),
                min_output_bytes: Some(1),
                trigger: HookTrigger::OnSuccess,
            },
        }
    }
}

impl BuiltinHook for HighMemoryProcessHook {
    fn id(&self) -> &str {
        "high-memory-process"
    }

    fn matcher(&self) -> &HookMatcher {
        &self.matcher
    }

    fn evaluate(&self, input: &HookInput) -> Option<HookFinding> {
        let program = memory_target_program(&input.command);
        let rows = if program == "top" {
            if !is_batch_top_command(&input.command) {
                return None;
            }
            parse_top_process_rows(&input.output_preview)
        } else if program == "ps" {
            parse_ps_process_rows(&input.output_preview)
        } else {
            return None;
        };
        high_memory_finding(&rows)
    }
}

pub struct MemoryPressureHook {
    matcher: HookMatcher,
}

impl Default for MemoryPressureHook {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryPressureHook {
    pub fn new() -> Self {
        Self {
            matcher: HookMatcher {
                id: "memory-pressure".into(),
                commands: vec!["free".into(), "top".into(), "env".into()],
                command_patterns: Vec::new(),
                command_regex: None,
                exit_codes: Some(vec![0]),
                min_output_bytes: Some(1),
                trigger: HookTrigger::OnSuccess,
            },
        }
    }
}

impl BuiltinHook for MemoryPressureHook {
    fn id(&self) -> &str {
        "memory-pressure"
    }

    fn matcher(&self) -> &HookMatcher {
        &self.matcher
    }

    fn evaluate(&self, input: &HookInput) -> Option<HookFinding> {
        let program = memory_target_program(&input.command);
        let metrics = if program == "top" {
            if !is_batch_top_command(&input.command) {
                return None;
            }
            parse_top_memory_metrics(&input.output_preview)
        } else if program == "free" {
            if is_free_sampling_command(&input.command) {
                return None;
            }
            parse_free_memory_metrics(&input.command, &input.output_preview)
        } else {
            return None;
        };
        memory_pressure_finding(metrics.as_ref())
    }
}

pub struct InteractiveTopGuidanceHook {
    matcher: HookMatcher,
}

impl Default for InteractiveTopGuidanceHook {
    fn default() -> Self {
        Self::new()
    }
}

impl InteractiveTopGuidanceHook {
    pub fn new() -> Self {
        Self {
            matcher: HookMatcher {
                id: "interactive-top-guidance".into(),
                commands: vec!["top".into(), "env".into()],
                command_patterns: Vec::new(),
                command_regex: None,
                exit_codes: Some(vec![0]),
                min_output_bytes: Some(1),
                trigger: HookTrigger::OnSuccess,
            },
        }
    }
}

impl BuiltinHook for InteractiveTopGuidanceHook {
    fn id(&self) -> &str {
        "interactive-top-guidance"
    }

    fn matcher(&self) -> &HookMatcher {
        &self.matcher
    }

    fn evaluate(&self, input: &HookInput) -> Option<HookFinding> {
        if memory_target_program(&input.command) != "top"
            || is_batch_top_command(&input.command)
            || is_top_metadata_command(&input.command)
        {
            return None;
        }
        Some(interactive_top_guidance_finding())
    }
}

fn max_severity(
    left: Option<FindingSeverity>,
    right: Option<FindingSeverity>,
) -> Option<FindingSeverity> {
    match (left, right) {
        (Some(FindingSeverity::Critical), _) | (_, Some(FindingSeverity::Critical)) => {
            Some(FindingSeverity::Critical)
        }
        (Some(FindingSeverity::Warning), _) | (_, Some(FindingSeverity::Warning)) => {
            Some(FindingSeverity::Warning)
        }
        (Some(FindingSeverity::Info), _) | (_, Some(FindingSeverity::Info)) => {
            Some(FindingSeverity::Info)
        }
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests;
