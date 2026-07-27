use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::super::generation::LineSubmitCounter;
use super::super::mode::current_raw_input_mode;
use super::super::spawn::{
    finish_input_relay, relay_input_bytes, relay_late_capture_input, RawInputRelayState,
};
use super::super::{
    update_input_mode, PromptGhostCandidate, RawInputCapture, RawObserverAction, RawRelayAction,
    UserPtyInputGeneration,
};
use super::*;

fn output_file(label: &str) -> (std::path::PathBuf, File) {
    let path = std::env::temp_dir().join(format!(
        "cosh-shell-prompt-ghost-{label}-{}",
        std::process::id()
    ));
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)
        .expect("test output file");
    (path, file)
}

fn selection_input_mode() -> Arc<Mutex<RawInputMode>> {
    let candidates = vec![
        PromptGhostCandidate {
            text: "inspect memory".to_string(),
            suggestion_id: "health-1".to_string(),
        },
        PromptGhostCandidate {
            text: "continue deployment".to_string(),
            suggestion_id: "personal-1".to_string(),
        },
    ];
    Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: candidates[0].text.clone(),
        route: PromptGhostRoute::AgentSelection {
            candidates,
            active: 0,
        },
    }))
}

fn expect_prompt_ghost_dismissal(receiver: &mpsc::Receiver<RawInputEvent>) {
    for _ in 0..2 {
        if receiver
            .recv_timeout(Duration::from_millis(250))
            .expect("prompt ghost dismissal event")
            == RawInputEvent::PromptGhostDismissed
        {
            return;
        }
    }
    panic!("missing prompt ghost dismissal event");
}

struct ChannelReader {
    receiver: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
}

impl Read for ChannelReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        while self.pending.is_empty() {
            match self.receiver.try_recv() {
                Ok(bytes) => self.pending = bytes,
                Err(mpsc::TryRecvError::Empty) => {
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                Err(mpsc::TryRecvError::Disconnected) => return Ok(0),
            }
        }
        let count = buffer.len().min(self.pending.len());
        buffer[..count].copy_from_slice(&self.pending[..count]);
        self.pending.drain(..count);
        Ok(count)
    }
}

struct ReadStartChannelReader {
    receiver: mpsc::Receiver<Vec<u8>>,
    read_started_tx: mpsc::Sender<()>,
}

impl Read for ReadStartChannelReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.read_started_tx.send(()).expect("observe read start");
        let bytes = match self.receiver.try_recv() {
            Ok(bytes) => bytes,
            Err(mpsc::TryRecvError::Empty) => {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            Err(mpsc::TryRecvError::Disconnected) => return Ok(0),
        };
        assert!(bytes.len() <= buffer.len());
        buffer[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
}

struct PausingChannelReader {
    receiver: mpsc::Receiver<Vec<u8>>,
    pause_on_read: usize,
    read_count: usize,
    bytes_ready_tx: mpsc::Sender<()>,
    resume_rx: mpsc::Receiver<()>,
}

impl Read for PausingChannelReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let bytes = match self.receiver.recv() {
            Ok(bytes) => bytes,
            Err(_) => return Ok(0),
        };
        assert!(bytes.len() <= buffer.len());
        buffer[..bytes.len()].copy_from_slice(&bytes);
        self.read_count += 1;
        if self.read_count == self.pause_on_read {
            self.bytes_ready_tx.send(()).expect("observe read bytes");
            self.resume_rx.recv().expect("resume read");
        }
        Ok(bytes.len())
    }
}

struct SelectionRelay {
    path: std::path::PathBuf,
    master: File,
    input_tx: Option<mpsc::Sender<Vec<u8>>>,
    event_rx: mpsc::Receiver<RawInputEvent>,
    input_mode: Arc<Mutex<RawInputMode>>,
    relay: thread::JoinHandle<io::Result<()>>,
}

