use super::*;
use std::sync::mpsc;

const HOOK_TIMEOUT_MS: u64 = 2_000;
const HOOK_START_TIMEOUT_SECS: u64 = 10;
const MARKER_DELAY_SECS: u64 = 5;

/// Best-effort SIGKILL of recorded PIDs and their groups so a failed
/// assertion does not leak processes into CI.
struct HookPidCleanup(Vec<i32>);

impl Drop for HookPidCleanup {
    fn drop(&mut self) {
        for pid in &self.0 {
            let _ = Command::new("sh")
                .arg("-c")
                .arg(format!("kill -9 -- -{pid} {pid} 2>/dev/null"))
                .status();
        }
    }
}

fn install_user_hook(home: &Path, name: &str, body: &str) {
    let hooks_dir = home.join(".copilot-shell/cosh/hooks");
    fs::create_dir_all(&hooks_dir).expect("create user hooks directory");
    write_executable(&hooks_dir.join(name), body);
}

fn spawn_hook_session(
    home: &Path,
    command: &[u8],
) -> (thread::JoinHandle<String>, mpsc::Receiver<()>) {
    let home = home.to_path_buf();
    let command = command.to_vec();
    let (session_started_tx, session_started_rx) = mpsc::channel();
    let session = thread::spawn(move || {
        let home = home.to_string_lossy().into_owned();
        run_raw_cli_with_args_env_and_delayed_input_after_start(
            "fake",
            &[],
            &[("HOME", home.as_str())],
            vec![
                (command, Duration::ZERO),
                (b"exit\n".to_vec(), Duration::from_millis(2_500)),
            ],
            session_started_tx,
        )
    });
    (session, session_started_rx)
}

