use std::ffi::OsString;

use asc_cli::{Cli, InputError, output::render_policy};
use asc_daemon_protocol::{DaemonResponse, RequestId};
use serde_json::{Value, json};

mod common;

#[test]
fn all_fifteen_commands_match_frozen_wire_parameters() {
    let directory = common::Directory::new();
    let methods: Value = serde_json::from_str(common::METHODS).unwrap();
    let mut covered = std::collections::BTreeSet::new();
    for row in methods.as_array().unwrap() {
        let mut args = vec![OsString::from("agent-sec-cli")];
        args.extend(common::args_for(
            row,
            &directory.0,
            &directory.0.join("socket"),
        ));
        let actual =
            serde_json::to_value(Cli::parse_from(args).unwrap().request().unwrap()).unwrap();
        assert_eq!(
            actual,
            json!({"method": row["method"], "params": row["canonicalParams"]})
        );
        covered.insert(row["method"].as_str().unwrap());
    }
    assert_eq!(
        covered,
        asc_daemon_protocol::method::PAP_METHODS
            .into_iter()
            .collect()
    );
}

#[test]
fn equals_syntax_option_looking_values_and_os_native_paths_are_preserved() {
    use std::os::unix::ffi::OsStringExt as _;
    let directory = common::Directory::new();
    let file = directory
        .0
        .join(OsString::from_vec(b"policy-\xff.json".to_vec()));
    std::fs::write(
        &file,
        br#"{"kind":"prevent_file_deletion","files":["/work/a b"]}"#,
    )
    .unwrap();
    let args: Vec<OsString> = vec![
        "agent-sec-cli".into(),
        "policy".into(),
        "create".into(),
        "--name=--help".into(),
        "--file".into(),
        file.into_os_string(),
        "--socket=/run/asc.sock".into(),
    ];
    let request = Cli::parse_from(args).unwrap().request().unwrap();
    assert_eq!(request.params["policyName"], "--help");
    assert_eq!(request.params["template"]["files"][0], "/work/a b");
}

#[test]
fn invalid_and_ambiguous_options_are_usage_errors() {
    let cases = [
        vec!["policy", "get", "--policy-id", "p"],
        vec!["policy", "get", "--policy-id", "p", "--revision", "0"],
        vec!["policy", "get", "--policy-id", "", "--revision", "1"],
        vec!["policy", "list", "--limit", "1001"],
        vec!["policy", "list", "--limit", "0"],
        vec!["scope", "list", "--offset", "4294967296"],
        vec!["scope", "list", "--offset", "-1"],
        vec!["scope", "create"],
        vec!["scope", "create", "--pid", "0"],
        vec!["scope", "create", "--cgroup-id", "0"],
        vec!["scope", "create", "--pid", "1", "--cgroup-id", "2"],
        vec!["scope", "create", "--pid", "1", "--pid", "2"],
        vec!["policy", "list", "--timeout-ms", "0"],
        vec!["--timeout-ms", "100", "policy", "list", "--timeout-ms=200"],
        vec!["policy", "list", "--unknown"],
        vec!["policy", "list", "--role", "admin"],
        vec!["binding", "get", "--binding-id", "b", "--revision", "1"],
        vec!["policy", "create", "--name", "--help", "--file", "x"],
        vec!["policy", "list", "--socket", "/run/other.sock"],
    ];
    for case in cases {
        let mut args = vec!["agent-sec-cli", "--socket", "/run/asc.sock"];
        args.extend(case);
        let error = Cli::parse_from(args.clone()).unwrap_err();
        assert!(error.use_stderr(), "{args:?} unexpectedly produced help");
    }
    assert!(Cli::parse_from(["agent-sec-cli", "policy", "list"]).is_err());
    assert!(
        Cli::parse_from([
            "agent-sec-cli",
            "--socket",
            "relative.sock",
            "policy",
            "list"
        ])
        .is_err()
    );
}

#[test]
fn repeated_socket_options_reject_non_utf8_inline_paths_at_every_level() {
    use std::os::unix::ffi::OsStringExt as _;

    let native = OsString::from_vec(b"--socket=/run/asc-\xff.sock".to_vec());
    let endpoints = [
        vec![native.clone()],
        vec!["--socket=/run/other.sock".into()],
        vec!["--socket".into(), "/run/other.sock".into()],
    ];
    for endpoint in endpoints {
        for (first, second) in [
            (vec![native.clone()], endpoint.clone()),
            (endpoint, vec![native.clone()]),
        ] {
            for position in [1, 2] {
                let mut args = first.clone();
                args.extend(["policy", "list"][..position].iter().map(OsString::from));
                args.extend(second.clone());
                args.extend(["policy", "list"][position..].iter().map(OsString::from));
                let error = Cli::parse_from(
                    std::iter::once(OsString::from("agent-sec-cli")).chain(args.clone()),
                )
                .unwrap_err();
                assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
                let output = common::run(&args);
                assert_eq!(output.status.code(), Some(2), "{args:?}");
                assert!(output.stdout.is_empty());
                assert!(
                    String::from_utf8_lossy(&output.stderr)
                        .contains("--socket may be specified only once")
                );
            }
        }
    }

    let cli = Cli::parse_from(vec![
        "agent-sec-cli".into(),
        native,
        "policy".into(),
        "list".into(),
    ])
    .unwrap();
    assert_eq!(
        cli.socket.into_os_string(),
        OsString::from_vec(b"/run/asc-\xff.sock".to_vec())
    );
}