impl SelectionRelay {
    fn start(label: &str) -> Self {
        let (path, master) = output_file(label);
        let (input_tx, input_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let input_mode = selection_input_mode();
        let relay = super::super::spawn_raw_input_relay(
            ChannelReader {
                receiver: input_rx,
                pending: Vec::new(),
            },
            master.try_clone().expect("clone output file"),
            event_tx,
            InputClassifier::default(),
            input_mode.clone(),
            UserPtyInputGeneration::default(),
        );
        Self {
            path,
            master,
            input_tx: Some(input_tx),
            event_rx,
            input_mode,
            relay,
        }
    }

    fn send(&self, bytes: &[u8]) {
        self.input_tx
            .as_ref()
            .expect("input sender")
            .send(bytes.to_vec())
            .expect("send input");
    }

    fn finish(mut self) -> (Vec<RawInputEvent>, Vec<u8>, RawInputMode) {
        self.input_tx.take();
        self.relay
            .join()
            .expect("relay thread")
            .expect("relay result");
        self.master.sync_all().expect("sync test output");
        let output = fs::read(&self.path).expect("read test output");
        fs::remove_file(&self.path).ok();
        let mode = self.input_mode.lock().expect("input mode").clone();
        (self.event_rx.try_iter().collect(), output, mode)
    }
}

struct DelayRelay {
    path: std::path::PathBuf,
    master: File,
    input_tx: Option<mpsc::Sender<Vec<u8>>>,
    event_rx: mpsc::Receiver<RawInputEvent>,
    input_mode: Arc<Mutex<RawInputMode>>,
    relay: thread::JoinHandle<io::Result<()>>,
}

impl DelayRelay {
    fn start(label: &str) -> Self {
        let (path, master) = output_file(label);
        let (input_tx, input_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let input_mode = Arc::new(Mutex::new(super::super::mode::new_delay_input_mode()));
        let relay = super::super::spawn_raw_input_relay(
            ChannelReader {
                receiver: input_rx,
                pending: Vec::new(),
            },
            master.try_clone().expect("clone output file"),
            event_tx,
            InputClassifier::default(),
            input_mode.clone(),
            UserPtyInputGeneration::default(),
        );
        Self {
            path,
            master,
            input_tx: Some(input_tx),
            event_rx,
            input_mode,
            relay,
        }
    }

    fn send(&self, bytes: &[u8]) {
        self.input_tx
            .as_ref()
            .expect("input sender")
            .send(bytes.to_vec())
            .expect("send input");
    }

    fn finish(mut self) -> (Vec<RawInputEvent>, Vec<u8>, RawInputMode) {
        self.input_tx.take();
        self.relay
            .join()
            .expect("relay thread")
            .expect("relay result");
        self.master.sync_all().expect("sync test output");
        let output = fs::read(&self.path).expect("read test output");
        fs::remove_file(&self.path).ok();
        let mode = self.input_mode.lock().expect("input mode").clone();
        (self.event_rx.try_iter().collect(), output, mode)
    }
}

fn expect_esc_event(receiver: &mpsc::Receiver<RawInputEvent>) {
    assert_eq!(
        receiver.recv_timeout(Duration::from_millis(250)),
        Ok(RawInputEvent::Esc),
        "expected Esc cancel event"
    );
}

#[test]
fn delay_bare_escape_requests_cancel_and_does_not_forward() {
    let relay = DelayRelay::start("delay-bare-escape");
    relay.send(b"\x1b");
    expect_esc_event(&relay.event_rx);
    let (events, output, mode) = relay.finish();
    assert!(
        !events.contains(&RawInputEvent::CtrlC),
        "unexpected CtrlC event"
    );
    assert_eq!(output, b"exit\n");
    assert!(matches!(mode, RawInputMode::Delay { .. }));
}

#[test]
fn delay_escape_sequence_is_forwarded_without_cancel() {
    let relay = DelayRelay::start("delay-escape-sequence");
    relay.send(b"\x1b[A");
    thread::sleep(Duration::from_millis(100));
    let (events, output, mode) = relay.finish();
    assert!(
        !events.contains(&RawInputEvent::Esc),
        "unexpected Esc event for arrow sequence"
    );
    assert_eq!(output, b"\x1b[Aexit\n");
    assert!(matches!(mode, RawInputMode::Delay { .. }));
}

#[test]
fn delay_split_escape_sequence_is_forwarded_without_cancel() {
    let relay = DelayRelay::start("delay-split-escape-sequence");
    relay.send(b"\x1b");
    thread::sleep(Duration::from_millis(10));
    relay.send(b"[A");
    thread::sleep(Duration::from_millis(100));
    let (events, output, mode) = relay.finish();
    assert!(
        !events.contains(&RawInputEvent::Esc),
        "unexpected Esc event for split arrow sequence"
    );
    assert_eq!(output, b"\x1b[Aexit\n");
    assert!(matches!(mode, RawInputMode::Delay { .. }));
}

#[test]
fn delay_escape_is_forwarded_when_run_finishes_before_deadline() {
    let relay = DelayRelay::start("delay-escape-run-finishes");
    relay.send(b"\x1b");
    thread::sleep(Duration::from_millis(10));
    *relay.input_mode.lock().expect("input mode") = RawInputMode::Passthrough;
    relay.send(b"x");
    let (events, output, mode) = relay.finish();
    assert!(
        !events.contains(&RawInputEvent::Esc),
        "unexpected Esc event after run finished"
    );
    assert_eq!(output, b"\x1bxexit\n");
    assert!(matches!(mode, RawInputMode::Passthrough));
}

#[test]
fn submitted_capture_discards_a_later_owned_read_when_the_chain_ends() {
    let (path, master) = output_file("capture-read-ahead");
    let (input_tx, input_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let first = RawInputCapture::Question {
        id: "q-1".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let second = RawInputCapture::Question {
        id: "q-2".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::Capture {
        capture: first,
        generation: 1,
        installed_at: Instant::now(),
    }));
    let relay = super::super::spawn_raw_input_relay(
        ChannelReader {
            receiver: input_rx,
            pending: Vec::new(),
        },
        master.try_clone().expect("clone output file"),
        event_tx,
        InputClassifier::default(),
        input_mode.clone(),
        UserPtyInputGeneration::default(),
    );

    input_tx.send(b"first\n".to_vec()).expect("first answer");
    let mut events = Vec::new();
    let first_generation = loop {
        let event = event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first capture submission");
        let generation = match &event {
            RawInputEvent::CaptureSubmitted { generation, .. } => Some(*generation),
            _ => None,
        };
        events.push(event);
        if let Some(generation) = generation {
            break generation;
        }
    };

    input_tx
        .send(b"typed-ahead\n".to_vec())
        .expect("read during ack");
    thread::sleep(Duration::from_millis(50));
    update_input_mode(
        &input_mode,
        &RawObserverAction::CaptureInput(second),
        Some(first_generation),
    );
    let deadline = Instant::now() + Duration::from_millis(250);
    while let Ok(event) = event_rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
    {
        let second_generation = match &event {
            RawInputEvent::CaptureSubmitted {
                target_id,
                generation,
                ..
            } if target_id == "q-2" => Some(*generation),
            _ => None,
        };
        events.push(event);
        if let Some(generation) = second_generation {
            update_input_mode(&input_mode, &RawObserverAction::Continue, Some(generation));
            break;
        }
    }
    drop(input_tx);
    relay.join().expect("relay thread").expect("relay result");
    events.extend(event_rx.try_iter());

    assert!(
        !events
            .iter()
            .any(|event| event == &RawInputEvent::CardAnswer("typed-ahead".to_string())),
        "{events:?}"
    );
    master.sync_all().expect("sync test output");
    assert_eq!(fs::read(&path).expect("read test output"), b"exit\n");
    fs::remove_file(path).ok();
}

#[test]
fn input_obtained_before_capture_replacement_does_not_enter_the_new_capture() {
    let (path, master) = output_file("capture-read-return-cutoff");
    let (input_tx, input_rx) = mpsc::channel();
    let (bytes_ready_tx, bytes_ready_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let first = RawInputCapture::Question {
        id: "q-1".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let second = RawInputCapture::Question {
        id: "q-2".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::Capture {
        capture: first,
        generation: 1,
        installed_at: Instant::now(),
    }));
    let relay = super::super::spawn_raw_input_relay(
        PausingChannelReader {
            receiver: input_rx,
            pause_on_read: 2,
            read_count: 0,
            bytes_ready_tx,
            resume_rx,
        },
        master.try_clone().expect("clone output file"),
        event_tx,
        InputClassifier::default(),
        input_mode.clone(),
        UserPtyInputGeneration::default(),
    );

    input_tx.send(b"first\n".to_vec()).expect("first answer");
    let first_generation = loop {
        let event = event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first capture submission");
        if let RawInputEvent::CaptureSubmitted { generation, .. } = event {
            break generation;
        }
    };

    input_tx.send(b"stale".to_vec()).expect("stale input");
    bytes_ready_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("stale bytes obtained before replacement");
    let (updated_tx, updated_rx) = mpsc::channel();
    let update_mode = input_mode.clone();
    let updater = thread::spawn(move || {
        update_input_mode(
            &update_mode,
            &RawObserverAction::CaptureInput(second),
            Some(first_generation),
        );
        updated_tx.send(()).expect("replacement update");
    });
    updated_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("replacement installs while read is pending");
    resume_tx.send(()).expect("release stale read");
    updater.join().expect("replacement updater");
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if matches!(
            current_raw_input_mode(&input_mode),
            RawInputMode::Capture {
                capture: RawInputCapture::Question { ref id, .. },
                ..
            } if id == "q-2"
        ) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "replacement capture not installed"
        );
        thread::yield_now();
    }
    drop(input_tx);

    relay.join().expect("relay thread").expect("relay result");
    let events = event_rx.try_iter().collect::<Vec<_>>();
    assert!(
        !events.iter().any(|event| matches!(
            event,
            RawInputEvent::CardInput(target_id, _) if target_id == "q-2"
        )),
        "{events:?}"
    );
    master.sync_all().expect("sync test output");
    assert_eq!(fs::read(&path).expect("read test output"), b"exit\n");
    fs::remove_file(path).ok();
}

#[test]
fn input_obtained_after_capture_install_enters_the_new_capture() {
    let (path, master) = output_file("capture-read-after-install");
    let (input_tx, input_rx) = mpsc::channel();
    let (read_started_tx, read_started_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let capture = RawInputCapture::Question {
        id: "q-1".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::Passthrough));
    let relay = super::super::spawn_raw_input_relay(
        ReadStartChannelReader {
            receiver: input_rx,
            read_started_tx,
        },
        master.try_clone().expect("clone output file"),
        event_tx,
        InputClassifier::default(),
        input_mode.clone(),
        UserPtyInputGeneration::default(),
    );

    read_started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("reader blocked before capture install");
    update_input_mode(&input_mode, &RawObserverAction::CaptureInput(capture), None);
    input_tx.send(b"answer\n".to_vec()).expect("capture answer");

    let mut events = Vec::new();
    let generation = loop {
        let event = event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("capture submission");
        let generation = match &event {
            RawInputEvent::CaptureSubmitted { generation, .. } => Some(*generation),
            _ => None,
        };
        events.push(event);
        if let Some(generation) = generation {
            break generation;
        }
    };
    update_input_mode(&input_mode, &RawObserverAction::Continue, Some(generation));
    drop(input_tx);
    relay.join().expect("relay thread").expect("relay result");
    events.extend(event_rx.try_iter());

    assert!(
        events
            .iter()
            .any(|event| event == &RawInputEvent::CardAnswer("answer".to_string())),
        "{events:?}"
    );
    master.sync_all().expect("sync test output");
    assert_eq!(fs::read(&path).expect("read test output"), b"exit\n");
    fs::remove_file(path).ok();
}

#[test]
fn passthrough_owned_input_does_not_enter_a_later_capture() {
    let (path, master) = output_file("passthrough-read-capture-cutover");
    let (input_tx, input_rx) = mpsc::channel();
    let (bytes_ready_tx, bytes_ready_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let input_mode = Arc::new(Mutex::new(RawInputMode::Passthrough));
    let relay = super::super::spawn_raw_input_relay(
        PausingChannelReader {
            receiver: input_rx,
            pause_on_read: 1,
            read_count: 0,
            bytes_ready_tx,
            resume_rx,
        },
        master.try_clone().expect("clone output file"),
        event_tx,
        InputClassifier::default(),
        input_mode.clone(),
        UserPtyInputGeneration::default(),
    );

    input_tx.send(b"stale".to_vec()).expect("passthrough input");
    bytes_ready_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("passthrough bytes obtained");
    let (updated_tx, updated_rx) = mpsc::channel();
    let update_mode = input_mode.clone();
    let updater = thread::spawn(move || {
        update_input_mode(
            &update_mode,
            &RawObserverAction::CaptureInput(RawInputCapture::Question {
                id: "q-1".to_string(),
                option_count: 0,
                allow_free_text: true,
                multiple: false,
                secret: false,
            }),
            None,
        );
        updated_tx.send(()).expect("capture update");
    });
    updated_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("capture installs while read is pending");
    resume_tx.send(()).expect("release passthrough read");
    updater.join().expect("capture updater");
    drop(input_tx);

    relay.join().expect("relay thread").expect("relay result");
    let events = event_rx.try_iter().collect::<Vec<_>>();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RawInputEvent::CardInput(target, _) if target == "q-1")),
        "{events:?}"
    );
    master.sync_all().expect("sync test output");
    assert_eq!(fs::read(&path).expect("read test output"), b"exit\n");
    fs::remove_file(path).ok();
}

