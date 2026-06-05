#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputClassifier {
    slash_commands: Vec<String>,
}

impl Default for InputClassifier {
    fn default() -> Self {
        Self {
            slash_commands: [
                "/agent",
                "/approval-mode",
                "/audit",
                "/cancel",
                "/clear",
                "/config",
                "/copy",
                "/details",
                "/explain",
                "/help",
                "/mode",
                "/select",
                "/shell",
                "/skill",
            ]
            .iter()
            .map(|command| command.to_string())
            .collect(),
        }
    }
}

impl InputClassifier {
    pub fn classify(&self, input: &str) -> InputDecision {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return InputDecision::SendToShell(input.to_string());
        }

        if trimmed.starts_with("??") {
            return InputDecision::Intercept {
                input: input.to_string(),
                reason: InterceptReason::AgentMarker,
            };
        }

        let first_token = trimmed.split_whitespace().next().unwrap_or_default();
        if self.is_slash_control_input(first_token) {
            return InputDecision::Intercept {
                input: input.to_string(),
                reason: InterceptReason::Slash,
            };
        }

        if starts_with_shell_command(trimmed) {
            return InputDecision::SendToShell(input.to_string());
        }

        if looks_like_natural_language(trimmed) {
            return InputDecision::Intercept {
                input: input.to_string(),
                reason: InterceptReason::NaturalLanguage,
            };
        }

        InputDecision::SendToShell(input.to_string())
    }

    fn is_slash_control_input(&self, token: &str) -> bool {
        if self.slash_commands.iter().any(|command| token == command) {
            return true;
        }

        is_slash_hint_candidate(token, &self.slash_commands)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputDecision {
    SendToShell(String),
    Intercept {
        input: String,
        reason: InterceptReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterceptReason {
    Slash,
    NaturalLanguage,
    AgentMarker,
}

impl InterceptReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Slash => "slash",
            Self::NaturalLanguage => "natural_language",
            Self::AgentMarker => "agent_marker",
        }
    }
}

fn starts_with_shell_command(input: &str) -> bool {
    let Some(token) = command_token(input) else {
        return false;
    };

    is_path_like_command(token) || is_known_shell_command(token)
}

fn command_token(input: &str) -> Option<&str> {
    input
        .split_whitespace()
        .find(|token| !is_env_assignment(token))
}

fn is_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_path_like_command(token: &str) -> bool {
    token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with("~/")
}

fn is_slash_hint_candidate(token: &str, slash_commands: &[String]) -> bool {
    if token == "/" {
        return true;
    }
    if !token.starts_with('/') || token[1..].contains('/') {
        return false;
    }
    if std::path::Path::new(token).exists() {
        return false;
    }
    slash_commands
        .iter()
        .any(|command| command.starts_with(token))
        || token.len() > 1
}

fn is_known_shell_command(token: &str) -> bool {
    matches!(
        token,
        "awk"
            | "bash"
            | "bat"
            | "brew"
            | "bun"
            | "cargo"
            | "cat"
            | "cd"
            | "chmod"
            | "chown"
            | "cp"
            | "curl"
            | "docker"
            | "du"
            | "echo"
            | "env"
            | "fd"
            | "find"
            | "git"
            | "grep"
            | "head"
            | "less"
            | "ls"
            | "make"
            | "mkdir"
            | "mv"
            | "node"
            | "npm"
            | "npx"
            | "nvim"
            | "pnpm"
            | "printf"
            | "ps"
            | "pwd"
            | "python"
            | "python3"
            | "rg"
            | "rm"
            | "sed"
            | "sh"
            | "sudo"
            | "tail"
            | "top"
            | "touch"
            | "tree"
            | "vi"
            | "vim"
            | "yarn"
    )
}

fn looks_like_natural_language(input: &str) -> bool {
    if input.chars().any(|ch| !ch.is_ascii() && ch.is_alphabetic()) {
        return true;
    }

    let lower = input.to_ascii_lowercase();
    let first = lower.split_whitespace().next().unwrap_or_default();
    matches!(first, "why" | "how" | "what" | "explain" | "fix" | "please")
        && lower.split_whitespace().count() > 1
}

#[cfg(test)]
mod tests {
    use super::{InputClassifier, InputDecision, InterceptReason};

