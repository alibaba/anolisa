#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputClassifier {
    slash_commands: Vec<String>,
    slash_hint_commands: Vec<String>,
    conservative: bool,
    ai_enabled: bool,
}

impl InputClassifier {
    pub fn with_conservative(mut self, conservative: bool) -> Self {
        self.conservative = conservative;
        self
    }

    pub fn with_ai_enabled(mut self, ai_enabled: bool) -> Self {
        self.ai_enabled = ai_enabled;
        self
    }
}

impl Default for InputClassifier {
    fn default() -> Self {
        Self {
            slash_commands: crate::slash::registry::exact_slash_control_commands()
                .map(str::to_string)
                .collect(),
            slash_hint_commands: crate::slash::registry::active_slash_hint_commands()
                .map(str::to_string)
                .collect(),
            conservative: false,
            ai_enabled: true,
        }
    }
}

impl InputClassifier {
    pub fn conservative() -> Self {
        Self {
            conservative: true,
            ..Self::default()
        }
    }

    pub fn is_conservative(&self) -> bool {
        self.conservative
    }

    pub(crate) fn is_slash_control_candidate(&self, token: &str) -> bool {
        self.is_slash_control_input(token)
    }

    pub fn classify(&self, input: &str) -> InputDecision {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return InputDecision::SendToShell(input.to_string());
        }

        let first_token = trimmed.split_whitespace().next().unwrap_or_default();
        if self.is_slash_control_input(first_token) {
            return InputDecision::Intercept {
                input: input.to_string(),
                reason: InterceptReason::Slash,
            };
        }

        if trimmed.starts_with("??") {
            if !self.ai_enabled {
                return InputDecision::Consume;
            }
            return InputDecision::Intercept {
                input: input.to_string(),
                reason: InterceptReason::AgentMarker,
            };
        }

        if self.conservative {
            if looks_like_pure_natural_language(trimmed) {
                if !self.ai_enabled {
                    return InputDecision::Consume;
                }
                return InputDecision::Intercept {
                    input: input.to_string(),
                    reason: InterceptReason::NaturalLanguage,
                };
            }
            return InputDecision::SendToShell(input.to_string());
        }

        if starts_with_shell_command(trimmed) {
            return InputDecision::SendToShell(input.to_string());
        }