fn wait_for_pids(path: &Path, count: usize) -> Option<Vec<i32>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(HOOK_START_TIMEOUT_SECS);
    loop {
        if let Ok(text) = fs::read_to_string(path) {
            let pids: Vec<i32> = text
                .split_whitespace()
                .filter_map(|token| token.parse().ok())
                .collect();
            if pids.len() == count {
                return Some(pids);
            }
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Whether `pid` can still execute code. Zombies and dead entries cannot
/// produce the delayed marker even if their PID table entries remain.
fn process_can_run(pid: i32) -> bool {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("failed to run ps to check process state");
    let stat = String::from_utf8_lossy(&output.stdout);
    match stat.trim().chars().next() {
        Some('Z' | 'X') => false,
        Some(_) => true,
        None => {
            assert!(
                !output.status.success() && output.stderr.is_empty(),
                "ps failed without reporting that pid {pid} is absent: status={}, stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            false
        }
    }
}

fn assert_process_gone(pid: i32, context: &str) {
    let gone = (0..125).any(|_| {
        if process_can_run(pid) {
            thread::sleep(Duration::from_millis(20));
            false
        } else {
            true
        }
    });
    assert!(gone, "process {pid} survived {context}");
}

fn assert_marker_never_written(marker: &Path, ready_at: std::time::Instant, context: &str) {
    let deadline = ready_at + Duration::from_millis(5_500);
    if std::time::Instant::now() < deadline {
        thread::sleep(deadline - std::time::Instant::now());
    }
    assert!(!marker.exists(), "grandchild survived {context}");
}

#[test]
fn raw_cli_external_hook_timeout_kills_grandchildren() {
    let home = temp_shell_home("external-hook-group-kill");
    let marker = home.join("marker");
    let pid_file = home.join("pids");
    let body = format!(
        "#!/bin/sh\n# cosh-hook: group-kill-hook\n# match-commands: echo\n# timeout: {HOOK_TIMEOUT_MS}ms\n(sleep {MARKER_DELAY_SECS}; : > '{}') &\necho $$ $! > '{}'\nsleep 30\n",
        marker.display(),
        pid_file.display()
    );
    install_user_hook(&home, "group_kill.sh", &body);

    let (session, session_started) = spawn_hook_session(&home, b"echo hook-group-kill\n");
    session_started.recv().expect("raw CLI session to start");
    let pids = wait_for_pids(&pid_file, 2);
    let ready_at = std::time::Instant::now();
    let output = session.join().expect("join raw CLI hook session");
    let pids = pids.unwrap_or_else(|| panic!("hook did not record PIDs before timeout:\n{output}"));
    let _cleanup = HookPidCleanup(pids.clone());

    for pid in pids {
        assert_process_gone(pid, "the external hook timeout");
    }
    assert_marker_never_written(&marker, ready_at, "the external hook timeout");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn raw_cli_external_hook_ignoring_large_stdin_respects_deadline() {
    let home = temp_shell_home("external-hook-large-stdin");
    let pid_file = home.join("pid");
    let body = format!(
        "#!/bin/sh\n# cosh-hook: ignore-stdin-hook\n# match-commands: printf\n# timeout: {HOOK_TIMEOUT_MS}ms\necho $$ > '{}'\nsleep 30\n",
        pid_file.display()
    );
    install_user_hook(&home, "ignore_stdin.sh", &body);

    // One valid UTF-8 line larger than a pipe buffer makes the serialized
    // hook payload block when the hook never reads stdin.
    let (session, session_started) = spawn_hook_session(&home, b"printf '%1048576s' x\n");
    session_started.recv().expect("raw CLI session to start");
    let pids = wait_for_pids(&pid_file, 1);
    let ready_at = std::time::Instant::now();
    let output = session.join().expect("join raw CLI hook session");
    let pids =
        pids.unwrap_or_else(|| panic!("hook did not record its PID before timeout:\n{output}"));
    let _cleanup = HookPidCleanup(pids.clone());

    assert!(
        ready_at.elapsed() < Duration::from_secs(4),
        "blocked stdin exceeded the hook deadline:\n{output}"
    );
    assert_process_gone(pids[0], "the external hook stdin-write timeout");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn raw_cli_external_hook_stdout_holder_is_killed() {
    let home = temp_shell_home("external-hook-stdout-holder");
    let marker = home.join("marker");
    let pid_file = home.join("pids");
    let body = format!(
        "#!/bin/sh\n# cosh-hook: stdout-holder-hook\n# match-commands: echo\n# timeout: {HOOK_TIMEOUT_MS}ms\n(sleep {MARKER_DELAY_SECS}; : > '{}') &\necho $$ $! > '{}'\nexit 0\n",
        marker.display(),
        pid_file.display()
    );
    install_user_hook(&home, "stdout_holder.sh", &body);

    let (session, session_started) = spawn_hook_session(&home, b"echo hook-stdout-holder\n");
    session_started.recv().expect("raw CLI session to start");
    let pids = wait_for_pids(&pid_file, 2);
    let ready_at = std::time::Instant::now();
    let output = session.join().expect("join raw CLI hook session");
    let pids = pids.unwrap_or_else(|| panic!("hook did not record PIDs before timeout:\n{output}"));
    let _cleanup = HookPidCleanup(pids.clone());

    assert!(
        ready_at.elapsed() < Duration::from_secs(4),
        "stdout drain waited for the grandchild:\n{output}"
    );
    assert_process_gone(pids[1], "the external hook output-drain timeout");
    assert_marker_never_written(&marker, ready_at, "the output-drain timeout");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn raw_cli_external_hook_stdin_holder_is_killed() {
    let home = temp_shell_home("external-hook-stdin-holder");
    let marker = home.join("marker");
    let pid_file = home.join("pids");
    let body = format!(
        "#!/bin/sh\n# cosh-hook: stdin-holder-hook\n# match-commands: printf\n# timeout: {HOOK_TIMEOUT_MS}ms\nexec 3<&0\n(sleep {MARKER_DELAY_SECS}; : > '{}') <&3 >/dev/null 2>&1 &\necho $$ $! > '{}'\nexit 0\n",
        marker.display(),
        pid_file.display()
    );
    install_user_hook(&home, "stdin_holder.sh", &body);

    let (session, session_started) = spawn_hook_session(&home, b"printf '%1048576s' x\n");
    session_started.recv().expect("raw CLI session to start");
    let pids = wait_for_pids(&pid_file, 2);
    let ready_at = std::time::Instant::now();
    let output = session.join().expect("join raw CLI hook session");
    let pids = pids.unwrap_or_else(|| panic!("hook did not record PIDs before timeout:\n{output}"));
    let _cleanup = HookPidCleanup(pids.clone());

    assert!(
        ready_at.elapsed() < Duration::from_secs(4),
        "stdin delivery waited for the grandchild:\n{output}"
    );
    assert_process_gone(pids[1], "the external hook stdin timeout");
    assert_marker_never_written(&marker, ready_at, "the stdin timeout");
    let _ = fs::remove_dir_all(&home);
}
