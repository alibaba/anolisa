use std::collections::HashSet;
use std::process::Command;

use cosh_shell::{adapter_for_kind, AdapterKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawShellKind {
    Bash,
    Zsh,
    MissingShellValue,
    Unsupported(String),
}

pub(crate) fn adapter_name_from_args(args: &[String]) -> Option<&str> {
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--shell" => idx += 2,
            arg if arg.starts_with("--shell=") => idx += 1,
            arg if arg.starts_with("--") => idx += 1,
            arg => return Some(arg),
        }
    }

    None
}

pub(crate) fn raw_shell_from_args_or_default(args: &[String], default_shell: &str) -> RawShellKind {
    if let Some(shell) = raw_shell_from_args(args) {
        return shell;
    }

    if let Some(shell) = std::env::var("COSH_SHELL_RAW_SHELL")
        .ok()
        .as_deref()
        .map(parse_raw_shell)
    {
        return shell;
    }

    shell_from_default_or_auto(default_shell)
}

pub(crate) fn raw_shell_from_args(args: &[String]) -> Option<RawShellKind> {
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--shell" => {
                return Some(match args.get(idx + 1) {
                    Some(value) if !value.starts_with("--") => parse_raw_shell(value),
                    _ => RawShellKind::MissingShellValue,
                });
            }
            arg if arg.starts_with("--shell=") => {
                return Some(parse_raw_shell(arg.trim_start_matches("--shell=")));
            }
            _ => idx += 1,
        }
    }

    None
}

pub(crate) fn should_start_default_raw(args: &[String]) -> bool {
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--shell" => {
                if !matches!(args.get(idx + 1), Some(value) if !value.starts_with("--")) {
                    return false;
                }
                idx += 2;
            }
            "--isolated" | "--login" | "-l" => idx += 1,
            arg if arg.starts_with("--shell=") => idx += 1,
            _ => return false,
        }
    }

    true
}

pub(crate) fn parse_raw_shell(value: &str) -> RawShellKind {
    let name = value.rsplit('/').next().unwrap_or(value);
    match name {
        "bash" | "cosh-shell-bash" => RawShellKind::Bash,
        "zsh" | "cosh-shell-zsh" => RawShellKind::Zsh,
        other => RawShellKind::Unsupported(other.to_string()),
    }
}

pub(crate) fn shell_from_default_or_auto(value: &str) -> RawShellKind {
    let value = value.trim();
    if !value.is_empty() && value != "auto" {
        return parse_raw_shell(value);
    }

    for candidate in [
        cosh_shell_default_state_previous_shell(),
        std::env::var("SHELL").ok(),
    ]
    .into_iter()
    .flatten()
    {
        let shell = parse_raw_shell(&candidate);
        if matches!(shell, RawShellKind::Bash | RawShellKind::Zsh) {
            return shell;
        }
    }

    RawShellKind::Bash
}