    #[test]
    fn classifies_known_slash_commands_without_capturing_paths() {
        let classifier = InputClassifier::default();
        assert_eq!(
            classifier.classify("/explain last error"),
            InputDecision::Intercept {
                input: "/explain last error".to_string(),
                reason: InterceptReason::Slash
            }
        );
        assert_eq!(
            classifier.classify("/tmp/tool --help"),
            InputDecision::SendToShell("/tmp/tool --help".to_string())
        );
        assert_eq!(
            classifier.classify("/select 1"),
            InputDecision::Intercept {
                input: "/select 1".to_string(),
                reason: InterceptReason::Slash
            }
        );
        assert_eq!(
            classifier.classify("/allow 1"),
            InputDecision::Intercept {
                input: "/allow 1".to_string(),
                reason: InterceptReason::Slash
            }
        );
        assert_eq!(
            classifier.classify("/approval-mode auto"),
            InputDecision::Intercept {
                input: "/approval-mode auto".to_string(),
                reason: InterceptReason::Slash
            }
        );
        assert_eq!(
            classifier.classify("/cancel"),
            InputDecision::Intercept {
                input: "/cancel".to_string(),
                reason: InterceptReason::Slash
            }
        );
        assert_eq!(
            classifier.classify("/"),
            InputDecision::Intercept {
                input: "/".to_string(),
                reason: InterceptReason::Slash
            }
        );
        assert_eq!(
            classifier.classify("/mo"),
            InputDecision::Intercept {
                input: "/mo".to_string(),
                reason: InterceptReason::Slash
            }
        );
        assert_eq!(
            classifier.classify("/modd"),
            InputDecision::Intercept {
                input: "/modd".to_string(),
                reason: InterceptReason::Slash
            }
        );
        assert_eq!(
            classifier.classify("/tmp/tool --help"),
            InputDecision::SendToShell("/tmp/tool --help".to_string())
        );
        assert_eq!(
            classifier.classify("/tmp"),
            InputDecision::SendToShell("/tmp".to_string())
        );
        assert_eq!(
            classifier.classify("/details req-1"),
            InputDecision::Intercept {
                input: "/details req-1".to_string(),
                reason: InterceptReason::Slash
            }
        );
    }

    #[test]
    fn classifies_natural_language_and_marker_inputs() {
        let classifier = InputClassifier::default();
        assert_eq!(
            classifier.classify("\u{5e2e}\u{6211}\u{5206}\u{6790}"),
            InputDecision::Intercept {
                input: "\u{5e2e}\u{6211}\u{5206}\u{6790}".to_string(),
                reason: InterceptReason::NaturalLanguage
            }
        );
        assert_eq!(
            classifier.classify("?? last command"),
            InputDecision::Intercept {
                input: "?? last command".to_string(),
                reason: InterceptReason::AgentMarker
            }
        );
        assert_eq!(
            classifier.classify("echo why not"),
            InputDecision::SendToShell("echo why not".to_string())
        );
    }

    #[test]
    fn classifies_shell_commands_with_non_ascii_arguments_as_shell() {
        let classifier = InputClassifier::default();
        let chinese_doc = "\u{8bbe}\u{8ba1}\u{6587}\u{6863}.md";
        let escaped_vim_path = format!(
            "vim cosh-ng\\ AI\\ Shell\\ \\{}\\ {}",
            "\u{2014}", chinese_doc
        );
        assert_eq!(
            classifier.classify(&format!("cat {chinese_doc}")),
            InputDecision::SendToShell(format!("cat {chinese_doc}"))
        );
        assert_eq!(
            classifier.classify(&escaped_vim_path),
            InputDecision::SendToShell(escaped_vim_path)
        );
        assert_eq!(
            classifier.classify("echo \u{4f60}\u{597d}"),
            InputDecision::SendToShell("echo \u{4f60}\u{597d}".to_string())
        );
        assert_eq!(
            classifier.classify(&format!("printf ok > /tmp/{chinese_doc}")),
            InputDecision::SendToShell(format!("printf ok > /tmp/{chinese_doc}"))
        );
        assert_eq!(
            classifier.classify(&format!("LC_ALL=C cat {chinese_doc}")),
            InputDecision::SendToShell(format!("LC_ALL=C cat {chinese_doc}"))
        );
    }
}