#[test]
fn delay_owned_escape_does_not_reach_a_later_capture_or_shell() {
    let (path, master) = output_file("delay-read-capture-cutover");
    let (input_tx, input_rx) = mpsc::channel();
    let (bytes_ready_tx, bytes_ready_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let input_mode = Arc::new(Mutex::new(RawInputMode::Delay { generation: 1 }));
    let relay = super::super::spawn_raw_input_relay(
        PausingChannelReader {
            receiver: input_rx,
            pause_on_read: 1,
            read_count: 0,
            bytes_ready_tx,
            resume_rx,
        },
        master.try_clone().expect("clone output file"),
        event_tx,
        InputClassifier::default(),
        input_mode.clone(),
        UserPtyInputGeneration::default(),
    );

    input_tx.send(vec![0x1b]).expect("delay escape");
    bytes_ready_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("delay escape obtained");
    update_input_mode(
        &input_mode,
        &RawObserverAction::CaptureInput(RawInputCapture::Question {
            id: "q-1".to_string(),
            option_count: 0,
            allow_free_text: true,
            multiple: false,
            secret: false,
        }),
        None,
    );
    resume_tx.send(()).expect("release delay read");
    drop(input_tx);

    relay.join().expect("relay thread").expect("relay result");
    let events = event_rx.try_iter().collect::<Vec<_>>();
    assert!(
        !events.iter().any(|event| matches!(
            event,
            RawInputEvent::QuestionCancel(target) if target == "q-1"
        )),
        "{events:?}"
    );
    master.sync_all().expect("sync test output");
    assert_eq!(fs::read(&path).expect("read test output"), b"exit\n");
    fs::remove_file(path).ok();
}

#[test]
fn capture_owned_input_does_not_enter_later_passthrough() {
    let (path, master) = output_file("capture-read-passthrough-cutover");
    let (input_tx, input_rx) = mpsc::channel();
    let (bytes_ready_tx, bytes_ready_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let (event_tx, _event_rx) = mpsc::channel();
    let input_mode = Arc::new(Mutex::new(RawInputMode::Capture {
        capture: RawInputCapture::Question {
            id: "q-1".to_string(),
            option_count: 0,
            allow_free_text: true,
            multiple: false,
            secret: false,
        },
        generation: 1,
        installed_at: Instant::now(),
    }));
    let relay = super::super::spawn_raw_input_relay(
        PausingChannelReader {
            receiver: input_rx,
            pause_on_read: 1,
            read_count: 0,
            bytes_ready_tx,
            resume_rx,
        },
        master.try_clone().expect("clone output file"),
        event_tx,
        InputClassifier::default(),
        input_mode.clone(),
        UserPtyInputGeneration::default(),
    );

    input_tx
        .send(b"stale-capture-input\n".to_vec())
        .expect("capture input");
    bytes_ready_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("capture bytes obtained");
    let (updated_tx, updated_rx) = mpsc::channel();
    let update_mode = input_mode.clone();
    let updater = thread::spawn(move || {
        *update_mode.lock().expect("input mode") = RawInputMode::Passthrough;
        updated_tx.send(()).expect("passthrough update");
    });
    updated_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("passthrough installs while read is pending");
    resume_tx.send(()).expect("release capture read");
    updater.join().expect("passthrough updater");
    drop(input_tx);

    relay.join().expect("relay thread").expect("relay result");
    master.sync_all().expect("sync test output");
    assert_eq!(fs::read(&path).expect("read test output"), b"exit\n");
    fs::remove_file(path).ok();
}

#[test]
fn prompt_ghost_candidate_cycle_during_read_does_not_drop_input() {
    let (path, master) = output_file("ghost-cycle-read-ownership");
    let (input_tx, input_rx) = mpsc::channel();
    let (bytes_ready_tx, bytes_ready_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let input_mode = selection_input_mode();
    let relay = super::super::spawn_raw_input_relay(
        PausingChannelReader {
            receiver: input_rx,
            pause_on_read: 1,
            read_count: 0,
            bytes_ready_tx,
            resume_rx,
        },
        master.try_clone().expect("clone output file"),
        event_tx,
        InputClassifier::default(),
        input_mode.clone(),
        UserPtyInputGeneration::default(),
    );

    input_tx.send(b"x".to_vec()).expect("typed input");
    bytes_ready_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("input bytes obtained");
    // Simulate Shift+Tab candidate cycling landing between the read obtaining
    // bytes and the reader publishing them: same prompt ghost owner, only the
    // active candidate changes.
    {
        let mut mode = input_mode.lock().expect("input mode");
        let RawInputMode::PromptGhost {
            route: PromptGhostRoute::AgentSelection { candidates, .. },
            ..
        } = mode.clone()
        else {
            panic!("prompt ghost selection mode");
        };
        *mode = RawInputMode::PromptGhost {
            text: candidates[1].text.clone(),
            route: PromptGhostRoute::AgentSelection {
                candidates,
                active: 1,
            },
        };
    }
    resume_tx.send(()).expect("release read");
    drop(input_tx);

    relay.join().expect("relay thread").expect("relay result");
    let events = event_rx.try_iter().collect::<Vec<_>>();
    assert!(
        events.contains(&RawInputEvent::PromptGhostDismissed),
        "{events:?}"
    );
    master.sync_all().expect("sync test output");
    assert_eq!(fs::read(&path).expect("read test output"), b"xexit\n");
    fs::remove_file(path).ok();
}

#[test]
fn stale_generation_reads_are_discarded_without_affecting_the_next_capture() {
    let (path, mut master) = output_file("capture-overflow-tagged-reads");
    let (event_tx, event_rx) = mpsc::channel();
    let input_mode = Arc::new(Mutex::new(RawInputMode::Capture {
        capture: RawInputCapture::Question {
            id: "q-2".to_string(),
            option_count: 0,
            allow_free_text: true,
            multiple: false,
            secret: false,
        },
        generation: 2,
        installed_at: Instant::now(),
    }));
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    let chunks = (0..12)
        .map(|index| vec![b'a' + index; 8192])
        .collect::<Vec<_>>();
    for chunk in &chunks {
        relay_late_capture_input(
            chunk,
            1,
            &mut master,
            &event_tx,
            &classifier,
            &input_mode,
            &mut state,
        )
        .expect("relay tagged capture input");
    }
    assert!(matches!(
        current_raw_input_mode(&input_mode),
        RawInputMode::Capture {
            capture: RawInputCapture::Question { id, .. },
            generation: 2,
            ..
        } if id == "q-2"
    ));
    finish_input_relay(&mut master, &event_tx, &classifier, &input_mode, &mut state)
        .expect("finish relay");
    let events = event_rx.try_iter().collect::<Vec<_>>();
    assert!(
        !events.iter().any(|event| matches!(
            event,
            RawInputEvent::CardInput(target_id, _)
                | RawInputEvent::CaptureSubmitted { target_id, .. }
                if target_id == "q-2"
        )),
        "{events:?}"
    );
    assert!(!events
        .iter()
        .any(|event| matches!(event, RawInputEvent::CaptureOverflow { .. })));
    master.sync_all().expect("sync test output");
    assert_eq!(fs::read(&path).expect("read test output"), b"exit\n");
    fs::remove_file(path).ok();
}

#[test]
fn active_capture_eof_drains_the_generation_without_input() {
    let (path, master) = output_file("capture-empty-eof");
    let (input_tx, input_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let input_mode = Arc::new(Mutex::new(RawInputMode::Capture {
        capture: RawInputCapture::Question {
            id: "q-1".to_string(),
            option_count: 0,
            allow_free_text: true,
            multiple: false,
            secret: false,
        },
        generation: 9,
        installed_at: Instant::now(),
    }));
    let relay = super::super::spawn_raw_input_relay(
        ChannelReader {
            receiver: input_rx,
            pending: Vec::new(),
        },
        master.try_clone().expect("clone output file"),
        event_tx,
        InputClassifier::default(),
        input_mode.clone(),
        UserPtyInputGeneration::default(),
    );

    drop(input_tx);
    relay.join().expect("relay thread").expect("relay result");
    let events = event_rx.try_iter().collect::<Vec<_>>();

    assert_eq!(
        events
            .iter()
            .filter(|event| { matches!(event, RawInputEvent::CaptureDrained { generation: 9 }) })
            .count(),
        1,
        "{events:?}"
    );
    assert!(matches!(
        *input_mode.lock().expect("input mode"),
        RawInputMode::Terminal { generation: 9, .. }
    ));
    master.sync_all().expect("sync test output");
    assert_eq!(fs::read(&path).expect("read test output"), b"exit\n");
    fs::remove_file(path).ok();
}

#[test]
fn selection_bare_escape_times_out_without_waiting_for_another_key() {
    let (path, master) = output_file("selection-bare-escape");
    let (input_tx, input_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let candidates = vec![PromptGhostCandidate {
        text: "inspect memory".to_string(),
        suggestion_id: "health-1".to_string(),
    }];
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: candidates[0].text.clone(),
        route: PromptGhostRoute::AgentSelection {
            candidates,
            active: 0,
        },
    }));
    let relay = super::super::spawn_raw_input_relay(
        ChannelReader {
            receiver: input_rx,
            pending: Vec::new(),
        },
        master.try_clone().expect("clone output file"),
        event_tx,
        InputClassifier::default(),
        input_mode.clone(),
        UserPtyInputGeneration::default(),
    );

    input_tx.send(b"\x1b".to_vec()).expect("send escape");
    expect_prompt_ghost_dismissal(&event_rx);
    assert!(matches!(
        *input_mode.lock().expect("input mode"),
        RawInputMode::Passthrough
    ));

    drop(input_tx);
    relay.join().expect("relay thread").expect("relay result");
    master.sync_all().expect("sync test output");
    assert_eq!(fs::read(&path).expect("read test output"), b"\x1bexit\n");
    fs::remove_file(path).ok();
}

#[test]
fn selection_action_wait_flushes_escape_at_the_deadline() {
    let (path, master) = output_file("selection-action-wait");
    let (tx, rx) = mpsc::channel();
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "inspect memory".to_string(),
        route: PromptGhostRoute::AgentSelection {
            candidates: vec![PromptGhostCandidate {
                text: "inspect memory".to_string(),
                suggestion_id: "health-1".to_string(),
            }],
            active: 0,
        },
    }));
    let relay = super::super::spawn_raw_action_relay(
        vec![
            RawRelayAction::write(b"\x1b"),
            RawRelayAction::wait(Duration::from_millis(500)),
        ],
        master.try_clone().expect("clone output file"),
        0,
        tx,
        InputClassifier::default(),
        input_mode.clone(),
        UserPtyInputGeneration::default(),
    );

    expect_prompt_ghost_dismissal(&rx);
    assert!(!relay.is_finished());
    assert!(matches!(
        *input_mode.lock().expect("input mode"),
        RawInputMode::Passthrough
    ));

    relay.join().expect("relay thread").expect("relay result");
    master.sync_all().expect("sync test output");
    assert_eq!(fs::read(&path).expect("read test output"), b"\x1bexit\n");
    fs::remove_file(path).ok();
}

