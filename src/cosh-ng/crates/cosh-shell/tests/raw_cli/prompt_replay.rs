//! Regression coverage for prompt replay after slash panels: a real empty
//! Enter must repaint a fresh prompt line instead of being deduplicated as a
//! replayed prompt echo (issue #1698).

use super::*;
use std::path::PathBuf;

const PROMPT: &str = "cosh-replay$ ";

/// Temporary shell HOME with a pinned prompt; removed on drop so repeated
/// runs do not leak `cosh-raw-cli-paste-*` trees under the temp dir.
struct TempReplayHome {
    root: PathBuf,
    home: String,
    inputrc: String,
}

impl TempReplayHome {
    fn new(label: &str, inputrc: &str) -> Self {
        let root = temp_shell_home(label);
        let inputrc_path = root.join(".inputrc");
        fs::write(&inputrc_path, inputrc).expect("write test INPUTRC");
        // Non-isolated shells source the user rc files; pin a deterministic
        // prompt and delay PROMPT_COMMAND so the accept-line bytes (CRLF +
        // bracketed-paste toggle) and the next prompt arrive in separate PTY
        // reads, like a real shell with a non-trivial PROMPT_COMMAND.
        fs::write(
            root.join(".bashrc"),
            format!("PS1='{PROMPT}'\nPROMPT_COMMAND='sleep 0.05'\n"),
        )
        .expect("write test bashrc");
        fs::write(
            root.join(".zshrc"),
            format!("PROMPT='{PROMPT}'\nprecmd() {{ sleep 0.05; }}\n"),
        )
        .expect("write test zshrc");
        Self {
            home: root.to_string_lossy().into_owned(),
            inputrc: inputrc_path.to_string_lossy().into_owned(),
            root,
        }
    }

    /// Seeds bash history so `Up` recalls a slash command through the
    /// DEBUG-trap intercept path instead of the raw candidate relay.
    fn seed_bash_history(&self, line: &str) {
        fs::write(
            self.root.join(".bashrc"),
            format!(
                "PS1='{PROMPT}'\nPROMPT_COMMAND='sleep 0.05'\n\
                 export HISTFILE=\"$HOME/.bash_history\"\n\
                 export HISTSIZE=1000\nshopt -s histappend\n"
            ),
        )
        .expect("write history-enabled bashrc");
        fs::write(self.root.join(".bash_history"), format!("{line}\n"))
            .expect("write seeded history");
    }

    /// Emits hook output and then delays PROMPT_COMMAND long enough to outlast
    /// the idle-reconcile window before the PS1 paint.
    fn set_bash_prompt_command_delay(&self, seconds: &str) {
        fs::write(
            self.root.join(".bashrc"),
            format!(
                "PS1='{PROMPT}'\n\
                 PROMPT_COMMAND='printf prompt-hook-output; sleep {seconds}'\n"
            ),
        )
        .expect("write delayed-prompt bashrc");
    }
}

impl Drop for TempReplayHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Slice between the end of the skills panel and the sentinel echo, where the
/// empty-Enter response must appear.
fn between_panel_and_sentinel<'a>(output: &'a str, sentinel: &str) -> &'a str {
    between_marker_and_sentinel(output, "Skills", sentinel)
}

fn between_marker_and_sentinel<'a>(output: &'a str, marker: &str, sentinel: &str) -> &'a str {
    let panel = output.find(marker).expect("panel marker");
    let after_panel = &output[panel..];
    let sentinel_at = after_panel
        .find(sentinel)
        .map(|idx| panel + idx)
        .expect("sentinel echo");
    &output[panel..sentinel_at]
}

fn assert_no_prompt_run_on(output: &str, sentinel: &str) {
    let normalized = strip_ansi_escape(between_panel_and_sentinel(output, sentinel));
    for line in normalized.split(['\r', '\n']) {
        assert!(
            count_occurrences(line, PROMPT.trim_end()) <= 1,
            "two prompts written on one line: {line:?}\n{output:?}"
        );
    }
}