#[test]
fn file_errors_duplicate_keys_and_oversized_inputs_are_local_failures() {
    let directory = common::Directory::new();
    let path = directory.0.join("template.json");
    let args = [
        "agent-sec-cli",
        "--socket",
        "/run/absent.sock",
        "policy",
        "create",
        "--name",
        "test",
        "--file",
        path.to_str().unwrap(),
    ];
    assert!(matches!(
        Cli::parse_from(args).unwrap().request(),
        Err(InputError::Read(_))
    ));
    for bytes in [
        b"not-json".as_slice(),
        br#"{"kind":"prevent_file_deletion","files":[],"files":["/etc"]}"#,
        br#"{"kind":"prevent_file_deletion","kind":"high_sensitivity_read_deny","files":["/etc"]}"#,
        br#"{"kind":"low_sensitivity_egress","files":["/etc"],"trustedDestinations":[{"type":"host","pattern":"one","pattern":"two","ports":[443]}]}"#,
        br#"{"kind":"prevent_file_deletion","files":[],"extra":true}"#,
    ] {
        std::fs::write(&path, bytes).unwrap();
        assert!(matches!(
            Cli::parse_from(args).unwrap().request(),
            Err(InputError::Json(_))
        ));
    }
    std::fs::write(&path, vec![b' '; 4 * 1024 * 1024 + 1]).unwrap();
    assert!(matches!(
        Cli::parse_from(args).unwrap().request(),
        Err(InputError::TooLarge)
    ));
}

#[test]
fn result_rendering_keeps_domains_and_errors_separate() {
    let response = DaemonResponse::success(
        RequestId::new("r1").unwrap(),
        json!({"status":"PENDING_APPLY"}),
    );
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    assert_eq!(
        render_policy(&response, &mut stdout, &mut stderr).unwrap(),
        0
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&stdout).unwrap(),
        json!({"status":"PENDING_APPLY"})
    );
    assert!(stderr.is_empty());
    stdout.clear();
    let response: DaemonResponse = DaemonResponse::error(
        RequestId::new("r2").unwrap(),
        "not_found",
        "Policy not found",
    );
    assert_eq!(
        render_policy(&response, &mut stdout, &mut stderr).unwrap(),
        1
    );
    assert!(stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&stderr).unwrap(),
        json!({"requestId":"r2","error":{"code":"not_found","message":"Policy not found"}})
    );
}

#[test]
fn binary_help_version_and_failures_have_stable_exit_codes() {
    let mut help_cases = vec![vec!["--help"], vec!["--version"]];
    for resource in ["policy", "scope", "binding"] {
        help_cases.push(vec![resource, "--help"]);
        for operation in ["create", "get", "list", "update", "delete"] {
            help_cases.push(vec![resource, operation, "--help"]);
        }
    }
    for args in help_cases {
        let output = common::run(&args.iter().map(OsString::from).collect::<Vec<_>>());
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        if args == ["--version"] {
            assert_eq!(
                stdout,
                format!("agent-sec-cli {}\n", env!("CARGO_PKG_VERSION"))
            );
        } else {
            assert!(stdout.contains("Usage: agent-sec-cli"), "{stdout}");
        }
        assert!(output.stderr.is_empty());
    }
    let directory = common::Directory::new();
    let socket = directory.0.join("absent.sock");
    for (args, code, message) in [
        (
            vec!["--socket", socket.to_str().unwrap(), "policy", "list"],
            1,
            "connection unavailable",
        ),
        (vec!["policy", "list"], 2, "--socket"),
        (
            vec![
                "--socket",
                socket.to_str().unwrap(),
                "scope",
                "create",
                "--pid",
                "0",
            ],
            2,
            "invalid value",
        ),
        (
            vec![
                "--socket",
                socket.to_str().unwrap(),
                "policy",
                "create",
                "--name",
                "x",
                "--file",
                "/no-such-policy-input",
            ],
            1,
            "cannot read Policy template",
        ),
    ] {
        let output = common::run(&args.iter().map(OsString::from).collect::<Vec<_>>());
        assert_eq!(output.status.code(), Some(code));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(message));
        if code == 1 {
            assert!(stderr.starts_with("agent-sec-cli: "), "{stderr}");
        }
    }
    assert!(!socket.exists());
}