#[test]
fn selection_shift_tab_cycles_when_arriving_in_one_chunk() {
    let relay = SelectionRelay::start("selection-shift-tab");
    relay.send(b"\x1b[Z");

    let (events, output, mode) = relay.finish();

    assert_eq!(output, b"exit\n");
    assert!(events.contains(&RawInputEvent::PromptGhostCycle {
        text: "continue deployment".to_string(),
    }));
    assert!(matches!(
        mode,
        RawInputMode::PromptGhost { text, .. } if text == "continue deployment"
    ));
}

#[test]
fn selection_shift_tab_cycles_when_arriving_in_three_chunks_within_window() {
    let (path, mut master) = output_file("selection-split-shift-tab");
    let (tx, rx) = mpsc::channel();
    let input_mode = selection_input_mode();
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    let received_at = Instant::now();

    for (bytes, offset) in [
        (b"\x1b".as_slice(), 0),
        (b"[".as_slice(), 1),
        (b"Z".as_slice(), 2),
    ] {
        relay_input_bytes(
            bytes,
            received_at + Duration::from_millis(offset),
            &mut master,
            &tx,
            &classifier,
            &input_mode,
            &mut state,
        )
        .expect("relay split shift-tab");
    }

    let events = rx.try_iter().collect::<Vec<_>>();
    assert_eq!(fs::read(&path).expect("read test output"), b"");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RawInputEvent::PromptGhostCycle { .. }))
            .count(),
        1
    );
    fs::remove_file(path).ok();
}

