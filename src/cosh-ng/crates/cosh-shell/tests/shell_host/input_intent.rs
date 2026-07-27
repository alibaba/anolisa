use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt;
use std::process::Command as Process;
use std::time::{Duration, Instant};

const SHELL_INTENT_HELPERS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/shell_host/input_intent.sh"
));

fn shell_intent_helpers() -> String {
    let high_bytes = (128_u16..=255)
        .map(|byte| format!("    $'\\x{byte:02X}') _COSH_BYTE_AT_RESULT='{byte}'; return 0 ;;"))
        .collect::<Vec<_>>()
        .join("\n");
    SHELL_INTENT_HELPERS.replace("__COSH_HIGH_BYTE_CASES__", &high_bytes)
}

fn shell_available(shell: &str) -> bool {
    Process::new(shell).arg("--version").output().is_ok()
}

fn classify(shell: &str, input: impl AsRef<OsStr>, top_token: &str) -> Option<String> {
    classify_with_context(shell, input, top_token, ":", None)
}

fn classify_with_setup(
    shell: &str,
    input: impl AsRef<OsStr>,
    top_token: &str,
    setup: &str,
) -> Option<String> {
    classify_with_context(shell, input, top_token, setup, None)
}

fn classify_with_context(
    shell: &str,
    input: impl AsRef<OsStr>,
    top_token: &str,
    setup: &str,
    locale: Option<&str>,
) -> Option<String> {
    let mut command = Process::new(shell);
    if shell == "bash" {
        command.args(["--noprofile", "--norc"]);
    } else {
        command.arg("-f");
    }
    if let Some(locale) = locale {
        command.env("LANG", locale).env("LC_ALL", locale);
    }
    let script = format!(
        "{}\n{setup}\n_cosh_classify_missing \"$1\" \"$2\"",
        shell_intent_helpers(),
    );
    let output = command
        .args(["-c", &script, "cosh-intent-test"])
        .arg(input)
        .arg(top_token)
        .output()
        .ok()?;
    assert!(
        output.status.success(),
        "{shell}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[test]
fn classification_is_locale_independent() {
    for shell in ["bash", "zsh"] {
        if !shell_available(shell) {
            continue;
        }
        for locale in ["C", "C.UTF-8"] {
            for (input, top_token, expected) in [
                ("Just do it", "Just", "natural_language"),
                (
                    "\u{5e2e}\u{6211}\u{770b}",
                    "\u{5e2e}\u{6211}\u{770b}",
                    "natural_language",
                ),
                (
                    "\u{3042}\u{308a}\u{304c}\u{3068}\u{3046}",
                    "\u{3042}\u{308a}\u{304c}\u{3068}\u{3046}",
                    "ambiguous",
                ),
            ] {
                assert_eq!(
                    classify_with_context(shell, input, top_token, ":", Some(locale)).as_deref(),
                    Some(expected),
                    "{shell}: {locale}: {input:?}"
                );
            }
        }
    }
}

#[test]
fn classification_ignores_user_ifs() {
    for shell in ["bash", "zsh"] {
        if !shell_available(shell) {
            continue;
        }
        assert_eq!(
            classify_with_setup(shell, "Just do it", "Just", "IFS=:").as_deref(),
            Some("natural_language"),
            "{shell}"
        );
    }
}

#[test]
fn long_ascii_input_avoids_per_byte_subprocess_cost() {
    let input = "a".repeat(4096);
    for shell in ["bash", "zsh"] {
        if !shell_available(shell) {
            continue;
        }
        let started = Instant::now();
        assert_eq!(
            classify(shell, &input, "a").as_deref(),
            Some("ambiguous"),
            "{shell}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{shell}: {:?}",
            started.elapsed()
        );
    }
}

#[test]
fn max_non_ascii_input_avoids_per_byte_subprocess_cost() {
    let input = "é".repeat(2048);
    for shell in ["bash", "zsh"] {
        if !shell_available(shell) {
            continue;
        }
        let started = Instant::now();
        assert_eq!(
            classify(shell, &input, &input).as_deref(),
            Some("ambiguous"),
            "{shell}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{shell}: {:?}",
            started.elapsed()
        );
    }
}

#[test]
fn input_limit_is_measured_in_bytes() {
    let within_limit = format!("{}a", "\u{5e2e}".repeat(1365));
    let over_limit = "\u{5e2e}".repeat(1366);

    for shell in ["bash", "zsh"] {
        if !shell_available(shell) {
            continue;
        }
        for locale in ["C", "C.UTF-8"] {
            assert_eq!(within_limit.len(), 4096);
            assert_eq!(
                classify_with_context(shell, &within_limit, &within_limit, ":", Some(locale))
                    .as_deref(),
                Some("natural_language"),
                "{shell}: {locale}: 4096 bytes"
            );
            assert_eq!(over_limit.len(), 4098);
            assert_eq!(
                classify_with_context(shell, &over_limit, &over_limit, ":", Some(locale))
                    .as_deref(),
                Some("unsafe"),
                "{shell}: {locale}: 4098 bytes"
            );
        }
    }
}

fn assert_bash_zsh(input: &str, top_token: &str, expected: &str) {
    for shell in ["bash", "zsh"] {
        if !shell_available(shell) {
            continue;
        }
        assert_eq!(
            classify(shell, input, top_token).as_deref(),
            Some(expected),
            "{shell}: {input:?}"
        );
    }
}

#[test]
fn strong_natural_language_matrix_is_consistent() {
    for (input, top_token) in [
        ("Who are you", "Who"),
        ("how file", "how"),
        ("please explain this", "please"),
        ("can you check this", "can"),
        ("is this safe", "is"),
        ("review this file", "review"),
        ("I need help", "I"),
        (
            "\u{5e2e}\u{6211}\u{770b}\u{770b}\u{5f53}\u{524d}\u{76ee}\u{5f55}",
            "\u{5e2e}\u{6211}\u{770b}\u{770b}\u{5f53}\u{524d}\u{76ee}\u{5f55}",
        ),
        (
            "\u{5e2e}\u{6211}\u{770b} ./\u{65e5}\u{5fd7}",
            "\u{5e2e}\u{6211}\u{770b}",
        ),
        ("review ./report.log", "review"),
    ] {
        assert_bash_zsh(input, top_token, "natural_language");
    }
}

#[test]
fn composed_natural_language_matrix_is_consistent() {
    for (input, top_token) in [
        ("Just do it", "Just"),
        ("please just check this", "please"),
        ("maybe explain this", "maybe"),
        ("simply tell me why", "simply"),
        ("go ahead", "go"),
        ("try again", "try"),
        ("keep going", "keep"),
        ("carry on", "carry"),
        ("never mind", "never"),
        ("forget it", "forget"),
        ("thank you", "thank"),
        ("good morning", "good"),
        ("do this", "do"),
        ("run the tests", "run"),
        ("find the problem", "find"),
        ("open this file", "open"),
        ("read the logs", "read"),
        ("create a report", "create"),
        ("update the config", "update"),
        ("restart the service", "restart"),
        ("I am stuck", "I"),
        ("I think this failed", "I"),
        ("we have a problem", "we"),
        ("you should retry", "you"),
        ("this is broken", "this"),
        ("that looks wrong", "that"),
        ("it failed again", "it"),
        ("there is an error", "there"),
        ("need some help", "need"),
        ("want more details", "want"),
        ("any ideas", "any"),
        ("nginx is down", "nginx"),
        ("the build failed", "the"),
        ("my command does not work", "my"),
        ("service not running", "service"),
        ("everything looks fine", "everything"),
        ("let us check", "let"),
        ("hello there", "hello"),
        ("yes please", "yes"),
        ("thanks again", "thanks"),
        ("not working", "not"),
        ("why?", "why"),
    ] {
        assert_bash_zsh(input, top_token, "natural_language");
    }
}

#[test]
fn command_veto_matrix_is_consistent() {
    for (input, top_token) in [
        (
            "./\u{4e2d}\u{6587}\u{811a}\u{672c}",
            "./\u{4e2d}\u{6587}\u{811a}\u{672c}",
        ),
        (
            "\u{5e2e}\u{6211}\u{770b} \"$PATH\"",
            "\u{5e2e}\u{6211}\u{770b}",
        ),
        (
            "\u{5e2e}\u{6211}\u{770b} ./*.log",
            "\u{5e2e}\u{6211}\u{770b}",
        ),
        ("review --all", "review"),
        ("review FOO=bar", "review"),
        ("review this | cat", "review"),
        ("command review this", "command"),
        ("Who are you??", "Who"),
    ] {
        assert_bash_zsh(input, top_token, "command");
    }
}

#[test]
fn ambiguous_tool_inputs_remain_shell_owned() {
    for (input, top_token) in [
        ("ok", "ok"),
        ("hello", "hello"),
        ("thanks", "thanks"),
        ("continue", "continue"),
        ("just", "just"),
        ("just build", "just"),
        ("just cargo test", "just"),
        ("just terraform plan", "just"),
        ("maybe kubectl get pods", "maybe"),
        ("simply deploy", "simply"),
        ("go test", "go"),
        ("try build", "try"),
        ("nginx status", "nginx"),
        ("the build", "the"),
        ("terraform plan", "terraform"),
        ("kubectl get pods", "kubectl"),
        ("make deploy", "make"),
        (
            "\u{3042}\u{308a}\u{304c}\u{3068}\u{3046}",
            "\u{3042}\u{308a}\u{304c}\u{3068}\u{3046}",
        ),
        ("\u{1f642}", "\u{1f642}"),
    ] {
        assert_bash_zsh(input, top_token, "ambiguous");
    }
}

#[test]
fn invalid_utf8_is_unsafe() {
    let input = OsString::from_vec(vec![b'r', b'e', b'v', b'i', b'e', b'w', b' ', 0xff]);
    for shell in ["bash", "zsh"] {
        if !shell_available(shell) {
            continue;
        }
        assert_eq!(
            classify(shell, &input, "review").as_deref(),
            Some("unsafe"),
            "{shell}"
        );
    }
}

#[test]
fn han_core_boundaries_are_locale_independent() {
    for codepoint in [
        0x3400, 0x4dbf, 0x4e00, 0x9fff, 0xf900, 0xfaff, 0x20000, 0x323af,
    ] {
        let input = char::from_u32(codepoint).expect("valid Han codepoint");
        assert_bash_zsh(&input.to_string(), &input.to_string(), "natural_language");
    }
    for codepoint in [0x33ff, 0x4dc0, 0x10000, 0x323b0] {
        let input = char::from_u32(codepoint).expect("valid non-Han codepoint");
        assert_bash_zsh(&input.to_string(), &input.to_string(), "ambiguous");
    }
}
