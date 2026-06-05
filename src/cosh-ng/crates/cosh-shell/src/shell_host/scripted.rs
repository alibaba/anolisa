use std::fs::File;
use std::io::{self, BufRead, Write};
use std::process::Child;
use std::time::Duration;

use crate::input::InputDecision;

use super::bootstrap::{start_bash_session, start_zsh_session, PtySession};
use super::io_loop::{read_until, read_until_streaming, wait_child};
use super::lifecycle::finish_shell_host_output;
use super::model::{ScriptedInput, ShellHostConfig, ShellHostOutput};
use super::osc::OscParser;

pub fn run_scripted_bash(
    config: &ShellHostConfig,
    inputs: &[ScriptedInput],
) -> io::Result<ShellHostOutput> {
    run_scripted_shell(config, inputs, start_bash_session)
}

pub fn run_scripted_zsh(
    config: &ShellHostConfig,
    inputs: &[ScriptedInput],
) -> io::Result<ShellHostOutput> {
    run_scripted_shell(config, inputs, start_zsh_session)
}

fn run_scripted_shell(
    config: &ShellHostConfig,
    inputs: &[ScriptedInput],
    start_session: fn(&ShellHostConfig) -> io::Result<PtySession>,
) -> io::Result<ShellHostOutput> {
    let mut session = start_session(config)?;

    read_until(
        &mut session.master,
        &mut session.child,
        &mut session.parser,
        Duration::from_secs(5),
        |parser| parser.precmd_count() >= 1,
    )?;

    for input in inputs {
        match input {
            ScriptedInput::Command(command) => {
                send_command_line(
                    &mut session.master,
                    &mut session.child,
                    &mut session.parser,
                    &config.prompt,
                    command,
                )?;
            }
            ScriptedInput::UserLine(input) => match config.input_classifier.classify(input) {
                InputDecision::SendToShell(command) => {
                    send_command_line(
                        &mut session.master,
                        &mut session.child,
                        &mut session.parser,
                        &config.prompt,
                        &command,
                    )?;
                }
                InputDecision::Intercept { input, reason } => {
                    session.parser.push_intercept_event(
                        &config.session_id,
                        input,
                        None,
                        reason.as_str(),
                    );
                }
                InputDecision::Consume => {}
            },
            ScriptedInput::Intercept { input, reason } => {
                session.parser.push_intercept_event(
                    &config.session_id,
                    input.clone(),
                    None,
                    reason,
                );
            }
        }
    }

    session.master.write_all(b"exit\n")?;
    session.master.flush()?;
    read_until(
        &mut session.master,
        &mut session.child,
        &mut session.parser,
        Duration::from_millis(300),
        |_| false,
    )?;
    session.parser.flush_pending();
    let exit_status = wait_child(&mut session.child)?;
    finish_shell_host_output(config, session.parser, exit_status)
}

pub fn run_streaming_line_bash<R, W>(
    config: &ShellHostConfig,
    mut input: R,
    mut output: W,
) -> io::Result<ShellHostOutput>
where
    R: BufRead,
    W: Write,
{
    let mut session = start_bash_session(config)?;

    read_until_streaming(
        &mut session.master,
        &mut session.child,
        &mut session.parser,
        &mut output,
        Duration::from_secs(5),
        |parser| parser.precmd_count() >= 1,
    )?;

    let mut line = String::new();
    loop {
        line.clear();
        let bytes = input.read_line(&mut line)?;
        if bytes == 0 {
            break;
        }

        let user_line = line.trim_end_matches(['\r', '\n']).to_string();
        if user_line.is_empty() {
            continue;
        }

        match config.input_classifier.classify(&user_line) {
            InputDecision::SendToShell(command) => send_command_line_streaming(
                &mut session.master,
                &mut session.child,
                &mut session.parser,
                &mut output,
                &config.prompt,
                &command,
            )?,
            InputDecision::Intercept { input, reason } => {
                session.parser.push_intercept_event(
                    &config.session_id,
                    input,
                    None,
                    reason.as_str(),
                );
            }
            InputDecision::Consume => {}
        }
    }

    session.master.write_all(b"exit\n")?;
    session.master.flush()?;
    read_until_streaming(
        &mut session.master,
        &mut session.child,
        &mut session.parser,
        &mut output,
        Duration::from_millis(300),
        |_| false,
    )?;
    let display_start = session.parser.display.len();
    session.parser.flush_pending();
    output.write_all(&session.parser.display[display_start..])?;
    output.flush()?;

    let exit_status = wait_child(&mut session.child)?;
    finish_shell_host_output(config, session.parser, exit_status)
}

fn send_command_line(
    master: &mut File,
    child: &mut Child,
    parser: &mut OscParser,
    _prompt: &str,
    command: &str,
) -> io::Result<()> {
    let target_precommands = parser.precmd_count() + 1;
    master.write_all(command.as_bytes())?;
    master.write_all(b"\n")?;
    master.flush()?;
    read_until(master, child, parser, Duration::from_secs(5), |parser| {
        parser.precmd_count() >= target_precommands
    })?;
    Ok(())
}

fn send_command_line_streaming<W: Write>(
    master: &mut File,
    child: &mut Child,
    parser: &mut OscParser,
    output: &mut W,
    _prompt: &str,
    command: &str,
) -> io::Result<()> {
    let target_precommands = parser.precmd_count() + 1;
    master.write_all(command.as_bytes())?;
    master.write_all(b"\n")?;
    master.flush()?;
    read_until_streaming(
        master,
        child,
        parser,
        output,
        Duration::from_secs(5),
        |parser| parser.precmd_count() >= target_precommands,
    )?;
    Ok(())
}