#[test]
fn selection_shift_tab_received_before_deadline_survives_a_delayed_relay() {
    let (path, mut master) = output_file("selection-delayed-shift-tab");
    let (tx, rx) = mpsc::channel();
    let input_mode = selection_input_mode();
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();

    relay_input_bytes(
        b"\x1b",
        Instant::now()
            .checked_sub(Duration::from_millis(100))
            .expect("recent timestamp"),
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("buffer escape");
    let received_at = Instant::now()
        .checked_sub(Duration::from_millis(90))
        .expect("recent timestamp");
    relay_input_bytes(
        b"[Z",
        received_at,
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("cycle delayed shift-tab");

    assert!(rx.try_iter().any(|event| matches!(
        event,
        RawInputEvent::PromptGhostCycle { text } if text == "continue deployment"
    )));
    assert_eq!(fs::read(&path).expect("read test output"), b"");
    fs::remove_file(path).ok();
}

#[test]
fn selection_escape_with_nonmatching_follow_up_dismisses_and_forwards_all_bytes() {
    let relay = SelectionRelay::start("selection-escape-nonmatching");
    relay.send(b"\x1b");
    relay.send(b"x");

    let (events, output, mode) = relay.finish();

    assert_eq!(output, b"\x1bxexit\n");
    assert!(events.contains(&RawInputEvent::PromptGhostDismissed));
    assert!(!events
        .iter()
        .any(|event| matches!(event, RawInputEvent::PromptGhostCycle { .. })));
    assert!(matches!(mode, RawInputMode::Passthrough));
}

#[test]
fn selection_partial_csi_times_out_and_forwards_all_bytes() {
    let relay = SelectionRelay::start("selection-partial-csi");
    relay.send(b"\x1b[");
    expect_prompt_ghost_dismissal(&relay.event_rx);

    let (_, output, mode) = relay.finish();
    assert_eq!(output, b"\x1b[exit\n");
    assert!(matches!(mode, RawInputMode::Passthrough));
}

#[test]
fn selection_pending_escape_at_eof_dismisses_and_forwards_escape() {
    let relay = SelectionRelay::start("selection-escape-eof");
    relay.send(b"\x1b");

    let (events, output, mode) = relay.finish();

    assert_eq!(output, b"\x1bexit\n");
    assert!(events.contains(&RawInputEvent::PromptGhostDismissed));
    assert!(matches!(mode, RawInputMode::Passthrough));
}

#[test]
fn selection_pending_escape_at_eof_after_route_change_is_not_dropped() {
    let (path, mut master) = output_file("selection-escape-route-eof");
    let (tx, rx) = mpsc::channel();
    let old_route = PromptGhostRoute::AgentSelection {
        candidates: vec![PromptGhostCandidate {
            text: "old selection".to_string(),
            suggestion_id: "old-1".to_string(),
        }],
        active: 0,
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "old selection".to_string(),
        route: old_route,
    }));
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    relay_input_bytes(
        b"\x1b",
        Instant::now(),
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("buffer escape");
    *input_mode.lock().expect("input mode") = RawInputMode::PromptGhost {
        text: "new selection".to_string(),
        route: PromptGhostRoute::AgentSelection {
            candidates: vec![PromptGhostCandidate {
                text: "new selection".to_string(),
                suggestion_id: "new-1".to_string(),
            }],
            active: 0,
        },
    };
    finish_input_relay(&mut master, &tx, &classifier, &input_mode, &mut state)
        .expect("finish relay");

    assert_eq!(fs::read(&path).expect("read test output"), b"\x1bexit\n");
    assert!(rx
        .try_iter()
        .any(|event| event == RawInputEvent::PromptGhostDismissed));
    assert!(matches!(
        *input_mode.lock().expect("input mode"),
        RawInputMode::Passthrough
    ));
    fs::remove_file(path).ok();
}

#[test]
fn selection_route_change_before_deadline_dismisses_then_forwards_shift_tab() {
    let (path, mut master) = output_file("selection-route-change-shift-tab");
    let (tx, rx) = mpsc::channel();
    let old_route = PromptGhostRoute::AgentSelection {
        candidates: vec![PromptGhostCandidate {
            text: "old selection".to_string(),
            suggestion_id: "old-1".to_string(),
        }],
        active: 0,
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "old selection".to_string(),
        route: old_route,
    }));
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    let received_at = Instant::now();

    relay_input_bytes(
        b"\x1b",
        received_at,
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("buffer escape");
    *input_mode.lock().expect("input mode") = RawInputMode::PromptGhost {
        text: "new selection".to_string(),
        route: PromptGhostRoute::AgentSelection {
            candidates: vec![
                PromptGhostCandidate {
                    text: "new selection".to_string(),
                    suggestion_id: "new-1".to_string(),
                },
                PromptGhostCandidate {
                    text: "another selection".to_string(),
                    suggestion_id: "new-2".to_string(),
                },
            ],
            active: 0,
        },
    };

    relay_input_bytes(
        b"[Z",
        received_at + Duration::from_millis(1),
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("handle shift-tab after route change");

    let events = rx.try_iter().collect::<Vec<_>>();
    assert_eq!(fs::read(&path).expect("read test output"), b"\x1b[Z");
    assert!(events.contains(&RawInputEvent::PromptGhostDismissed));
    assert!(!events
        .iter()
        .any(|event| matches!(event, RawInputEvent::PromptGhostCycle { .. })));
    assert!(matches!(
        *input_mode.lock().expect("input mode"),
        RawInputMode::Passthrough
    ));
    fs::remove_file(path).ok();
}

#[test]
fn selection_expired_escape_dismisses_instead_of_rebuffering_for_a_new_route() {
    let (path, mut master) = output_file("selection-expired-route-change");
    let (tx, rx) = mpsc::channel();
    let old_route = PromptGhostRoute::AgentSelection {
        candidates: vec![PromptGhostCandidate {
            text: "old selection".to_string(),
            suggestion_id: "old-1".to_string(),
        }],
        active: 0,
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "old selection".to_string(),
        route: old_route,
    }));
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    let received_at = Instant::now()
        .checked_sub(Duration::from_millis(100))
        .expect("recent timestamp");
    relay_input_bytes(
        b"\x1b",
        received_at,
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("buffer escape");
    *input_mode.lock().expect("input mode") = RawInputMode::PromptGhost {
        text: "new selection".to_string(),
        route: PromptGhostRoute::AgentSelection {
            candidates: vec![PromptGhostCandidate {
                text: "new selection".to_string(),
                suggestion_id: "new-1".to_string(),
            }],
            active: 0,
        },
    };
    relay_input_bytes(
        b"",
        received_at + Duration::from_millis(51),
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("flush expired escape");

    assert_eq!(fs::read(&path).expect("read test output"), b"\x1b");
    assert!(rx
        .try_iter()
        .any(|event| event == RawInputEvent::PromptGhostDismissed));
    assert!(matches!(
        *input_mode.lock().expect("input mode"),
        RawInputMode::Passthrough
    ));
    fs::remove_file(path).ok();
}

#[test]
fn selection_timeout_and_follow_up_byte_do_not_duplicate_or_reorder_input() {
    let (path, mut master) = output_file("selection-timeout-follow-up");
    let (tx, rx) = mpsc::channel();
    let input_mode = selection_input_mode();
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    let received_at = Instant::now()
        .checked_sub(Duration::from_millis(100))
        .expect("recent timestamp");

    relay_input_bytes(
        b"\x1b",
        received_at,
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("buffer escape");
    relay_input_bytes(
        b"x",
        received_at + Duration::from_millis(51),
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("flush escape and relay follow-up");

    let events = rx.try_iter().collect::<Vec<_>>();
    assert_eq!(fs::read(&path).expect("read test output"), b"\x1bx");
    assert_eq!(
        events
            .iter()
            .filter(|event| **event == RawInputEvent::PromptGhostDismissed)
            .count(),
        1
    );
    fs::remove_file(path).ok();
}

#[test]
fn selection_pending_escape_is_forwarded_when_the_input_mode_changes() {
    let (path, mut master) = output_file("selection-mode-change");
    let (tx, rx) = mpsc::channel();
    let input_mode = selection_input_mode();
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    let received_at = Instant::now();

    relay_input_bytes(
        b"\x1b",
        received_at,
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("buffer escape");
    *input_mode.lock().expect("input mode") = RawInputMode::RawPassthrough;
    relay_input_bytes(
        b"x",
        received_at + Duration::from_millis(1),
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("relay pending escape after mode change");

    let events = rx.try_iter().collect::<Vec<_>>();
    let mode = input_mode.lock().expect("input mode").clone();
    assert_eq!(fs::read(&path).expect("read test output"), b"\x1bx");
    assert!(!events
        .iter()
        .any(|event| matches!(event, RawInputEvent::PromptGhostCycle { .. })));
    assert!(matches!(mode, RawInputMode::RawPassthrough));
    fs::remove_file(path).ok();
}

#[test]
fn selection_pending_escape_does_not_cancel_a_new_capture() {
    let (path, mut master) = output_file("selection-to-capture");
    let (tx, rx) = mpsc::channel();
    let input_mode = selection_input_mode();
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    let received_at = Instant::now();

    relay_input_bytes(
        b"\x1b",
        received_at,
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("buffer escape");
    *input_mode.lock().expect("input mode") = RawInputMode::Capture {
        capture: RawInputCapture::Question {
            id: "q-1".to_string(),
            option_count: 0,
            allow_free_text: true,
            multiple: false,
            secret: false,
        },
        generation: 7,
        installed_at: Instant::now(),
    };

    let mode_for_ack = input_mode.clone();
    let ack = thread::spawn(move || {
        let mut events = Vec::new();
        while let Ok(event) = rx.recv_timeout(Duration::from_millis(250)) {
            let generation = match &event {
                RawInputEvent::CaptureSubmitted { generation, .. } => Some(*generation),
                _ => None,
            };
            events.push(event);
            if let Some(generation) = generation {
                update_input_mode(
                    &mode_for_ack,
                    &RawObserverAction::Continue,
                    Some(generation),
                );
                break;
            }
        }
        events
    });

    relay_input_bytes(
        b"",
        received_at + Duration::from_millis(1),
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("flush pending escape after capture install");
    let events = ack.join().expect("ack thread");

    assert!(
        !events.iter().any(|event| matches!(
            event,
            RawInputEvent::QuestionCancel(_) | RawInputEvent::CaptureSubmitted { .. }
        )),
        "{events:?}"
    );
    assert!(matches!(
        *input_mode.lock().expect("input mode"),
        RawInputMode::Capture { generation: 7, .. }
    ));
    assert_eq!(fs::read(&path).expect("read test output"), b"");
    fs::remove_file(path).ok();
}

#[test]
fn selection_split_shift_tab_suffix_does_not_enter_a_new_capture() {
    let (path, mut master) = output_file("selection-split-shift-tab-to-capture");
    let (tx, rx) = mpsc::channel();
    let input_mode = selection_input_mode();
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    let received_at = Instant::now();

    relay_input_bytes(
        b"\x1b",
        received_at,
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("buffer escape");
    *input_mode.lock().expect("input mode") = RawInputMode::Capture {
        capture: RawInputCapture::Question {
            id: "q-1".to_string(),
            option_count: 0,
            allow_free_text: true,
            multiple: false,
            secret: false,
        },
        generation: 7,
        installed_at: Instant::now(),
    };

    relay_input_bytes(
        b"[Z",
        received_at + Duration::from_millis(1),
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("discard replaced ghost suffix");

    let events = rx.try_iter().collect::<Vec<_>>();
    assert!(
        !events.iter().any(|event| matches!(
            event,
            RawInputEvent::CardInput(_, _)
                | RawInputEvent::QuestionCancel(_)
                | RawInputEvent::CaptureSubmitted { .. }
        )),
        "{events:?}"
    );
    assert!(matches!(
        *input_mode.lock().expect("input mode"),
        RawInputMode::Capture { generation: 7, .. }
    ));
    assert_eq!(fs::read(&path).expect("read test output"), b"");
    fs::remove_file(path).ok();
}

#[test]
fn shell_rewrite_tab_writes_to_native_line_editor_without_agent_intercept() {
    let (path, mut master) = output_file("native");
    let (tx, rx) = mpsc::channel();
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "grep file".to_string(),
        route: PromptGhostRoute::NativeShell,
    }));
    let mut line_buffer = CandidateLineBuffer::default();
    let mut native_line_state = NativeLineState::default();
    let mut exit_tracker = ExplicitExitTracker::default();
    let classifier = InputClassifier::default();
    let input_generation = UserPtyInputGeneration::default();
    let mut line_submits = LineSubmitCounter::default();
    let mut relay = InputRelayContext {
        master: &mut master,
        input_classifier: &classifier,
        input_events: &tx,
        input_mode: &input_mode,
        input_generation: &input_generation,
        line_submits: &mut line_submits,
        line_buffer: &mut line_buffer,
        native_line_state: &mut native_line_state,
        exit_tracker: &mut exit_tracker,
    };

    assert!(relay_prompt_ghost_input(
        b"\t",
        "grep file",
        &PromptGhostRoute::NativeShell,
        &mut relay,
    )
    .expect("accept native ghost"));
    relay_passthrough_input(b"\t\x15", &mut relay)
        .expect("native completion and line clearing remain available");
    master.sync_all().expect("sync test output");

    assert_eq!(
        fs::read(&path).expect("read test output"),
        b"grep file\t\x15"
    );
    assert_eq!(
        rx.try_iter().collect::<Vec<_>>(),
        vec![
            RawInputEvent::PromptGhostClear,
            RawInputEvent::PtyUserWrite {
                generation: 1,
                line_submits: 0,
            },
            RawInputEvent::ShellInputActivity { empty: true },
            RawInputEvent::PtyUserWrite {
                generation: 2,
                line_submits: 0,
            },
        ]
    );
    assert!(!line_buffer.force_agent_intercept);
    assert!(matches!(
        *input_mode.lock().expect("input mode"),
        RawInputMode::RawPassthrough
    ));
    fs::remove_file(path).ok();
}

#[test]
fn native_slash_tab_is_not_redrawn_before_shell_completion() {
    let (path, mut master) = output_file("native-slash-tab");
    let (tx, rx) = mpsc::channel();
    let input_mode = Arc::new(Mutex::new(RawInputMode::Passthrough));
    let mut line_buffer = CandidateLineBuffer::default();
    let mut native_line_state = NativeLineState::default();
    let mut exit_tracker = ExplicitExitTracker::default();
    let classifier = InputClassifier::conservative();
    let input_generation = UserPtyInputGeneration::default();
    let mut line_submits = LineSubmitCounter::default();
    let mut relay = InputRelayContext {
        master: &mut master,
        input_classifier: &classifier,
        input_events: &tx,
        input_mode: &input_mode,
        input_generation: &input_generation,
        line_submits: &mut line_submits,
        line_buffer: &mut line_buffer,
        native_line_state: &mut native_line_state,
        exit_tracker: &mut exit_tracker,
    };

    relay_passthrough_input(b"/ho", &mut relay).expect("buffer slash prefix");
    relay_passthrough_input(b"\t", &mut relay).expect("send completion to shell");
    master.sync_all().expect("sync test output");

    let events = rx.try_iter().collect::<Vec<_>>();
    assert!(events.iter().any(|event| matches!(
        event,
        RawInputEvent::CandidateRedraw { input, .. } if input == b"/ho"
    )));
    assert!(events.iter().all(|event| !matches!(
        event,
        RawInputEvent::CandidateRedraw { input, .. } if input.contains(&b'\t')
    )));
    assert!(events.contains(&RawInputEvent::CandidateClearLine));
    assert_eq!(fs::read(&path).expect("read test output"), b"/ho\t");
    assert!(!line_buffer.is_active());
    fs::remove_file(path).ok();
}

#[test]
fn native_shell_input_reports_editing_then_empty_without_content() {
    let (path, mut master) = output_file("input-state");
    let (tx, rx) = mpsc::channel();
    let input_mode = Arc::new(Mutex::new(RawInputMode::RawPassthrough));
    let mut line_buffer = CandidateLineBuffer::default();
    let mut native_line_state = NativeLineState::default();
    let mut exit_tracker = ExplicitExitTracker::default();
    let classifier = InputClassifier::default();
    let input_generation = UserPtyInputGeneration::default();
    let mut line_submits = LineSubmitCounter::default();
    let mut relay = InputRelayContext {
        master: &mut master,
        input_classifier: &classifier,
        input_events: &tx,
        input_mode: &input_mode,
        input_generation: &input_generation,
        line_submits: &mut line_submits,
        line_buffer: &mut line_buffer,
        native_line_state: &mut native_line_state,
        exit_tracker: &mut exit_tracker,
    };

    relay_passthrough_input(b"partial", &mut relay).expect("type partial line");
    relay_passthrough_input(&[super::super::CTRL_U], &mut relay).expect("clear line");

    assert_eq!(
        rx.try_iter().collect::<Vec<_>>(),
        vec![
            RawInputEvent::ShellInputActivity { empty: false },
            RawInputEvent::PtyUserWrite {
                generation: 1,
                line_submits: 0,
            },
            RawInputEvent::ShellInputActivity { empty: true },
            RawInputEvent::PtyUserWrite {
                generation: 2,
                line_submits: 0,
            },
        ]
    );
    fs::remove_file(path).ok();
}

#[test]
fn agent_prompt_tab_stays_local_until_enter_and_keeps_suggestion_id() {
    let (path, mut master) = output_file("agent");
    let (tx, rx) = mpsc::channel();
    let route = PromptGhostRoute::AgentIntercept {
        suggestion_id: Some("suggestion-1".to_string()),
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "analyze failure".to_string(),
        route: route.clone(),
    }));
    let mut line_buffer = CandidateLineBuffer::default();
    let mut native_line_state = NativeLineState::default();
    let mut exit_tracker = ExplicitExitTracker::default();
    let classifier = InputClassifier::default();
    let input_generation = UserPtyInputGeneration::default();
    let mut line_submits = LineSubmitCounter::default();
    let mut relay = InputRelayContext {
        master: &mut master,
        input_classifier: &classifier,
        input_events: &tx,
        input_mode: &input_mode,
        input_generation: &input_generation,
        line_submits: &mut line_submits,
        line_buffer: &mut line_buffer,
        native_line_state: &mut native_line_state,
        exit_tracker: &mut exit_tracker,
    };

    relay_prompt_ghost_input(b"\t", "analyze failure", &route, &mut relay)
        .expect("accept agent ghost");
    let accepted = rx.try_iter().collect::<Vec<_>>();
    assert!(accepted.contains(&RawInputEvent::PromptGhostAccepted {
        suggestion_id: Some("suggestion-1".to_string()),
    }));
    assert!(accepted
        .iter()
        .all(|event| !matches!(event, RawInputEvent::PromptGhostIntercept { .. })));

    relay_passthrough_input(b" safely\n", &mut relay).expect("submit edited agent prompt");
    assert!(rx.try_iter().any(|event| matches!(
        event,
        RawInputEvent::PromptGhostIntercept { input, suggestion_id }
            if input == "analyze failure safely"
                && suggestion_id.as_deref() == Some("suggestion-1")
    )));
    assert_eq!(fs::read(&path).expect("read test output"), b"");
    fs::remove_file(path).ok();
}

#[test]
fn selection_shift_tab_cycles_and_tab_inserts_the_active_prompt() {
    let relay = SelectionRelay::start("selection-cycle-tab");
    relay.send(b"\x1b[Z");
    relay.send(b"\t");

    let (events, output, mode) = relay.finish();
    assert!(events.contains(&RawInputEvent::PromptGhostCycle {
        text: "continue deployment".to_string(),
    }));
    assert!(events.contains(&RawInputEvent::PromptGhostAccepted {
        suggestion_id: Some("personal-1".to_string()),
    }));
    assert_eq!(output, b"exit\n");
    assert!(matches!(mode, RawInputMode::Passthrough));
}

#[test]
fn selection_enter_submits_the_active_prompt_without_shell_execution() {
    let (path, mut master) = output_file("selection-enter");
    let (tx, rx) = mpsc::channel();
    let route = PromptGhostRoute::AgentSelection {
        candidates: vec![PromptGhostCandidate {
            text: "inspect disk pressure".to_string(),
            suggestion_id: "health-disk".to_string(),
        }],
        active: 0,
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "inspect disk pressure".to_string(),
        route: route.clone(),
    }));
    let mut line_buffer = CandidateLineBuffer::default();
    let mut native_line_state = NativeLineState::default();
    let mut exit_tracker = ExplicitExitTracker::default();
    let classifier = InputClassifier::default();
    let input_generation = UserPtyInputGeneration::default();
    let mut line_submits = LineSubmitCounter::default();
    let mut relay = InputRelayContext {
        master: &mut master,
        input_classifier: &classifier,
        input_events: &tx,
        input_mode: &input_mode,
        input_generation: &input_generation,
        line_submits: &mut line_submits,
        line_buffer: &mut line_buffer,
        native_line_state: &mut native_line_state,
        exit_tracker: &mut exit_tracker,
    };

    relay_prompt_ghost_input(b"\r", "inspect disk pressure", &route, &mut relay)
        .expect("submit active selection");

    let events = rx.try_iter().collect::<Vec<_>>();
    assert!(events.iter().any(|event| matches!(
        event,
        RawInputEvent::PromptGhostIntercept { input, suggestion_id }
            if input == "inspect disk pressure"
                && suggestion_id.as_deref() == Some("health-disk")
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event, RawInputEvent::PromptGhostAccepted { .. })));
    assert_eq!(fs::read(&path).unwrap(), b"");
    assert!(matches!(
        *input_mode.lock().unwrap(),
        RawInputMode::Delay { .. }
    ));
    fs::remove_file(path).ok();
}

#[test]
fn clearing_accepted_agent_prompt_emits_binding_dismissal() {
    let (path, mut master) = output_file("clear-agent");
    let (tx, rx) = mpsc::channel();
    let route = PromptGhostRoute::AgentIntercept {
        suggestion_id: Some("suggestion-1".to_string()),
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "analyze failure".to_string(),
        route: route.clone(),
    }));
    let mut line_buffer = CandidateLineBuffer::default();
    let mut native_line_state = NativeLineState::default();
    let mut exit_tracker = ExplicitExitTracker::default();
    let classifier = InputClassifier::default();
    let input_generation = UserPtyInputGeneration::default();
    let mut line_submits = LineSubmitCounter::default();
    let mut relay = InputRelayContext {
        master: &mut master,
        input_classifier: &classifier,
        input_events: &tx,
        input_mode: &input_mode,
        input_generation: &input_generation,
        line_submits: &mut line_submits,
        line_buffer: &mut line_buffer,
        native_line_state: &mut native_line_state,
        exit_tracker: &mut exit_tracker,
    };

    relay_prompt_ghost_input(b"\t", "analyze failure", &route, &mut relay)
        .expect("accept agent ghost");
    relay_passthrough_input(&[0x15], &mut relay).expect("clear accepted prompt");

    assert!(rx
        .try_iter()
        .any(|event| event == RawInputEvent::PromptGhostDismissed));
    assert!(!line_buffer.is_active());
    fs::remove_file(path).ok();
}