/// Asserts the empty Enter produced a fresh prompt line: two prompts after the
/// panel (the synthesized replay and the one bash paints after Enter), never
/// glued on one visible line, with a line break between them.
fn assert_empty_enter_repaints_prompt(output: &str, sentinel: &str) {
    let between = between_panel_and_sentinel(output, sentinel);
    let normalized = strip_ansi_escape(between);
    let prompt_positions = normalized
        .match_indices(PROMPT.trim_end())
        .map(|(idx, _)| idx)
        .collect::<Vec<_>>();
    assert!(
        prompt_positions.len() >= 2,
        "empty Enter did not repaint a prompt after the panel\n{output:?}"
    );
    for pair in prompt_positions.windows(2) {
        assert!(
            normalized[pair[0]..pair[1]].contains('\n'),
            "empty Enter CRLF was swallowed; prompts run on together\n{output:?}"
        );
    }
}

#[test]
fn raw_cli_bash_bracketed_paste_empty_enter_after_slash_panel_repaints_prompt() {
    let home = TempReplayHome::new("paste-on", "set enable-bracketed-paste on\n");
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[
            ("HOME", home.home.as_str()),
            ("INPUTRC", home.inputrc.as_str()),
            // The isolated-mode fallback writes the prompt inline and skips
            // the RestorePrompt replay path this regression must exercise.
            ("COSH_SHELL_ISOLATED", "0"),
        ],
        vec![
            // Let the initial prompt render before typing the slash command,
            // as a real user would.
            (
                b"/skills disable xlsx\r".to_vec(),
                Duration::from_millis(600),
            ),
            // Wait for the panel and the synthesized prompt replay to finish
            // before sending the lone empty Enter.
            (b"\r".to_vec(), Duration::from_millis(800)),
            (
                b"echo replay-sentinel-on\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    // Raw bytes: readline emits ESC[?2004l when the empty Enter is accepted;
    // the toggle must reach the outer terminal instead of being consumed as a
    // replay separator.
    let between = between_panel_and_sentinel(&output, "replay-sentinel-on");
    assert!(
        count_occurrences(between, "\u{1b}[?2004l") >= 1,
        "bracketed paste disable of the empty Enter was swallowed\n{output:?}"
    );
    assert_empty_enter_repaints_prompt(&output, "replay-sentinel-on");
    assert!(output.contains("replay-sentinel-on"), "{output}");
}

#[test]
fn raw_cli_bash_bracketed_paste_empty_enter_within_delay_window_is_not_swallowed() {
    let home = TempReplayHome::new("paste-delay", "set enable-bracketed-paste on\n");
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[
            ("HOME", home.home.as_str()),
            ("INPUTRC", home.inputrc.as_str()),
            ("COSH_SHELL_ISOLATED", "0"),
        ],
        vec![
            // The empty Enter rides in the same chunk as the slash command, so
            // it is relayed to bash while the panel still holds shell output.
            (
                b"/skills disable xlsx\r\r".to_vec(),
                Duration::from_millis(600),
            ),
            (
                b"echo replay-sentinel-delay\n".to_vec(),
                Duration::from_millis(800),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    let between = between_panel_and_sentinel(&output, "replay-sentinel-delay");
    assert!(
        count_occurrences(between, "\u{1b}[?2004l") >= 1,
        "bracketed paste disable of the early empty Enter was swallowed\n{output:?}"
    );
    assert!(
        between.contains("\r\r\n") || between.contains("\r\n"),
        "early empty Enter CRLF was swallowed\n{output:?}"
    );
    assert_no_prompt_run_on(&output, "replay-sentinel-delay");
    assert!(output.contains("replay-sentinel-delay"), "{output}");
}

#[test]
fn raw_cli_bash_recalled_slash_with_same_chunk_empty_enter_is_not_swallowed() {
    let home = TempReplayHome::new("paste-recall", "set enable-bracketed-paste on\n");
    // The recalled line reaches bash itself, so the intercept travels the
    // DEBUG-trap marker path (an `intercept` display cut, not a candidate
    // relay intercept); the trailing empty Enter shares the same PTY write.
    home.seed_bash_history("/skills disable xlsx");
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[
            ("HOME", home.home.as_str()),
            ("INPUTRC", home.inputrc.as_str()),
            ("COSH_SHELL_ISOLATED", "0"),
        ],
        vec![
            // Up-arrow recalls the seeded slash command from bash history.
            (b"\x1b[A".to_vec(), Duration::from_millis(600)),
            // Submit it and the empty Enter in one chunk (one relay write).
            (b"\r\r".to_vec(), Duration::from_millis(300)),
            (
                b"echo replay-sentinel-recall\n".to_vec(),
                Duration::from_millis(1000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    assert!(
        !output.contains("bash: /skills"),
        "recalled slash leaked to bash\n{output:?}"
    );
    let between = between_panel_and_sentinel(&output, "replay-sentinel-recall");
    assert!(
        count_occurrences(between, "\u{1b}[?2004l") >= 1,
        "bracketed paste disable of the empty Enter was swallowed\n{output:?}"
    );
    assert!(
        between.contains("\r\r\n") || between.contains("\r\n"),
        "empty Enter CRLF after the recalled slash was swallowed\n{output:?}"
    );
    assert_no_prompt_run_on(&output, "replay-sentinel-recall");
    assert!(output.contains("replay-sentinel-recall"), "{output}");
}

#[test]
fn raw_cli_bash_typeahead_empty_enter_during_failing_command_survives_prompt_restore() {
    let home = TempReplayHome::new("paste-typeahead", "set enable-bracketed-paste on\n");
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[
            ("HOME", home.home.as_str()),
            ("INPUTRC", home.inputrc.as_str()),
            ("COSH_SHELL_ISOLATED", "0"),
            ("COSH_SHELL_ANALYSIS_MODE", "auto"),
        ],
        vec![
            // A slightly delayed failing command that triggers the
            // failed-command prompt restore flow.
            (
                b"sh -c 'sleep 0.6; ls /nonexistent-replay-typeahead'\n".to_vec(),
                Duration::from_millis(600),
            ),
            // Empty Enter typed ahead while the command is still running: its
            // write event is drained before the command's precmd arrives.
            (b"\r".to_vec(), Duration::from_millis(300)),
            (
                b"echo replay-sentinel-typeahead\n".to_vec(),
                Duration::from_millis(2000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    // The failed-command flow restores the prompt after the command
    // completes; that restore must not swallow the queued empty Enter's
    // response.
    let between = between_marker_and_sentinel(
        &output,
        "No such file or directory",
        "replay-sentinel-typeahead",
    );
    assert!(
        count_occurrences(between, "\u{1b}[?2004l") >= 1,
        "bracketed paste disable of the typeahead empty Enter was swallowed\n{output:?}"
    );
    assert!(
        between.contains("\r\r\n") || between.contains("\r\n"),
        "typeahead empty Enter CRLF was swallowed\n{output:?}"
    );
    let normalized = strip_ansi_escape(between);
    assert!(
        normalized.contains(PROMPT.trim_end()),
        "typeahead empty Enter did not repaint a prompt\n{output:?}"
    );
    for line in normalized.split(['\r', '\n']) {
        assert!(
            count_occurrences(line, PROMPT.trim_end()) <= 1,
            "two prompts written on one line: {line:?}\n{output:?}"
        );
    }
    assert!(output.contains("replay-sentinel-typeahead"), "{output}");
}

#[test]
fn raw_cli_bash_ctrl_o_submission_with_typeahead_empty_enter_is_not_swallowed() {
    let home = TempReplayHome::new("paste-ctrl-o", "set enable-bracketed-paste on\n");
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[
            ("HOME", home.home.as_str()),
            ("INPUTRC", home.inputrc.as_str()),
            ("COSH_SHELL_ISOLATED", "0"),
            ("COSH_SHELL_ANALYSIS_MODE", "auto"),
        ],
        vec![
            // Submit a delayed failing command with Ctrl-O
            // (operate-and-get-next), bash's default non-Enter accept-line
            // binding, so the submission carries no CR/LF at all.
            (
                b"sh -c 'sleep 0.6; ls /nonexistent-replay-ctrl-o'\x0f".to_vec(),
                Duration::from_millis(600),
            ),
            // Empty Enter typed ahead while the command is still running.
            (b"\r".to_vec(), Duration::from_millis(300)),
            (
                b"echo replay-sentinel-ctrl-o\n".to_vec(),
                Duration::from_millis(2000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    // The prompt restore after the failed command must not treat the
    // Ctrl-O-submitted command's precmd as acknowledging the queued Enter.
    let between = between_marker_and_sentinel(
        &output,
        "No such file or directory",
        "replay-sentinel-ctrl-o",
    );
    assert!(
        count_occurrences(between, "\u{1b}[?2004l") >= 1,
        "bracketed paste disable of the typeahead empty Enter was swallowed\n{output:?}"
    );
    assert!(
        between.contains("\r\r\n") || between.contains("\r\n"),
        "typeahead empty Enter CRLF was swallowed\n{output:?}"
    );
    let normalized = strip_ansi_escape(between);
    assert!(
        normalized.contains(PROMPT.trim_end()),
        "typeahead empty Enter did not repaint a prompt\n{output:?}"
    );
    for line in normalized.split(['\r', '\n']) {
        assert!(
            count_occurrences(line, PROMPT.trim_end()) <= 1,
            "two prompts written on one line: {line:?}\n{output:?}"
        );
    }
    assert!(output.contains("replay-sentinel-ctrl-o"), "{output}");
}

#[test]
fn raw_cli_bash_slow_prompt_command_does_not_write_off_queued_enter() {
    let home = TempReplayHome::new("paste-slow-precmd", "set enable-bracketed-paste on\n");
    // The precmd marker fires before the user's PROMPT_COMMAND body and the
    // PS1 paint. The body first emits output (which must not count as a
    // painted prompt), then stays silent for 500ms — far past the 200ms
    // idle-reconcile window while the queued Enter is still unconsumed by
    // readline. The ledger must survive and the Enter response must pass.
    home.set_bash_prompt_command_delay("0.5");
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[
            ("HOME", home.home.as_str()),
            ("INPUTRC", home.inputrc.as_str()),
            ("COSH_SHELL_ISOLATED", "0"),
            ("COSH_SHELL_ANALYSIS_MODE", "auto"),
        ],
        vec![
            (
                b"sh -c 'sleep 0.6; ls /nonexistent-replay-slow-precmd'\n".to_vec(),
                Duration::from_millis(900),
            ),
            // Empty Enter typed ahead while the command still runs; it stays
            // queued through the whole PROMPT_COMMAND delay.
            (b"\r".to_vec(), Duration::from_millis(300)),
            (
                b"echo replay-sentinel-slow-precmd\n".to_vec(),
                Duration::from_millis(3000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    let between = between_marker_and_sentinel(
        &output,
        "No such file or directory",
        "replay-sentinel-slow-precmd",
    );
    assert!(
        count_occurrences(between, "\u{1b}[?2004l") >= 1,
        "bracketed paste disable of the queued Enter was swallowed during \
         a slow PROMPT_COMMAND\n{output:?}"
    );
    assert!(
        between.contains("\r\r\n") || between.contains("\r\n"),
        "queued Enter CRLF was swallowed during a slow PROMPT_COMMAND\n{output:?}"
    );
    let normalized = strip_ansi_escape(between);
    assert!(
        normalized.contains(PROMPT.trim_end()),
        "queued Enter did not repaint a prompt after the slow PROMPT_COMMAND\n{output:?}"
    );
    for line in normalized.split(['\r', '\n']) {
        assert!(
            count_occurrences(line, PROMPT.trim_end()) <= 1,
            "two prompts written on one line: {line:?}\n{output:?}"
        );
    }
    assert!(output.contains("replay-sentinel-slow-precmd"), "{output}");
}

#[test]
fn raw_cli_bash_replay_dedup_recovers_after_foreground_program_consumed_enter() {
    let home = TempReplayHome::new("paste-read", "set enable-bracketed-paste on\n");
    // The recalled slash reaches bash and is killed by the DEBUG trap, so
    // bash repaints a real prompt afterwards: replay dedup must arm to strip
    // it, making a stuck submission ledger visible as a duplicate prompt.
    home.seed_bash_history("/skills disable xlsx");
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[
            ("HOME", home.home.as_str()),
            ("INPUTRC", home.inputrc.as_str()),
            ("COSH_SHELL_ISOLATED", "0"),
        ],
        vec![
            // A foreground `read` consumes the second Enter itself: the
            // submission ledger would otherwise stay off by one forever.
            (b"read value\r".to_vec(), Duration::from_millis(600)),
            (b"hello\r".to_vec(), Duration::from_millis(400)),
            // Idle at the prompt long enough for the write-off, then recall
            // the slash from history (twice up: past this session's `read
            // value` entry): replay dedup must have recovered.
            (b"\x1b[A\x1b[A".to_vec(), Duration::from_millis(800)),
            (b"\r".to_vec(), Duration::from_millis(300)),
            (
                b"echo replay-sentinel-read\n".to_vec(),
                Duration::from_millis(1200),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    // With a stuck ledger the panel's prompt restore cannot arm, so bash's
    // late post-intercept repaint (prompt plus the recalled line text) is
    // painted a second time after the synthesized replay.
    let between = between_panel_and_sentinel(&output, "replay-sentinel-read");
    let normalized = strip_ansi_escape(between);
    let recalled = count_occurrences(&normalized, "/skills disable xlsx");
    assert!(
        recalled <= 1,
        "replay dedup stayed disabled after a foreground `read`: \
         the recalled line was painted {recalled} times after the panel\n{output:?}"
    );
    let prompts = count_occurrences(&normalized, PROMPT.trim_end());
    assert!(
        prompts <= 2,
        "replay dedup stayed disabled after a foreground `read`: \
         {prompts} prompts painted after the panel\n{output:?}"
    );
    assert!(output.contains("replay-sentinel-read"), "{output}");
}

#[test]
fn raw_cli_bash_bracketed_paste_off_empty_enter_after_slash_panel_repaints_prompt() {
    let home = TempReplayHome::new("paste-off", "set enable-bracketed-paste off\n");
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[
            ("HOME", home.home.as_str()),
            ("INPUTRC", home.inputrc.as_str()),
            ("COSH_SHELL_ISOLATED", "0"),
        ],
        vec![
            (
                b"/skills disable xlsx\r".to_vec(),
                Duration::from_millis(600),
            ),
            (b"\r".to_vec(), Duration::from_millis(800)),
            (
                b"echo replay-sentinel-off\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    let between = between_panel_and_sentinel(&output, "replay-sentinel-off");
    assert!(
        !between.contains("\u{1b}[?2004l"),
        "bracketed paste should be off in the baseline run\n{output:?}"
    );
    assert_empty_enter_repaints_prompt(&output, "replay-sentinel-off");
    assert!(output.contains("replay-sentinel-off"), "{output}");
}

#[test]
fn raw_cli_zsh_empty_enter_after_slash_panel_repaints_prompt() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let home = TempReplayHome::new("paste-zsh", "");
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        &[("HOME", home.home.as_str()), ("COSH_SHELL_ISOLATED", "0")],
        vec![
            (
                b"/skills disable xlsx\r".to_vec(),
                Duration::from_millis(600),
            ),
            (b"\r".to_vec(), Duration::from_millis(800)),
            (
                b"echo replay-sentinel-zsh\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    assert_no_prompt_run_on(&output, "replay-sentinel-zsh");
    assert!(output.contains("replay-sentinel-zsh"), "{output}");
    assert!(
        !output.contains("zsh: no such file or directory: /skills"),
        "{output}"
    );
}

/// Regression for issue #1811: after a bash slash command is intercepted, the
/// echoed command text must not be replayed again below the panel.
///
/// bash echoes user input before the DEBUG trap fires, so the display buffer
/// contains `prompt$ /skills detail\r\n` when the intercept marker arrives.
/// Without advancing `last_prompt_display_start` past that echo, RestorePrompt
/// would re-emit the command text on the line below the panel.
#[test]
fn raw_cli_bash_slash_intercept_does_not_replay_user_command_echo() {
    let home = TempReplayHome::new("intercept-echo", "set enable-bracketed-paste on\n");
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[
            ("HOME", home.home.as_str()),
            ("INPUTRC", home.inputrc.as_str()),
            ("COSH_SHELL_ISOLATED", "0"),
        ],
        vec![
            // Type and submit a slash command that renders a usage panel.
            (b"/skills detail\r".to_vec(), Duration::from_millis(600)),
            // Wait for the panel and any synthesized prompt replay to settle,
            // then run a sentinel command.
            (
                b"echo replay-sentinel-1811\n".to_vec(),
                Duration::from_millis(800),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    // The only prompt line that should carry the slash text is the original
    // user input; RestorePrompt must not duplicate it below the panel.
    let normalized = strip_ansi_escape(&output);
    let echoed_lines = normalized
        .lines()
        .filter(|line| line.starts_with(PROMPT.trim_end()) && line.contains("/skills detail"))
        .count();
    assert_eq!(
        echoed_lines, 1,
        "expected exactly one prompt line containing /skills detail (the user input); \
         RestorePrompt duplicated the echoed command\n{output:?}"
    );
    assert!(output.contains("replay-sentinel-1811"), "{output}");
}
