use crate::runtime::mode::render_mode_command;
use crate::runtime::prelude::*;
use crate::slash::audit::render_audit_command;
use crate::slash::config::render_config_command;
use crate::slash::debug::render_debug_command;
use crate::slash::extensions::render_extensions_command;
use crate::slash::health::render_health_command;
use crate::slash::hooks::render_hooks_command;
use crate::slash::notices::{
    render_help, render_hint, render_info, render_removed_command, render_unknown,
};
use crate::slash::parser::SlashCommand;
use crate::slash::recommendations::render_recommendations_command;
use crate::slash::session::render_session_command;
use crate::slash::skills::render_skills_command;

pub(super) fn render_slash_command<W: Write>(
    command: SlashCommand<'_>,
    event: &ShellEvent,
    blocks: &[CommandBlock],
    adapter: &AdapterInstance,
    state: &mut InlineState,
    shell_cwd: Option<&str>,
    output: &mut W,
) -> std::io::Result<bool> {
    match command {
        SlashCommand::Noop => Ok(true),
        SlashCommand::Auth => {
            crate::auth::runtime::trigger_auth_from_slash(adapter, state, output)?;
            Ok(false)
        }
        SlashCommand::Audit(arguments) => {
            render_audit_command(arguments, state, output)?;
            Ok(true)
        }
        SlashCommand::Help => {
            render_help(state, output)?;
            Ok(true)
        }
        SlashCommand::Hooks(sub, arg, extra) => {
            render_hooks_command(sub, arg, extra, blocks, adapter, state, output)?;
            Ok(true)
        }
        SlashCommand::Mode(arg, sub, confirm) => {
            render_mode_command(arg, sub, confirm, state, output)
        }
        SlashCommand::Config(sub, value) => render_config_command(sub, value, state, output),
        SlashCommand::Debug(sub) => {
            render_debug_command(sub, adapter, state, output)?;
            Ok(true)
        }
        SlashCommand::Info(command) => {
            render_info(command, state, output)?;
            Ok(true)
        }
        SlashCommand::Removed(command) => {
            render_removed_command(command, state, output)?;
            Ok(true)
        }
        SlashCommand::Hint(prefix) => {
            render_hint(prefix, state, output)?;
            Ok(true)
        }
        SlashCommand::Unknown(command) => {
            render_unknown(command, state, output)?;
            Ok(true)
        }
        SlashCommand::Extensions(sub, arg) => {
            render_extensions_command(sub, arg, adapter, state, output)?;
            Ok(true)
        }
        SlashCommand::Skills(sub, arg) => {
            render_skills_command(sub, arg, adapter, state, output)?;
            Ok(true)
        }
        SlashCommand::Session(arguments) => {
            render_session_command(arguments, blocks, adapter, state, output)
        }
        SlashCommand::Recommendations(sub, arg, extra) => {
            render_recommendations_command(sub, arg, extra, event, adapter, state, output)?;
            Ok(true)
        }
        SlashCommand::Health => {
            render_health_command(state, shell_cwd, output)?;
            Ok(true)
        }
    }
}
