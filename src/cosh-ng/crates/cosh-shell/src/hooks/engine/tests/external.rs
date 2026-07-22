use super::*;

#[cfg(unix)]
#[test]
fn external_hook_nonzero_exit_is_no_finding() {
    let (dir, path) = write_executable_hook(
        "cosh_hook_test_external_nonzero",
        "nonzero.sh",
        "#!/bin/sh\n# cosh-hook: nonzero-hook\n# match-commands: echo\nprintf '{\"hook_id\":\"nonzero-hook\",\"severity\":\"warning\",\"title\":\"t\",\"description\":\"d\",\"suggestion\":\"s\"}'\nexit 7\n",
    );
    let config = parse_hook_header(&path).unwrap();
    let mut engine = HookEngine::new();
    engine.register_external(config);

    assert!(engine.evaluate(&make_block("echo hi")).is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn external_hook_malformed_json_is_no_finding() {
    let (dir, path) = write_executable_hook(
        "cosh_hook_test_external_malformed",
        "malformed.sh",
        "#!/bin/sh\n# cosh-hook: malformed-hook\n# match-commands: echo\nprintf 'not-json'\n",
    );
    let config = parse_hook_header(&path).unwrap();
    let mut engine = HookEngine::new();
    engine.register_external(config);

    assert!(engine.evaluate(&make_block("echo hi")).is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn external_hook_empty_or_stderr_only_output_is_no_finding() {
    let (empty_dir, empty_path) = write_executable_hook(
        "cosh_hook_test_external_empty",
        "empty.sh",
        "#!/bin/sh\n# cosh-hook: empty-hook\n# match-commands: echo\n",
    );
    let (stderr_dir, stderr_path) = write_executable_hook(
        "cosh_hook_test_external_stderr",
        "stderr.sh",
        "#!/bin/sh\n# cosh-hook: stderr-hook\n# match-commands: echo\necho noisy >&2\n",
    );
    let mut engine = HookEngine::new();
    engine.register_external(parse_hook_header(&empty_path).unwrap());
    engine.register_external(parse_hook_header(&stderr_path).unwrap());

    assert!(engine.evaluate(&make_block("echo hi")).is_empty());

    let _ = fs::remove_dir_all(&empty_dir);
    let _ = fs::remove_dir_all(&stderr_dir);
}

#[cfg(unix)]
#[test]
fn external_hook_timeout_is_killed_and_no_finding() {
    let (dir, path) = write_executable_hook(
        "cosh_hook_test_external_timeout",
        "timeout.sh",
        "#!/bin/sh\n# cosh-hook: timeout-hook\n# match-commands: echo\n# timeout: 20ms\nsleep 2\nprintf '{\"hook_id\":\"timeout-hook\",\"severity\":\"warning\",\"title\":\"t\",\"description\":\"d\",\"suggestion\":\"s\"}'\n",
    );
    let config = parse_hook_header(&path).unwrap();
    let mut engine = HookEngine::new();
    engine.register_external(config);

    let started = std::time::Instant::now();
    assert!(engine.evaluate(&make_block("echo hi")).is_empty());
    assert!(started.elapsed() < std::time::Duration::from_secs(1));

    let _ = fs::remove_dir_all(&dir);
}

/// Best-effort SIGKILL of recorded PIDs and their groups so a failed
/// assertion does not leak processes into CI.
#[cfg(unix)]
struct PidCleanup(Vec<i32>);

#[cfg(unix)]
impl Drop for PidCleanup {
    fn drop(&mut self) {
        for pid in &self.0 {
            let _ = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("kill -9 -- -{pid} {pid} 2>/dev/null"))
                .status();
        }
    }
}

/// Whether `pid` can still execute code. Zombie (Z) and dead (X) states
/// count as terminated: SIGKILL already landed but the parent has not
/// reaped the entry yet, and `kill -0` would still report such a PID as
/// alive.
///
/// Fails closed: an unrunnable or misbehaving `ps` panics instead of
/// letting the liveness assertion pass vacuously.
#[cfg(unix)]
fn process_can_run(pid: i32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("failed to run ps to check process state");
    let stat = String::from_utf8_lossy(&output.stdout);
    match stat.trim().chars().next() {
        Some('Z' | 'X') => false,
        Some(_) => true,
        // No stat line: ps signals "no such process" via non-zero exit.
        // A successful exit without output is a ps anomaly and must fail
        // the test rather than report the PID as gone.
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

#[cfg(unix)]
#[test]
fn external_hook_timeout_kills_grandchildren() {
    use std::time::Duration;

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let scratch = std::env::temp_dir().join(format!(
        "cosh_hook_test_group_kill_scratch-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&scratch).unwrap();
    let marker = scratch.join("marker");
    let pid_file = scratch.join("pids");

    // The hook backgrounds a grandchild that writes the marker after 5s,
    // records `<script-pid> <grandchild-pid>`, then blocks past the timeout.
    let body = format!(
        "#!/bin/sh\n# cosh-hook: group-kill-hook\n# match-commands: echo\n# timeout: 300ms\n(sleep 5; : > '{}') &\necho $$ $! > '{}'\nsleep 30\n",
        marker.display(),
        pid_file.display()
    );
    let (dir, path) = write_executable_hook("cosh_hook_test_external_group_kill", "leak.sh", &body);
    let config = parse_hook_header(&path).unwrap();
    let mut engine = HookEngine::new();
    engine.register_external(config);

    let started = std::time::Instant::now();
    assert!(engine.evaluate(&make_block("echo hi")).is_empty());

    let mut recorded: Option<Vec<i32>> = None;
    for _ in 0..100 {
        if let Ok(text) = fs::read_to_string(&pid_file) {
            let pids: Vec<i32> = text
                .split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect();
            if pids.len() == 2 {
                recorded = Some(pids);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let pids = recorded.expect("pid file was never fully written");
    let _cleanup = PidCleanup(pids.clone());

    // Both the script and its grandchild must be gone within 2.5s, well
    // before the marker would have been written.
    for pid in &pids {
        let gone = (0..125).any(|_| {
            if process_can_run(*pid) {
                std::thread::sleep(Duration::from_millis(20));
                false
            } else {
                true
            }
        });
        assert!(gone, "process {pid} survived the hook timeout kill");
    }

    let budget = Duration::from_millis(5500);
    if started.elapsed() < budget {
        std::thread::sleep(budget - started.elapsed());
    }
    assert!(!marker.exists(), "grandchild survived the hook timeout");

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&scratch);
}

#[cfg(unix)]
#[test]
fn external_hook_ignoring_large_stdin_respects_deadline() {
    use std::time::Duration;

    // A single preview line larger than any pipe buffer: the old
    // implementation blocked in the stdin write before the timeout started.
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let scratch = std::env::temp_dir().join(format!(
        "cosh_hook_test_large_stdin_scratch-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&scratch).unwrap();
    let output_file = scratch.join("output.txt");
    fs::write(&output_file, "x".repeat(1 << 20)).unwrap();

    let (dir, path) = write_executable_hook(
        "cosh_hook_test_external_large_stdin",
        "ignore_stdin.sh",
        "#!/bin/sh\n# cosh-hook: ignore-stdin-hook\n# match-commands: echo\n# timeout: 300ms\nsleep 30\n",
    );
    let mut engine = HookEngine::new();
    engine.register_external(parse_hook_header(&path).unwrap());

    let mut block = make_block("echo hi");
    block.output.terminal_output_ref = Some(output_file.to_string_lossy().into_owned());
    block.output.terminal_output_bytes = 1 << 20;

    let started = std::time::Instant::now();
    assert!(engine.evaluate(&block).is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "stdin write must be bounded by the hook deadline"
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&scratch);
}

#[cfg(unix)]
#[test]
fn external_hook_stdout_holding_grandchild_is_killed_after_parent_exit() {
    use std::time::Duration;

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let scratch = std::env::temp_dir().join(format!(
        "cosh_hook_test_stdout_holder_scratch-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&scratch).unwrap();
    let marker = scratch.join("marker");
    let pid_file = scratch.join("pids");

    // The hook exits successfully at once; its backgrounded grandchild
    // inherits stdout and keeps the pipe open past the deadline.
    let body = format!(
        "#!/bin/sh\n# cosh-hook: stdout-holder-hook\n# match-commands: echo\n# timeout: 300ms\n(sleep 5; : > '{}') &\necho $$ $! > '{}'\nexit 0\n",
        marker.display(),
        pid_file.display()
    );
    let (dir, path) = write_executable_hook(
        "cosh_hook_test_external_stdout_holder",
        "stdout_holder.sh",
        &body,
    );
    let mut engine = HookEngine::new();
    engine.register_external(parse_hook_header(&path).unwrap());

    let started = std::time::Instant::now();
    assert!(engine.evaluate(&make_block("echo hi")).is_empty());
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "stdout drain must return at the deadline, not when the grandchild exits"
    );

    let mut recorded: Option<Vec<i32>> = None;
    for _ in 0..100 {
        if let Ok(text) = fs::read_to_string(&pid_file) {
            let pids: Vec<i32> = text
                .split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect();
            if pids.len() == 2 {
                recorded = Some(pids);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let pids = recorded.expect("pid file was never fully written");
    let _cleanup = PidCleanup(pids.clone());

    // The grandchild pipe holder must be killed with the group.
    let gone = (0..125).any(|_| {
        if process_can_run(pids[1]) {
            std::thread::sleep(Duration::from_millis(20));
            false
        } else {
            true
        }
    });
    assert!(
        gone,
        "grandchild {} survived the drain timeout kill",
        pids[1]
    );

    let budget = Duration::from_millis(5500);
    if started.elapsed() < budget {
        std::thread::sleep(budget - started.elapsed());
    }
    assert!(!marker.exists(), "grandchild survived the drain timeout");

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&scratch);
}

#[cfg(unix)]
#[test]
fn external_hook_stdin_holding_grandchild_is_killed_after_parent_exit() {
    use std::time::Duration;

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let scratch = std::env::temp_dir().join(format!(
        "cosh_hook_test_stdin_holder_scratch-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&scratch).unwrap();
    let marker = scratch.join("marker");
    let pid_file = scratch.join("pids");
    // 1 MiB preview so the stdin payload overflows the pipe buffer and the
    // writer thread stays blocked once the hook itself has exited.
    let output_file = scratch.join("output.txt");
    fs::write(&output_file, "x".repeat(1 << 20)).unwrap();

    // The hook exits successfully at once; its backgrounded grandchild
    // explicitly inherits stdin (fd 3 dup) but redirects stdout away, so
    // wait and stdout drain complete while stdin delivery never can.
    let body = format!(
        "#!/bin/sh\n# cosh-hook: stdin-holder-hook\n# match-commands: echo\n# timeout: 300ms\nexec 3<&0\n(sleep 5; : > '{}') <&3 >/dev/null 2>&1 &\necho $$ $! > '{}'\nexit 0\n",
        marker.display(),
        pid_file.display()
    );
    let (dir, path) = write_executable_hook(
        "cosh_hook_test_external_stdin_holder",
        "stdin_holder.sh",
        &body,
    );
    let mut engine = HookEngine::new();
    engine.register_external(parse_hook_header(&path).unwrap());

    let mut block = make_block("echo hi");
    block.output.terminal_output_ref = Some(output_file.to_string_lossy().into_owned());
    block.output.terminal_output_bytes = 1 << 20;

    let started = std::time::Instant::now();
    assert!(engine.evaluate(&block).is_empty());
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "stdin delivery must return at the deadline, not when the grandchild exits"
    );

    let mut recorded: Option<Vec<i32>> = None;
    for _ in 0..100 {
        if let Ok(text) = fs::read_to_string(&pid_file) {
            let pids: Vec<i32> = text
                .split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect();
            if pids.len() == 2 {
                recorded = Some(pids);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let pids = recorded.expect("pid file was never fully written");
    let _cleanup = PidCleanup(pids.clone());

    // The grandchild stdin holder must be killed with the group.
    let gone = (0..125).any(|_| {
        if process_can_run(pids[1]) {
            std::thread::sleep(Duration::from_millis(20));
            false
        } else {
            true
        }
    });
    assert!(
        gone,
        "grandchild {} survived the stdin delivery timeout kill",
        pids[1]
    );

    let budget = Duration::from_millis(5500);
    if started.elapsed() < budget {
        std::thread::sleep(budget - started.elapsed());
    }
    assert!(!marker.exists(), "grandchild survived the stdin timeout");

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&scratch);
}

#[cfg(unix)]
#[test]
fn external_payload_cannot_forge_builtin_provenance() {
    let (dir, path) = write_executable_hook(
        "cosh_hook_test_external_provenance",
        "memory.sh",
        "#!/bin/sh\n# cosh-hook: external-memory\n# match-commands: echo\nprintf '{\"hook_id\":\"memory-pressure\",\"severity\":\"warning\",\"title\":\"t\",\"description\":\"d\",\"suggestion\":\"s\",\"builtin_facts\":{\"MemoryPressure\":{\"available_ratio\":0.01}}}'\n",
    );
    let mut engine = HookEngine::new();
    engine.register_external(parse_hook_header(&path).unwrap());

    let findings = engine.evaluate(&make_block("echo hi"));

    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].hook_id, "memory-pressure");
    assert!(matches!(
        findings[0].provenance(),
        HookProvenance::External { .. }
    ));
    assert!(findings[0].builtin_facts.is_none());

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn different_external_registrations_have_distinct_provenance() {
    let (first_dir, first_path) = write_executable_hook(
        "cosh_hook_test_external_registration_first",
        "first.sh",
        "#!/bin/sh\n# cosh-hook: duplicate\n# match-commands: echo\nprintf '{\"hook_id\":\"duplicate\",\"severity\":\"warning\",\"title\":\"t\",\"description\":\"d\",\"suggestion\":\"s\"}'\n",
    );
    let (second_dir, second_path) = write_executable_hook(
        "cosh_hook_test_external_registration_second",
        "second.sh",
        "#!/bin/sh\n# cosh-hook: duplicate\n# match-commands: echo\nprintf '{\"hook_id\":\"duplicate\",\"severity\":\"warning\",\"title\":\"t\",\"description\":\"d\",\"suggestion\":\"s\"}'\n",
    );
    let mut engine = HookEngine::new();
    engine.register_external(parse_hook_header(&first_path).unwrap());
    engine.register_external(parse_hook_header(&second_path).unwrap());

    let findings = engine.evaluate(&make_block("echo hi"));

    assert_eq!(findings.len(), 2);
    assert_ne!(findings[0].provenance(), findings[1].provenance());

    let _ = fs::remove_dir_all(&first_dir);
    let _ = fs::remove_dir_all(&second_dir);
}