fn cosh_shell_default_state_previous_shell() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let path = std::path::Path::new(&home)
        .join(".config")
        .join("cosh")
        .join("cosh-shell-default.state");
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().find_map(|line| {
        line.strip_prefix("PREVIOUS_SHELL=")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

pub(crate) fn run_raw(adapter_name: &str, shell_kind: RawShellKind) -> i32 {
    crate::runtime::controller::run_raw(adapter_name, shell_kind)
}

pub(crate) fn bootstrap_process_path_from_shell(shell_kind: &RawShellKind, login: bool) {
    if std::env::var("COSH_SHELL_BOOTSTRAP_PATH").as_deref() == Ok("0") {
        return;
    }

    let shell = match shell_kind {
        RawShellKind::Bash => "bash",
        RawShellKind::Zsh => "zsh",
        _ => return,
    };
    let flags = if login { "-lic" } else { "-ic" };
    let Ok(output) = Command::new(shell)
        .arg(flags)
        .arg("printf '\\n__COSH_PATH_BEGIN__%s__COSH_PATH_END__\\n' \"$PATH\"")
        .env("COSH_SHELL_BOOTSTRAP_PATH", "0")
        .output()
    else {
        return;
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let Some(path) = extract_bootstrap_path(&text) else {
        return;
    };
    let current = std::env::var("PATH").unwrap_or_default();
    let merged = merge_path_lists(&[
        path.as_str(),
        current.as_str(),
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    ]);
    if merged != current {
        std::env::set_var("PATH", merged);
    }
}

pub(crate) fn extract_bootstrap_path(text: &str) -> Option<String> {
    let start = text.rfind("__COSH_PATH_BEGIN__")? + "__COSH_PATH_BEGIN__".len();
    let rest = &text[start..];
    let end = rest.find("__COSH_PATH_END__")?;
    let path = rest[..end].trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

pub(crate) fn merge_path_lists(paths: &[&str]) -> String {
    let mut seen = HashSet::new();
    let mut merged = Vec::new();
    for path in paths {
        for item in path.split(':') {
            if item.is_empty() {
                continue;
            }
            if seen.insert(item.to_string()) {
                merged.push(item.to_string());
            }
        }
    }
    merged.join(":")
}

pub(crate) fn build_adapter(kind: AdapterKind) -> cosh_shell::AdapterInstance {
    match adapter_for_kind(kind) {
        cosh_shell::AdapterInstance::ClaudeCode(adapter) => {
            cosh_shell::AdapterInstance::ClaudeCode(adapter.with_model_call(true))
        }
        cosh_shell::AdapterInstance::QwenCli(adapter) => {
            cosh_shell::AdapterInstance::QwenCli(adapter.with_model_call(true))
        }
        cosh_shell::AdapterInstance::CoshTui(adapter) => {
            cosh_shell::AdapterInstance::CoshTui(adapter.with_model_call(true))
        }
        other => other,
    }
}

pub(crate) fn passthrough_non_interactive(args: &[String]) -> Option<i32> {
    if args.get(1).map(String::as_str) == Some("--") {
        let Some(command) = args.get(2) else {
            eprintln!("cosh-shell: missing command after --");
            return Some(2);
        };
        let status = std::process::Command::new(command)
            .args(&args[3..])
            .status()
            .map(|s| s.code().unwrap_or(1))
            .unwrap_or_else(|err| {
                eprintln!("cosh-shell: exec {command} failed: {err}");
                126
            });
        return Some(status);
    }

    if args.iter().any(|a| a == "-c") {
        let shell = detect_passthrough_shell(args);
        let pass_args = passthrough_shell_args(args);
        let status = std::process::Command::new(&shell)
            .args(&pass_args)
            .status()
            .map(|s| s.code().unwrap_or(1))
            .unwrap_or_else(|err| {
                eprintln!("cosh-shell: exec {shell} failed: {err}");
                126
            });
        return Some(status);
    }

    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        let shell = detect_passthrough_shell(args);
        let pass_args = passthrough_shell_args(args);
        let status = std::process::Command::new(&shell)
            .args(&pass_args)
            .stdin(std::process::Stdio::inherit())
            .status()
            .map(|s| s.code().unwrap_or(1))
            .unwrap_or_else(|err| {
                eprintln!("cosh-shell: exec {shell} failed: {err}");
                126
            });
        return Some(status);
    }

    None
}

fn detect_passthrough_shell(args: &[String]) -> String {
    for (i, arg) in args.iter().enumerate() {
        if arg == "--shell" {
            if let Some(val) = args.get(i + 1) {
                return val.clone();
            }
        }
        if let Some(val) = arg.strip_prefix("--shell=") {
            return val.to_string();
        }
    }
    std::env::var("COSH_SHELL_DEFAULT_SHELL").unwrap_or_else(|_| "bash".to_string())
}

fn passthrough_shell_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--shell" => {
                let _ = iter.next();
            }
            "--isolated" => {}
            "--login" => out.push("-l".to_string()),
            _ if arg.starts_with("--shell=") => {}
            _ => out.push(arg.clone()),
        }
    }
    out
}

pub(crate) fn print_usage_help() {
    eprintln!(
        "Usage: cosh-shell [OPTIONS]\n\
         \n\
         AI-augmented interactive shell wrapper.\n\
         \n\
         Modes:\n\
          raw [adapter] [--run]   Interactive mode with AI (adapters: fake, claude, co, qwen, cosh-tui)\n\
           demo                    Demo with synthetic events\n\
         \n\
         Options:\n\
           -c <command>            Execute command and exit (passthrough to bash/zsh)\n\
           -- <command> [args...]   Execute command directly and exit\n\
           --shell <shell>         Use specified shell (bash, zsh) [default: bash]\n\
           --isolated              Isolated mode: skip user rcfiles\n\
           --login, -l             Treat as login shell\n\
           --version               Print version\n\
           --help                  Print help"
    );
}

