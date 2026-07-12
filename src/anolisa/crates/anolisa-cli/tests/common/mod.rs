//! Bounded subprocess support shared by CLI integration tests.

use std::io;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const PROCESS_TIMEOUT: Duration = Duration::from_secs(5);

struct ChildGuard(Child);

impl ChildGuard {
    fn wait_bounded(&mut self, deadline: Instant) -> ExitStatus {
        loop {
            match self.0.try_wait() {
                Ok(Some(status)) => return status,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => panic!("CLI subprocess exceeded {PROCESS_TIMEOUT:?}"),
                Err(error) => panic!("failed to wait for CLI subprocess: {error}"),
            }
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn drain<R: io::Read + Send + 'static>(mut pipe: Option<R>) -> Receiver<io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = match pipe.as_mut() {
            Some(pipe) => pipe.read_to_end(&mut bytes).map(|_| bytes),
            None => Ok(bytes),
        };
        let _ = sender.send(result);
    });
    receiver
}

fn receive_bounded(receiver: Receiver<io::Result<Vec<u8>>>, deadline: Instant) -> Vec<u8> {
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .expect("CLI output drain exceeded process timeout")
        .expect("CLI output must be readable")
}

pub(crate) fn run(arguments: &[&str]) -> Output {
    run_with_stdout(arguments, Stdio::piped())
}

pub(crate) fn run_with_stdout(arguments: &[&str], stdout: Stdio) -> Output {
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_anolisa"))
            .args(arguments)
            .stdout(stdout)
            .stderr(Stdio::piped())
            .spawn()
            .expect("CLI subprocess must start"),
    );
    let stdout = drain::<ChildStdout>(child.0.stdout.take());
    let stderr = drain::<ChildStderr>(child.0.stderr.take());
    let status = child.wait_bounded(deadline);

    Output {
        status,
        stdout: receive_bounded(stdout, deadline),
        stderr: receive_bounded(stderr, deadline),
    }
}
