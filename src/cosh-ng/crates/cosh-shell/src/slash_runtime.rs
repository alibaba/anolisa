use super::mode_runtime::{render_mode_card_actions, render_mode_command};
use super::*;

pub(super) fn render_slash_actions<W: Write>(
    events: &[ShellEvent],
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    render_mode_card_actions(events, state, output)?;

    for (idx, event) in events.iter().enumerate() {
        let Some(input) = slash_input(event) else {
            continue;
        };
        let Some(command) = SlashCommand::parse(input) else {
            continue;
        };

        let key = stable_event_key("slash", idx, event);
        if !state.handled_slash_commands.insert(key) {
            continue;
        }

        clear_shell_prompt_line(output)?;
        let restore_prompt = match command {
            SlashCommand::Noop => true,
            SlashCommand::Help => {
                render_help(state, output)?;
                true
            }
            SlashCommand::Hooks(sub, arg) => {
                render_hooks_command(sub, arg, state, output)?;
                true
            }
            SlashCommand::Mode(arg, sub) => render_mode_command(arg, sub, state, output)?,
            SlashCommand::Info(command) => {
                render_info(command, output)?;
                true
            }
            SlashCommand::Hint(prefix) => {
                render_hint(prefix, state, output)?;
                true
            }
            SlashCommand::Unknown(command) => {
                render_unknown(command, output)?;
                true
            }
        };
        if restore_prompt {
            write_shell_prompt(state, output)?;
        }
        output.flush()?;
    }

    Ok(())
}

fn slash_input(event: &ShellEvent) -> Option<&str> {
    if event.kind != ShellEventKind::UserInputIntercepted {
        return None;
    }
    if event.component.as_deref() != Some("slash") {
        return None;
    }
    event.input.as_deref()
}

pub(super) fn write_shell_prompt<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    if std::env::var("COSH_SHELL_ISOLATED").is_ok() {
        writeln!(output)?;
        write!(output, "cosh-osc$ ")
    } else {
        state.trigger_pty_prompt = true;
        Ok(())
    }
}

fn clear_shell_prompt_line<W: Write>(output: &mut W) -> std::io::Result<()> {
    write!(output, "\r\x1b[2K")
}

