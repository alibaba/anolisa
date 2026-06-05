use std::collections::BTreeMap;

use crate::types::{CommandBlock, CommandStatus, OutputRefs, ShellEvent, ShellEventKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerOutput {
    pub blocks: Vec<CommandBlock>,
    pub errors: Vec<String>,
}

pub fn build_command_blocks(events: &[ShellEvent]) -> LedgerOutput {
    let mut starts = BTreeMap::new();
    let mut blocks = Vec::new();
    let mut errors = Vec::new();

    for event in events {
        match &event.kind {
            ShellEventKind::CommandStarted => {
                if let Some(command_id) = &event.command_id {
                    starts.insert(command_id.clone(), event.clone());
                } else {
                    errors.push("command_started_missing_id".to_string());
                }
            }
            ShellEventKind::CommandCompleted | ShellEventKind::CommandFailed => {
                let Some(command_id) = &event.command_id else {
                    errors.push("command_finished_missing_id".to_string());
                    continue;
                };

                let Some(start) = starts.remove(command_id) else {
                    errors.push(format!("command_finished_without_start:{command_id}"));
                    continue;
                };

                let started_at_ms = start.started_at_ms.unwrap_or(0);
                let ended_at_ms = event.ended_at_ms.unwrap_or(started_at_ms);
                let duration_ms = event
                    .duration_ms
                    .unwrap_or_else(|| ended_at_ms.saturating_sub(started_at_ms));
                let exit_code = event.exit_code.unwrap_or(0);
                let status =
                    if matches!(&event.kind, ShellEventKind::CommandFailed) || exit_code != 0 {
                        CommandStatus::Failed
                    } else {
                        CommandStatus::Completed
                    };

                blocks.push(CommandBlock {
                    id: command_id.clone(),
                    session_id: event.session_id.clone(),
                    command: start.command.unwrap_or_default(),
                    cwd: start.cwd.clone().unwrap_or_default(),
                    end_cwd: event
                        .end_cwd
                        .clone()
                        .or_else(|| event.cwd.clone())
                        .or(start.cwd)
                        .unwrap_or_default(),
                    started_at_ms,
                    ended_at_ms,
                    duration_ms,
                    exit_code,
                    status,
                    output: OutputRefs {
                        terminal_output_ref: event.terminal_output_ref.clone(),
                        terminal_output_bytes: event.terminal_output_bytes.unwrap_or(0),
                    },
                });
            }
            _ => {}
        }
    }

    for command_id in starts.keys() {
        errors.push(format!("command_started_without_finish:{command_id}"));
    }

    LedgerOutput { blocks, errors }
}
