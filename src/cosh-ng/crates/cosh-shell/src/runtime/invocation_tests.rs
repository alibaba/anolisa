use super::*;
use std::ffi::{OsStr, OsString};

const ALL_TTY: (bool, bool, bool) = (true, true, true);

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn classify(argv0: &str, args: &[&str], tty: (bool, bool, bool)) -> Invocation {
    classify_invocation(OsStr::new(argv0), &os(args), tty.0, tty.1, tty.2)
}

fn exec_args(invocation: Invocation) -> Vec<OsString> {
    match invocation {
        Invocation::ExecShell(plan) => plan.args,
        Invocation::Tui(entry) => panic!("expected ExecShell, got Tui({entry:?})"),
    }
}

#[test]
fn agent_namespace_builds_gateway_plan_without_the_namespace_token() {
    assert_eq!(
        gateway_plan(&os(&["agent", "task", "get", "task-1"])),
        Some(GatewayPlan {
            args: os(&["task", "get", "task-1"]),
        })
    );
    assert_eq!(gateway_plan(&os(&["agentic"])), None);
    assert_eq!(gateway_plan(&[]), None);
}

#[test]
fn cosh_entry_matches_only_the_cosh_basename() {
    for argv0 in ["cosh", "-cosh", "/usr/bin/cosh", "./cosh"] {
        assert!(is_cosh_entry(OsStr::new(argv0)), "argv0 {argv0}");
    }
    for argv0 in ["cosh-shell", "-cosh-shell", "/usr/libexec/cosh-shell", ""] {
        assert!(!is_cosh_entry(OsStr::new(argv0)), "argv0 {argv0}");
    }
}

#[test]
fn raw_subcommand_enters_tui_and_is_marked_for_launch_normalization() {
    for args in [
        vec!["raw"],
        vec!["raw", "cosh-core"],
        vec!["raw", "--shell", "zsh", "cosh-core", "--resume"],
    ] {
        assert_eq!(
            classify("cosh", &args, ALL_TTY),
            Invocation::Tui(TuiEntry {
                login: false,
                launch_args: os(&args[1..]),
            }),
            "args {args:?}"
        );
    }

    assert!(matches!(
        classify("cosh", &["raw", "cosh-core"], (false, true, true)),
        Invocation::Tui(TuiEntry { launch_args, .. }) if launch_args == os(&["cosh-core"])
    ));
}

#[test]
fn raw_non_interactive_escape_hatches_keep_their_legacy_passthrough() {
    for (args, expected) in [
        (
            vec!["raw", "cosh-core", "-c", "echo ok"],
            vec!["-c", "echo ok"],
        ),
        (vec!["raw", "--", "echo", "ok"], vec!["--", "echo", "ok"]),
    ] {
        assert_eq!(
            exec_args(classify("cosh", &args, ALL_TTY)),
            os(&expected),
            "args {args:?}"
        );
    }
}