#[test]
fn unsupported_arrow_after_agent_prompt_tab_cancels_without_writing_to_shell() {
    let (path, mut master) = output_file("agent-arrow-cancel");
    let (tx, rx) = mpsc::channel();
    let route = PromptGhostRoute::AgentIntercept {
        suggestion_id: Some("suggestion-1".to_string()),
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "analyze failure".to_string(),
        route: route.clone(),
    }));
    let mut line_buffer = CandidateLineBuffer::default();
    let mut native_line_state = NativeLineState::default();
    let mut exit_tracker = ExplicitExitTracker::default();
    let classifier = InputClassifier::default();
    let input_generation = UserPtyInputGeneration::default();
    let mut line_submits = LineSubmitCounter::default();
    let mut relay = InputRelayContext {
        master: &mut master,
        input_classifier: &classifier,
        input_events: &tx,
        input_mode: &input_mode,
        input_generation: &input_generation,
        line_submits: &mut line_submits,
        line_buffer: &mut line_buffer,
        native_line_state: &mut native_line_state,
        exit_tracker: &mut exit_tracker,
    };

    relay_prompt_ghost_input(b"\t", "analyze failure", &route, &mut relay)
        .expect("accept agent ghost");
    relay_passthrough_input(b"\x1b[D", &mut relay).expect("cancel unsupported edit");
    master.sync_all().expect("sync test output");

    let events = rx.try_iter().collect::<Vec<_>>();
    assert_eq!(fs::read(&path).expect("read test output"), b"");
    assert!(events.contains(&RawInputEvent::PromptGhostDismissed));
    assert!(!events
        .iter()
        .any(|event| matches!(event, RawInputEvent::PromptGhostIntercept { .. })));
    assert!(!line_buffer.is_active());
    assert!(line_buffer.forced_agent_suggestion_id.is_none());
    fs::remove_file(path).ok();
}

