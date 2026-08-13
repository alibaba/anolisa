//! Focused supervisor lifecycle and cleanup tests.

use std::io;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::tempdir;

use super::*;

#[derive(Debug, Default)]
struct TermFailingProcessGroup {
    terminate_calls: AtomicUsize,
    kill_calls: AtomicUsize,
}

impl ProcessGroupLifecycle for TermFailingProcessGroup {
    fn configure(&self, command: &mut Command) {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
    }

    fn terminate(&self, _process_group: u32) -> io::Result<()> {
        self.terminate_calls.fetch_add(1, Ordering::SeqCst);
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "injected TERM failure",
        ))
    }

    fn kill(&self, _process_group: u32) -> io::Result<()> {
        self.kill_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn launch_validation_rejects_relative_program_before_state_change() {
    let workspace = tempdir().unwrap();
    let spec = RuntimeLaunchSpec::new("sh", workspace.path());
    let mut supervisor = RuntimeSupervisor::new();

    assert!(matches!(
        supervisor.launch(&spec),
        Err(RuntimeSupervisorError::Launch(
            RuntimeLaunchError::ProgramNotAbsolute(_)
        ))
    ));
    assert_eq!(supervisor.state(), RuntimeState::Idle);
}

#[cfg(unix)]
#[test]
fn reaps_once_and_retains_bounded_stderr_tail() {
    let workspace = tempdir().unwrap();
    let mut spec = RuntimeLaunchSpec::new("/bin/sh", workspace.path());
    spec.arguments = vec![
        "-c".into(),
        "printf 'ready\\n'; printf '0123456789' >&2; exit 7".into(),
    ];
    spec.stderr_capacity = 5;
    let mut supervisor = RuntimeSupervisor::new();

    supervisor.launch(&spec).unwrap();
    assert_eq!(supervisor.state(), RuntimeState::Initializing);
    supervisor.mark_ready().unwrap();
    assert_eq!(supervisor.read_frame().unwrap().as_deref(), Some("ready"));

    let deadline = Instant::now() + Duration::from_secs(2);
    let terminal = loop {
        if let Some(terminal) = supervisor.poll_terminal().unwrap() {
            break terminal;
        }
        assert!(Instant::now() < deadline, "child did not exit");
        thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(terminal.exit, ProcessExit::Code(7));
    assert_eq!(terminal.stderr.tail, "56789");
    assert_eq!(terminal.stderr.discarded_bytes, 5);
    assert_eq!(supervisor.poll_terminal().unwrap(), None);
}

#[cfg(unix)]
#[test]
fn shutdown_escalates_and_reaps_term_ignoring_child() {
    let workspace = tempdir().unwrap();
    let mut spec = RuntimeLaunchSpec::new("/bin/sh", workspace.path());
    spec.arguments = vec![
        "-c".into(),
        "trap '' TERM; printf 'ready\\n'; while :; do sleep 1; done".into(),
    ];
    let mut supervisor = RuntimeSupervisor::new();

    supervisor.launch(&spec).unwrap();
    assert_eq!(supervisor.read_frame().unwrap().as_deref(), Some("ready"));
    let terminal = supervisor
        .shutdown(Duration::from_millis(20))
        .unwrap()
        .unwrap();

    assert_eq!(terminal.exit, ProcessExit::Signal(9));
    assert_eq!(supervisor.state(), RuntimeState::Exited);
    assert_eq!(supervisor.poll_terminal().unwrap(), None);
}

#[cfg(unix)]
#[test]
fn stdin_write_deadline_keeps_shutdown_available() {
    let workspace = tempdir().unwrap();
    let mut spec = RuntimeLaunchSpec::new("/bin/sh", workspace.path());
    spec.arguments = vec!["-c".into(), "sleep 60".into()];
    spec.stdin_write_timeout = Duration::from_millis(30);
    let mut supervisor = RuntimeSupervisor::new();
    supervisor.launch(&spec).unwrap();

    let frame = "x".repeat(256 * 1024);
    assert!(matches!(
        supervisor.write_frame(&frame),
        Err(RuntimeSupervisorError::Process(ref error))
            if error.kind() == io::ErrorKind::TimedOut
    ));
    assert!(supervisor
        .shutdown(Duration::from_millis(30))
        .unwrap()
        .is_some());
    assert_eq!(supervisor.state(), RuntimeState::Exited);
}

#[cfg(unix)]
#[test]
fn term_group_failure_still_kills_reaps_and_settles_once() {
    let workspace = tempdir().unwrap();
    let mut spec = RuntimeLaunchSpec::new("/bin/sh", workspace.path());
    spec.arguments = vec!["-c".into(), "printf 'ready\\n'; while :; do :; done".into()];
    let process_group = Arc::new(TermFailingProcessGroup::default());
    let mut supervisor = RuntimeSupervisor::with_process_group(process_group.clone());

    supervisor.launch(&spec).unwrap();
    assert_eq!(supervisor.read_frame().unwrap().as_deref(), Some("ready"));
    let error = supervisor.shutdown(Duration::from_secs(1)).unwrap_err();

    assert!(matches!(
        error,
        RuntimeSupervisorError::ProcessGroupSignal { signal: "TERM", .. }
    ));
    assert_eq!(process_group.terminate_calls.load(Ordering::SeqCst), 1);
    assert_eq!(supervisor.state(), RuntimeState::Exited);
    let terminal = supervisor.poll_terminal().unwrap().unwrap();
    assert_eq!(terminal.exit, ProcessExit::Signal(9));
    assert_eq!(supervisor.poll_terminal().unwrap(), None);
}
