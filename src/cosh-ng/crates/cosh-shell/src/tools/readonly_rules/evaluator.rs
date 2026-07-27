use super::runtime_config::RuntimeValidator;
use super::runtime_config::{RuntimeGenericSpec, RuntimeReadonlyConfig, RuntimeSubcommandSpec};
use super::specs::{GenericSpec, PathMode, SubcommandSpec, Validator};

pub(super) fn evaluate(validator: &Validator, tokens: &[String]) -> bool {
    match validator {
        Validator::Bare => tokens.len() == 1,
        Validator::Generic(spec) => evaluate_generic(&tokens[1..], spec),
        Validator::Subcommand(spec) => evaluate_subcommand(tokens, spec),
        Validator::VersionCheck(flags) => evaluate_version_check(tokens, flags),
        Validator::Custom(f) => f(tokens),
    }
}

pub(super) fn evaluate_runtime(validator: &RuntimeValidator, tokens: &[String]) -> bool {
    match validator {
        RuntimeValidator::Bare => tokens.len() == 1,
        RuntimeValidator::Generic(spec) => evaluate_runtime_generic(&tokens[1..], spec),
        RuntimeValidator::Subcommand(spec) => evaluate_runtime_subcommand(tokens, spec),
        RuntimeValidator::VersionCheck(flags) => {
            tokens.len() == 2 && flags.iter().any(|flag| flag == &tokens[1])
        }
    }
}

pub(super) fn config_disables_command(
    config: &RuntimeReadonlyConfig,
    command: &str,
    subcommand: Option<&str>,
) -> bool {
    config
        .disabled
        .iter()
        .any(|key| key.matches(command, subcommand))
}

fn evaluate_runtime_generic(args: &[String], spec: &RuntimeGenericSpec) -> bool {
    let mut idx = 0;
    let mut saw_path = false;
    let mut positionals_only = false;

    while idx < args.len() {
        let token = args[idx].as_str();

        if positionals_only {
            if !check_positional_after_separator(token, spec.path_mode) {
                return false;
            }
            saw_path = true;
            idx += 1;
            continue;
        }

        if token == "--" {
            positionals_only = true;
            idx += 1;
            continue;
        }

        if token.starts_with("--") {
            if spec.deny_flags.iter().any(|d| token.starts_with(d)) {
                return false;
            }

            if let Some((flag, bound)) = spec
                .value_flags
                .iter()
                .find(|(f, _)| f == token || token.starts_with(&format!("{f}=")))
            {
                if token.contains('=') {
                    let val = token.split_once('=').unwrap().1;
                    if let Some(max) = bound {
                        if !is_bounded_positive_count(val, *max) {
                            return false;
                        }
                    }
                } else {
                    idx += 1;
                    let Some(val) = args.get(idx) else {
                        return false;
                    };
                    if let Some(max) = bound {
                        if !is_bounded_positive_count(val, *max) {
                            return false;
                        }
                    }
                }
                let _ = flag;
                idx += 1;
                continue;
            }

            if spec.long_flags.iter().any(|flag| flag == token) {
                idx += 1;
                continue;
            }

            return false;
        }

        if token.starts_with('-') && token.len() > 1 {
            if spec.deny_flags.iter().any(|d| token.starts_with(d)) {
                return false;
            }

            if let Some((flag, bound)) = spec
                .value_flags
                .iter()
                .find(|(f, _)| f.len() == 2 && token.starts_with(f))
            {
                if token.len() > flag.len() {
                    let val = &token[flag.len()..];
                    if let Some(max) = bound {
                        if !is_bounded_positive_count(val, *max) {
                            return false;
                        }
                    }
                } else {
                    idx += 1;
                    let Some(val) = args.get(idx) else {
                        return false;
                    };
                    if let Some(max) = bound {
                        if !is_bounded_positive_count(val, *max) {
                            return false;
                        }
                    }
                }
                idx += 1;
                continue;
            }

            if spec.bare_number_max > 0 && token[1..].chars().all(|ch| ch.is_ascii_digit()) {
                if !is_bounded_positive_count(&token[1..], spec.bare_number_max) {
                    return false;
                }
                idx += 1;
                continue;
            }

            let chars = &token[1..];
            if !chars.chars().all(|ch| spec.short_flags.contains(ch)) {
                return false;
            }
            idx += 1;
            continue;
        }

        if !check_positional(token, spec.path_mode) {
            return false;
        }
        saw_path = true;
        idx += 1;
    }

    match spec.path_mode {
        PathMode::Required => saw_path,
        _ => true,
    }
}

fn evaluate_runtime_subcommand(tokens: &[String], spec: &RuntimeSubcommandSpec) -> bool {
    if tokens.len() < 2 {
        return false;
    }

    if tokens
        .iter()
        .skip(1)
        .any(|arg| spec.deny_args.iter().any(|deny| arg.starts_with(deny)))
    {
        return false;
    }

    let subcmd = tokens[1].as_str();
    spec.subcommands
        .iter()
        .find(|(name, _)| name == subcmd)
        .is_some_and(|(_, validator)| {
            let sub_tokens = &tokens[1..];
            match validator {
                RuntimeValidator::Bare => sub_tokens.len() == 1,
                RuntimeValidator::Generic(g) => evaluate_runtime_generic(&sub_tokens[1..], g),
                RuntimeValidator::VersionCheck(flags) => {
                    sub_tokens.len() == 2 && flags.iter().any(|flag| flag == &sub_tokens[1])
                }
                RuntimeValidator::Subcommand(_) => false,
            }
        })
}