#[test]
fn split_cursor_sequences_after_agent_prompt_tab_never_reach_shell() {
    for (name, sequence) in [
        ("left", b"\x1b[D".as_slice()),
        ("right", b"\x1b[C".as_slice()),
        ("home", b"\x1b[H".as_slice()),
        ("end", b"\x1b[F".as_slice()),
    ] {
        let (path, mut master) = output_file(&format!("agent-split-{name}"));
        let (tx, rx) = mpsc::channel();
        let route = PromptGhostRoute::AgentIntercept {
            suggestion_id: Some("suggestion-1".to_string()),
        };
        let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
            text: "analyze failure".to_string(),
            route: route.clone(),
        }));
        let mut line_buffer = CandidateLineBuffer::default();
        let mut native_line_state = NativeLineState::default();
        let mut exit_tracker = ExplicitExitTracker::default();
        let classifier = InputClassifier::default();
        let input_generation = UserPtyInputGeneration::default();
        let mut line_submits = LineSubmitCounter::default();
        let mut relay = InputRelayContext {
            master: &mut master,
            input_classifier: &classifier,
            input_events: &tx,
            input_mode: &input_mode,
            input_generation: &input_generation,
            line_submits: &mut line_submits,
            line_buffer: &mut line_buffer,
            native_line_state: &mut native_line_state,
            exit_tracker: &mut exit_tracker,
        };

        relay_prompt_ghost_input(b"\t", "analyze failure", &route, &mut relay)
            .expect("accept agent ghost");
        for byte in sequence {
            relay_passthrough_input(&[*byte], &mut relay).expect("relay split sequence");
        }
        master.sync_all().expect("sync test output");

        let events = rx.try_iter().collect::<Vec<_>>();
        assert_eq!(fs::read(&path).expect("read test output"), b"");
        assert!(events.contains(&RawInputEvent::PromptGhostDismissed));
        assert!(!events
            .iter()
            .any(|event| matches!(event, RawInputEvent::PromptGhostIntercept { .. })));
        assert!(!line_buffer.is_active());
        fs::remove_file(path).ok();
    }
}

