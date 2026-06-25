use crate::runtime::prelude::{
    visible_slash_commands, ShellEvent, ShellEventKind, SlashCommandSpec,
};

pub(super) fn slash_input(event: &ShellEvent) -> Option<&str> {
    if event.kind != ShellEventKind::UserInputIntercepted {
        return None;
    }
    if event.component.as_deref() != Some("slash") {
        return None;
    }
    event.input.as_deref()
}

pub(super) enum SlashCommand<'a> {
    Noop,
    Help,
    Auth,
    Hooks(Option<&'a str>, Option<&'a str>, Option<&'a str>),
    Mode(Option<&'a str>, Option<&'a str>, Option<&'a str>),
    Config(Option<&'a str>, Option<&'a str>),
    Debug(Option<&'a str>),
    Info(SlashInfoCommand),
    Removed(RemovedCommand<'a>),
    Hint(&'a str),
    Unknown(&'a str),
    Extensions(Option<&'a str>, Option<&'a str>),
    Skills(Option<&'a str>, Option<&'a str>),
}

impl<'a> SlashCommand<'a> {
    pub(super) fn parse(input: &'a str) -> Option<Self> {
        let mut parts = input.split_whitespace();
        let token = parts.next()?;
        match token {
            "/help" => Some(Self::Help),
            "/auth" => Some(Self::Auth),
            "/hooks" => {
                let sub = parts.next();
                let arg = parts.next();
                let extra = parts.next();
                Some(Self::Hooks(sub, arg, extra))
            }
            "/mode" => {
                let first = parts.next();
                let second = parts.next();
                let third = parts.next();
                Some(Self::Mode(first, second, third))
            }
            "/approval-mode" => Some(Self::Removed(RemovedCommand::ApprovalMode(parts.next()))),
            "/allow" | "/approve" | "/deny" => {
                Some(Self::Removed(RemovedCommand::ApprovalDecision(token)))
            }
            "/answer" => Some(Self::Removed(RemovedCommand::QuestionAnswer)),
            "/audit" => Some(Self::Info(SlashInfoCommand::Audit)),
            "/config" => {
                let sub = parts.next();
                let value = parts.next();
                Some(Self::Config(sub, value))
            }
            "/debug" => Some(Self::Debug(parts.next())),
            "/extensions" => {
                let sub = parts.next();
                let arg = parts.next();
                Some(Self::Extensions(sub, arg))
            }
            "/skills" => {
                let sub = parts.next();
                let arg = parts.next();
                Some(Self::Skills(sub, arg))
            }
            "/agent" | "/cancel" | "/clear" | "/copy" | "/details" | "/explain" | "/select"
            | "/send-to-shell" | "/shell" => None,
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
pub(super) enum SlashInfoCommand {
    Audit,
    Config,
}

pub(super) enum RemovedCommand<'a> {
    ApprovalMode(Option<&'a str>),
    ApprovalDecision(&'a str),
    QuestionAnswer,
}

pub(super) fn slash_command_hints(prefix: &str) -> Vec<&'static SlashCommandSpec> {
    visible_slash_commands()
        .filter(|hint| prefix == "/" || hint.name.starts_with(prefix))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{RemovedCommand, SlashCommand, SlashCommandSpec};

    #[test]
    fn removed_decision_commands_parse_as_removed_not_unknown() {
        for command in ["/allow 1", "/approve 1", "/deny 1"] {
            match SlashCommand::parse(command) {
                Some(SlashCommand::Removed(RemovedCommand::ApprovalDecision(token))) => {
                    assert_eq!(token, command.split_whitespace().next().unwrap());
                }
                _ => panic!("{command} did not parse as removed approval decision"),
            }
        }

        match SlashCommand::parse("/answer yes") {
            Some(SlashCommand::Removed(RemovedCommand::QuestionAnswer)) => {}
            _ => panic!("/answer did not parse as removed question answer"),
        }
    }

    #[test]
    fn hidden_and_contextual_commands_are_not_public_hints() {
        assert!(slash_hints("/co").iter().any(|hint| hint.name == "/config"));
        for prefix in ["/ag", "/ca", "/de", "/au", "/se", "/co", "/send", "/debug"] {
            let hints = slash_hints(prefix);
            assert!(
                hints.iter().all(|hint| matches!(
                    hint.name,
                    "/config" | "/mode" | "/hooks" | "/extensions" | "/skills"
                )),
                "{prefix} returned non-public hints: {:?}",
                hints.iter().map(|hint| hint.name).collect::<Vec<_>>()
            );
        }
        // /ex and /skill now match public commands
        assert!(slash_hints("/ex")
            .iter()
            .any(|hint| hint.name == "/extensions"));
        assert!(slash_hints("/skill")
            .iter()
            .any(|hint| hint.name == "/skills"));
    }

    fn slash_hints(prefix: &str) -> Vec<&'static SlashCommandSpec> {
        super::slash_command_hints(prefix)
    }
}