fn evaluate_generic(args: &[String], spec: &GenericSpec) -> bool {
    let mut idx = 0;
    let mut saw_path = false;
    let mut positionals_only = false;

    while idx < args.len() {
        let token = args[idx].as_str();

        if positionals_only {
            if !check_positional_after_separator(token, spec.path_mode) {
                return false;
            }
            saw_path = true;
            idx += 1;
            continue;
        }

        if token == "--" {
            positionals_only = true;
            idx += 1;
            continue;
        }

        if token.starts_with("--") {
            if spec.deny_flags.iter().any(|d| token.starts_with(d)) {
                return false;
            }

            if let Some(&(_, bound)) = spec
                .value_flags
                .iter()
                .find(|(f, _)| *f == token || token.starts_with(&format!("{f}=")))
            {
                if token.contains('=') {
                    let val = token.split_once('=').unwrap().1;
                    if let Some(max) = bound {
                        if !is_bounded_positive_count(val, max) {
                            return false;
                        }
                    }
                } else {
                    idx += 1;
                    let Some(val) = args.get(idx) else {
                        return false;
                    };
                    if let Some(max) = bound {
                        if !is_bounded_positive_count(val, max) {
                            return false;
                        }
                    }
                }
                idx += 1;
                continue;
            }

            if spec.long_flags.contains(&token) {
                idx += 1;
                continue;
            }

            return false;
        }

        if token.starts_with('-') && token.len() > 1 {
            if spec.deny_flags.iter().any(|d| token.starts_with(d)) {
                return false;
            }

            if let Some(&(flag, bound)) = spec
                .value_flags
                .iter()
                .find(|(f, _)| f.len() == 2 && token.starts_with(f))
            {
                if token.len() > flag.len() {
                    let val = &token[flag.len()..];
                    if let Some(max) = bound {
                        if !is_bounded_positive_count(val, max) {
                            return false;
                        }
                    }
                } else {
                    idx += 1;
                    let Some(val) = args.get(idx) else {
                        return false;
                    };
                    if let Some(max) = bound {
                        if !is_bounded_positive_count(val, max) {
                            return false;
                        }
                    }
                }
                idx += 1;
                continue;
            }

            if spec.bare_number_max > 0
                && token.len() > 1
                && token[1..].chars().all(|ch| ch.is_ascii_digit())
            {
                if !is_bounded_positive_count(&token[1..], spec.bare_number_max) {
                    return false;
                }
                idx += 1;
                continue;
            }

            let chars = &token[1..];
            if !chars.chars().all(|ch| spec.short_flags.contains(ch)) {
                return false;
            }
            idx += 1;
            continue;
        }

        if !check_positional(token, spec.path_mode) {
            return false;
        }
        saw_path = true;
        idx += 1;
    }

    match spec.path_mode {
        PathMode::Required => saw_path,
        _ => true,
    }
}

fn evaluate_subcommand(tokens: &[String], spec: &SubcommandSpec) -> bool {
    if tokens.len() < 2 {
        return false;
    }

    if tokens
        .iter()
        .skip(1)
        .any(|a| spec.deny_args.iter().any(|d| a.starts_with(d)))
    {
        return false;
    }

    let subcmd = tokens[1].as_str();
    spec.subcommands
        .iter()
        .find(|(name, _)| *name == subcmd)
        .map(|(_, validator)| {
            let sub_tokens = &tokens[1..];
            match validator {
                Validator::Bare => sub_tokens.len() == 1,
                Validator::Generic(g) => evaluate_generic(&sub_tokens[1..], g),
                Validator::Custom(f) => f(tokens),
                Validator::VersionCheck(flags) => evaluate_version_check(sub_tokens, flags),
                Validator::Subcommand(_) => false,
            }
        })
        .unwrap_or(false)
}

fn evaluate_version_check(tokens: &[String], allowed: &[&str]) -> bool {
    tokens.len() == 2 && allowed.contains(&tokens[1].as_str())
}

fn check_positional(token: &str, mode: PathMode) -> bool {
    match mode {
        PathMode::None => false,
        PathMode::Unchecked => true,
        PathMode::Optional | PathMode::Required => is_safe_readonly_path(token),
    }
}

fn check_positional_after_separator(token: &str, mode: PathMode) -> bool {
    match mode {
        PathMode::None => false,
        PathMode::Unchecked => true,
        PathMode::Optional | PathMode::Required => {
            !token.is_empty() && !is_blocked_special_path(token)
        }
    }
}

pub fn is_safe_readonly_path(path: &str) -> bool {
    !path.is_empty() && path != "-" && !path.starts_with('-') && !is_blocked_special_path(path)
}

pub fn is_bounded_positive_count(value: &str, max: u32) -> bool {
    let Ok(count) = value.parse::<u32>() else {
        return false;
    };
    count > 0 && count <= max
}

pub fn is_blocked_special_path(path: &str) -> bool {
    path == "/dev"
        || path.starts_with("/dev/")
        || path == "/proc"
        || path.starts_with("/proc/")
        || path == "/sys"
        || path.starts_with("/sys/")
}