#[test]
fn clearing_and_submitting_in_one_buffer_dismisses_binding() {
    let (path, mut master) = output_file("clear-submit-agent");
    let (tx, rx) = mpsc::channel();
    let route = PromptGhostRoute::AgentIntercept {
        suggestion_id: Some("suggestion-1".to_string()),
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "analyze failure".to_string(),
        route: route.clone(),
    }));
    let mut line_buffer = CandidateLineBuffer::default();
    let mut native_line_state = NativeLineState::default();
    let mut exit_tracker = ExplicitExitTracker::default();
    let classifier = InputClassifier::default();
    let input_generation = UserPtyInputGeneration::default();
    let mut line_submits = LineSubmitCounter::default();
    let mut relay = InputRelayContext {
        master: &mut master,
        input_classifier: &classifier,
        input_events: &tx,
        input_mode: &input_mode,
        input_generation: &input_generation,
        line_submits: &mut line_submits,
        line_buffer: &mut line_buffer,
        native_line_state: &mut native_line_state,
        exit_tracker: &mut exit_tracker,
    };

    relay_prompt_ghost_input(b"\t", "analyze failure", &route, &mut relay)
        .expect("accept agent ghost");
    relay_passthrough_input(b"\x15\n", &mut relay).expect("clear and submit");

    let events = rx.try_iter().collect::<Vec<_>>();
    assert!(events.contains(&RawInputEvent::PromptGhostDismissed));
    assert!(!events
        .iter()
        .any(|event| matches!(event, RawInputEvent::PromptGhostIntercept { .. })));
    assert!(line_buffer.forced_agent_suggestion_id.is_none());
    assert_eq!(fs::read(&path).expect("read test output"), b"\n");
    fs::remove_file(path).ok();
}