        if looks_like_natural_language(trimmed) {
            if !self.ai_enabled {
                return InputDecision::Consume;
            }
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

        is_slash_hint_candidate(token, &self.slash_hint_commands)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputDecision {
    SendToShell(String),
    Intercept {
        input: String,
        reason: InterceptReason,
    },
    Consume,
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
        .any(|command| command.starts_with(token) || edit_distance_at_most(token, command, 2))
}

fn edit_distance_at_most(left: &str, right: &str, max: usize) -> bool {
    left.len().abs_diff(right.len()) <= max && edit_distance(left, right) <= max
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right_chars = right.chars().collect::<Vec<_>>();
    let mut prev = (0..=right_chars.len()).collect::<Vec<_>>();
    let mut curr = vec![0; right_chars.len() + 1];

    for (left_idx, left_ch) in left.chars().enumerate() {
        curr[0] = left_idx + 1;
        for (right_idx, right_ch) in right_chars.iter().enumerate() {
            let cost = usize::from(left_ch != *right_ch);
            curr[right_idx + 1] = (prev[right_idx + 1] + 1)
                .min(curr[right_idx] + 1)
                .min(prev[right_idx] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[right_chars.len()]
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

const SHELL_META_CHARS: &[char] = &[';', '|', '&', '>', '<', '$', '`', '(', ')', '{', '}'];

fn has_command_like_tokens(input: &str) -> bool {
    for token in input.split_whitespace() {
        if token.starts_with('-') {
            return true;
        }
        if token.contains('/') || token.contains('~') {
            return true;
        }
        if token.chars().any(|ch| SHELL_META_CHARS.contains(&ch)) {
            return true;
        }
    }
    false
}

fn looks_like_pure_natural_language(input: &str) -> bool {
    if has_command_like_tokens(input) {
        return false;
    }

    let first_token = input.split_whitespace().next().unwrap_or_default();
    if first_token
        .chars()
        .any(|ch| !ch.is_ascii() && ch.is_alphabetic())
    {
        return true;
    }

    let lower = input.to_ascii_lowercase();
    let first = lower.split_whitespace().next().unwrap_or_default();
    matches!(first, "why" | "how" | "what" | "explain" | "fix" | "please")
        && lower.split_whitespace().count() > 1
}

#[cfg(test)]
mod tests {
    use super::{has_command_like_tokens, InputClassifier, InputDecision, InterceptReason};

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
            classifier.classify("/approve 1"),
            InputDecision::Intercept {
                input: "/approve 1".to_string(),
                reason: InterceptReason::Slash
            }
        );
        assert_eq!(
            classifier.classify("/deny 1"),
            InputDecision::Intercept {
                input: "/deny 1".to_string(),
                reason: InterceptReason::Slash
            }
        );
        assert_eq!(
            classifier.classify("/answer Blue"),
            InputDecision::Intercept {
                input: "/answer Blue".to_string(),
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
            classifier.classify("/approval"),
            InputDecision::SendToShell("/approval".to_string())
        );
        assert_eq!(
            classifier.classify("/hep"),
            InputDecision::Intercept {
                input: "/hep".to_string(),
                reason: InterceptReason::Slash
            }
        );
        assert_eq!(
            classifier.classify("/sk"),
            InputDecision::SendToShell("/sk".to_string())
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

    #[test]
    fn conservative_intercepts_agent_marker() {
        let c = InputClassifier::conservative();
        assert_eq!(
            c.classify("?? what happened"),
            InputDecision::Intercept {
                input: "?? what happened".to_string(),
                reason: InterceptReason::AgentMarker,
            }
        );
    }

    #[test]
    fn conservative_intercepts_slash_commands() {
        let c = InputClassifier::conservative();
        assert_eq!(
            c.classify("/explain last error"),
            InputDecision::Intercept {
                input: "/explain last error".to_string(),
                reason: InterceptReason::Slash,
            }
        );
        assert_eq!(
            c.classify("/help"),
            InputDecision::Intercept {
                input: "/help".to_string(),
                reason: InterceptReason::Slash,
            }
        );
    }

    #[test]
    fn conservative_intercepts_pure_natural_language() {
        let c = InputClassifier::conservative();
        assert_eq!(
            c.classify("\u{5e2e}\u{6211}\u{5206}\u{6790}"),
            InputDecision::Intercept {
                input: "\u{5e2e}\u{6211}\u{5206}\u{6790}".to_string(),
                reason: InterceptReason::NaturalLanguage,
            }
        );
        assert_eq!(
            c.classify("why is the build failing"),
            InputDecision::Intercept {
                input: "why is the build failing".to_string(),
                reason: InterceptReason::NaturalLanguage,
            }
        );
        assert_eq!(
            c.classify("how do I reset my password"),
            InputDecision::Intercept {
                input: "how do I reset my password".to_string(),
                reason: InterceptReason::NaturalLanguage,
            }
        );
        assert_eq!(
            c.classify("what is a mutex"),
            InputDecision::Intercept {
                input: "what is a mutex".to_string(),
                reason: InterceptReason::NaturalLanguage,
            }
        );
        assert_eq!(
            c.classify("explain the error"),
            InputDecision::Intercept {
                input: "explain the error".to_string(),
                reason: InterceptReason::NaturalLanguage,
            }
        );
    }

    #[test]
    fn conservative_sends_unknown_commands_to_shell() {
        let c = InputClassifier::conservative();
        assert_eq!(
            c.classify("git status"),
            InputDecision::SendToShell("git status".to_string())
        );
        assert_eq!(
            c.classify("ls -la"),
            InputDecision::SendToShell("ls -la".to_string())
        );
        assert_eq!(
            c.classify("cargo build"),
            InputDecision::SendToShell("cargo build".to_string())
        );
        assert_eq!(
            c.classify("mycustomtool run"),
            InputDecision::SendToShell("mycustomtool run".to_string())
        );
    }

    #[test]
    fn conservative_rejects_nl_with_flags() {
        let c = InputClassifier::conservative();
        assert_eq!(
            c.classify("why -v"),
            InputDecision::SendToShell("why -v".to_string())
        );
        assert_eq!(
            c.classify("fix --dry-run"),
            InputDecision::SendToShell("fix --dry-run".to_string())
        );
    }

    #[test]
    fn conservative_rejects_nl_with_paths() {
        let c = InputClassifier::conservative();
        assert_eq!(
            c.classify("explain /etc/passwd"),
            InputDecision::SendToShell("explain /etc/passwd".to_string())
        );
        assert_eq!(
            c.classify("fix src/main.rs"),
            InputDecision::SendToShell("fix src/main.rs".to_string())
        );
        assert_eq!(
            c.classify("what ~/docs"),
            InputDecision::SendToShell("what ~/docs".to_string())
        );
    }

    #[test]
    fn conservative_rejects_nl_with_shell_metacharacters() {
        let c = InputClassifier::conservative();
        assert_eq!(
            c.classify("why echo | grep foo"),
            InputDecision::SendToShell("why echo | grep foo".to_string())
        );
        assert_eq!(
            c.classify("how to cat > file"),
            InputDecision::SendToShell("how to cat > file".to_string())
        );
        assert_eq!(
            c.classify("fix $HOME"),
            InputDecision::SendToShell("fix $HOME".to_string())
        );
    }

    #[test]
    fn conservative_rejects_non_ascii_with_command_tokens() {
        let c = InputClassifier::conservative();
        assert_eq!(
            c.classify("cat \u{8bbe}\u{8ba1}\u{6587}\u{6863}.md"),
            InputDecision::SendToShell("cat \u{8bbe}\u{8ba1}\u{6587}\u{6863}.md".to_string())
        );
        assert_eq!(
            c.classify("\u{67e5}\u{770b} --help"),
            InputDecision::SendToShell("\u{67e5}\u{770b} --help".to_string())
        );
    }

    #[test]
    fn conservative_default_is_false() {
        let d = InputClassifier::default();
        assert!(!d.conservative);
    }

    #[test]
    fn has_command_like_tokens_detects_flags_paths_metas() {
        assert!(has_command_like_tokens("-v"));
        assert!(has_command_like_tokens("foo --bar"));
        assert!(has_command_like_tokens("foo /etc/passwd"));
        assert!(has_command_like_tokens("foo ~/dir"));
        assert!(has_command_like_tokens("a | b"));
        assert!(has_command_like_tokens("echo $VAR"));
        assert!(!has_command_like_tokens("why is the sky blue"));
        assert!(!has_command_like_tokens("explain the error"));
    }

    #[test]
    fn default_mode_unchanged_for_known_shell_commands() {
        let d = InputClassifier::default();
        assert_eq!(
            d.classify("git status"),
            InputDecision::SendToShell("git status".to_string())
        );
        assert_eq!(
            d.classify("\u{5e2e}\u{6211}\u{5206}\u{6790}"),
            InputDecision::Intercept {
                input: "\u{5e2e}\u{6211}\u{5206}\u{6790}".to_string(),
                reason: InterceptReason::NaturalLanguage,
            }
        );
    }

    #[test]
    fn ai_disabled_consumes_agent_inputs_but_keeps_shell_and_slash() {
        let d = InputClassifier::default().with_ai_enabled(false);
        assert_eq!(d.classify("?? last command"), InputDecision::Consume);
        assert_eq!(d.classify("why is this failing"), InputDecision::Consume);
        assert_eq!(
            d.classify("\u{5e2e}\u{6211}\u{5206}\u{6790}"),
            InputDecision::Consume
        );
        assert_eq!(
            d.classify("/help"),
            InputDecision::Intercept {
                input: "/help".to_string(),
                reason: InterceptReason::Slash,
            }
        );
        assert_eq!(
            d.classify("echo ok"),
            InputDecision::SendToShell("echo ok".to_string())
        );
    }
}