enum SlashCommand<'a> {
    Noop,
    Help,
    Hooks(Option<&'a str>, Option<&'a str>),
    Mode(Option<&'a str>, Option<&'a str>),
    Info(SlashInfoCommand),
    Hint(&'a str),
    Unknown(&'a str),
}

impl<'a> SlashCommand<'a> {
    fn parse(input: &'a str) -> Option<Self> {
        let mut parts = input.split_whitespace();
        let token = parts.next()?;
        match token {
            "/help" => Some(Self::Help),
            "/hooks" => {
                let sub = parts.next();
                let arg = parts.next();
                Some(Self::Hooks(sub, arg))
            }
            "/mode" | "/approval-mode" => {
                let first = parts.next();
                let second = parts.next();
                Some(Self::Mode(first, second))
            }
            "/audit" => Some(Self::Info(SlashInfoCommand::Audit)),
            "/config" => Some(Self::Info(SlashInfoCommand::Config)),
            "/skill" => Some(Self::Info(SlashInfoCommand::Skill)),
            "/agent" | "/cancel" | "/clear" | "/copy" | "/details" | "/explain" | "/select"
            | "/shell" => None,
            "/" => Some(Self::Noop),
            token if token.starts_with('/') => {
                if slash_command_hints(token).is_empty() {
                    Some(Self::Unknown(token))
                } else {
                    Some(Self::Hint(token))
                }
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SlashInfoCommand {
    Audit,
    Config,
    Skill,
}

fn render_help<W: Write>(state: &InlineState, output: &mut W) -> std::io::Result<()> {
    let body = all_slash_command_hints()
        .iter()
        .map(|hint| format!("{} - {}", hint.usage, hint.summary))
        .collect::<Vec<_>>();

    RatatuiInlineRenderer::for_terminal().write_notice(
        output,
        "Slash commands",
        body,
        Some(&format!(
            "Mode: {}. Strategy: {}.",
            state.approval_mode.user_mode_label(),
            state.analysis_mode.label()
        )),
    )
}

fn render_hooks_command<W: Write>(
    sub: Option<&str>,
    arg: Option<&str>,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let renderer = RatatuiInlineRenderer::for_terminal();
    match (sub, arg) {
        (None, _) => {
            let hooks = state.hook_engine.registered_hooks();
            let disabled = &state.disabled_hooks;
            let body: Vec<String> = if hooks.is_empty() {
                vec!["No hooks registered.".to_string()]
            } else {
                hooks
                    .iter()
                    .map(|id| {
                        if disabled.contains(*id) {
                            format!("{id} (disabled)")
                        } else {
                            id.to_string()
                        }
                    })
                    .collect()
            };
            renderer.write_notice(
                output,
                "Registered hooks",
                body,
                Some(&format!("{} hook(s) registered.", hooks.len())),
            )
        }
        (Some("enable"), Some(id)) => {
            state.disabled_hooks.remove(id);
            renderer.write_notice(
                output,
                "Hook enabled",
                vec![format!("Hook '{id}' enabled.")],
                None,
            )
        }
        (Some("disable"), Some(id)) => {
            state.disabled_hooks.insert(id.to_string());
            renderer.write_notice(
                output,
                "Hook disabled",
                vec![format!("Hook '{id}' disabled.")],
                None,
            )
        }
        _ => renderer.write_notice(
            output,
            "Usage",
            vec![
                "/hooks                - list all hooks".into(),
                "/hooks enable <id>    - enable a hook".into(),
                "/hooks disable <id>   - disable a hook".into(),
            ],
            None,
        ),
    }
}

fn render_hint<W: Write>(prefix: &str, state: &InlineState, output: &mut W) -> std::io::Result<()> {
    let mut body = vec![
        format!("Prefix: {prefix}"),
        format!("Current mode: {}", state.approval_mode.user_mode_label()),
    ];
    body.extend(
        slash_command_hints(prefix)
            .into_iter()
            .map(|hint| format!("{} - {}", hint.usage, hint.summary)),
    );

    RatatuiInlineRenderer::for_terminal().write_notice(
        output,
        "Slash command hint",
        body,
        Some("Type a full command and press Enter; paths like /tmp/foo stay in shell."),
    )
}

fn render_info<W: Write>(command: SlashInfoCommand, output: &mut W) -> std::io::Result<()> {
    let (title, body, footer) = match command {
        SlashInfoCommand::Audit => (
            "Audit",
            vec![
                "Approval decisions are available with /details approvals.".to_string(),
                "Activity output refs are available with /details <id>.".to_string(),
            ],
            "Audit views are read-only; no shell command runs.",
        ),
        SlashInfoCommand::Config => (
            "Config",
            vec![
                "Session-local controls: /mode recommend|agent."
                    .to_string(),
                "Advanced legacy controls: /approval-mode suggest|ask|auto|trust; /mode analysis smart|auto|manual."
                    .to_string(),
                "Render fallback: set COSH_SHELL_RENDER=plain before starting cosh-shell."
                    .to_string(),
            ],
            "Config slash commands only report current controls in this MVP.",
        ),
        SlashInfoCommand::Skill => (
            "Skill",
            vec![
                "Command result hook hints can route Agent analysis toward a skill.".to_string(),
                "No external skill registry is configured for this shell session.".to_string(),
            ],
            "Skill hooks are advisory and still go through governance.",
        ),
    };

    RatatuiInlineRenderer::for_terminal().write_notice(output, title, body, Some(footer))
}

fn render_unknown<W: Write>(command: &str, output: &mut W) -> std::io::Result<()> {
    RatatuiInlineRenderer::for_terminal().write_notice(
        output,
        "Slash command",
        vec![format!("Unknown slash command: {command}")],
        Some("Use /help to see available commands."),
    )
}

#[derive(Debug, Clone, Copy)]
struct SlashCommandHint {
    name: &'static str,
    usage: &'static str,
    summary: &'static str,
}

fn slash_command_hints(prefix: &str) -> Vec<SlashCommandHint> {
    all_slash_command_hints()
        .iter()
        .copied()
        .filter(|hint| prefix == "/" || hint.name.starts_with(prefix))
        .collect()
}

fn all_slash_command_hints() -> &'static [SlashCommandHint] {
    &[
        SlashCommandHint {
            name: "/approval-mode",
            usage: "/approval-mode [suggest|ask|auto|trust]",
            summary: "advanced legacy governance strategy",
        },
        SlashCommandHint {
            name: "/audit",
            usage: "/audit",
            summary: "show audit entry points",
        },
        SlashCommandHint {
            name: "/config",
            usage: "/config",
            summary: "show session-local controls",
        },
        SlashCommandHint {
            name: "/details",
            usage: "/details <id>",
            summary: "inspect approval/activity details",
        },
        SlashCommandHint {
            name: "/help",
            usage: "/help",
            summary: "show command reference",
        },
        SlashCommandHint {
            name: "/hooks",
            usage: "/hooks",
            summary: "list registered hooks",
        },
        SlashCommandHint {
            name: "/mode",
            usage: "/mode [recommend|agent]",
            summary: "show or change user mode",
        },
        SlashCommandHint {
            name: "/mode",
            usage: "/mode analysis [smart|auto|manual]",
            summary: "advanced legacy analysis strategy",
        },
        SlashCommandHint {
            name: "/skill",
            usage: "/skill",
            summary: "show skill-related controls",
        },
    ]
}