#[cfg(test)]
mod tests {
    use super::{
        adapter_name_from_args, extract_bootstrap_path, merge_path_lists, parse_raw_shell,
        raw_shell_from_args, shell_from_default_or_auto, should_start_default_raw, RawShellKind,
    };

    #[test]
    fn raw_shell_selection_uses_explicit_arg_only() {
        assert_eq!(parse_raw_shell("/bin/zsh"), RawShellKind::Zsh);
        assert_eq!(parse_raw_shell("bash"), RawShellKind::Bash);
        assert_eq!(
            parse_raw_shell("/usr/local/bin/cosh-shell-zsh"),
            RawShellKind::Zsh
        );
        assert_eq!(
            parse_raw_shell("/usr/local/bin/cosh-shell-bash"),
            RawShellKind::Bash
        );
        assert_eq!(
            parse_raw_shell("/usr/bin/fish"),
            RawShellKind::Unsupported("fish".to_string())
        );
        assert_eq!(
            raw_shell_from_args(&["fake".to_string(), "--shell".to_string(), "zsh".to_string()]),
            Some(RawShellKind::Zsh)
        );
        assert_eq!(
            raw_shell_from_args(&[
                "fake".to_string(),
                "--shell=bash".to_string(),
                "--run".to_string()
            ]),
            Some(RawShellKind::Bash)
        );
        assert_eq!(
            raw_shell_from_args(&["fake".to_string(), "--run".to_string()]),
            None
        );
        assert_eq!(
            raw_shell_from_args(&["fake".to_string(), "--shell".to_string()]),
            Some(RawShellKind::MissingShellValue)
        );
        assert_eq!(
            raw_shell_from_args(&[
                "fake".to_string(),
                "--shell".to_string(),
                "--run".to_string()
            ]),
            Some(RawShellKind::MissingShellValue)
        );
        assert_eq!(
            adapter_name_from_args(&["--shell".to_string(), "zsh".to_string(), "qwen".to_string()]),
            Some("qwen")
        );
        assert_eq!(
            adapter_name_from_args(&["--shell".to_string(), "zsh".to_string(), "co".to_string()]),
            Some("co")
        );
    }

    #[test]
    fn raw_shell_default_uses_config_before_auto() {
        assert_eq!(shell_from_default_or_auto("zsh"), RawShellKind::Zsh);
        assert_eq!(shell_from_default_or_auto("/bin/bash"), RawShellKind::Bash);
        assert_eq!(
            shell_from_default_or_auto("/usr/bin/fish"),
            RawShellKind::Unsupported("fish".to_string())
        );
    }

    #[test]
    fn no_subcommand_interactive_raw_accepts_only_shell_entry_options() {
        assert!(should_start_default_raw(&[]));
        assert!(should_start_default_raw(&["--login".to_string()]));
        assert!(should_start_default_raw(&["-l".to_string()]));
        assert!(should_start_default_raw(&[
            "--shell".to_string(),
            "zsh".to_string(),
            "--isolated".to_string()
        ]));
        assert!(should_start_default_raw(&["--shell=bash".to_string()]));

        assert!(!should_start_default_raw(&["fake".to_string()]));
        assert!(!should_start_default_raw(&["--shell".to_string()]));
        assert!(!should_start_default_raw(&[
            "--shell".to_string(),
            "--isolated".to_string()
        ]));
        assert!(!should_start_default_raw(&["--unknown".to_string()]));
    }

    #[test]
    fn bootstrap_path_extracts_last_marked_value() {
        let text = "plugin noise\n__COSH_PATH_BEGIN__/a:/b__COSH_PATH_END__\n";
        assert_eq!(extract_bootstrap_path(text), Some("/a:/b".to_string()));
        assert_eq!(extract_bootstrap_path("plugin noise"), None);
    }

    #[test]
    fn bootstrap_path_merge_keeps_existing_and_common_dirs() {
        assert_eq!(
            merge_path_lists(&[
                "/opt/homebrew/bin:/usr/bin:/bin",
                "/usr/local/bin:/bin",
                "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            ]),
            "/opt/homebrew/bin:/usr/bin:/bin:/usr/local/bin:/usr/sbin:/sbin"
        );
    }
}
