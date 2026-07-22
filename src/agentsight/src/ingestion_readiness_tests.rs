use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::{GenerationReadiness, IngestionReadinessError, JointReadinessEvent};

#[test]
fn wait_blocks_until_the_current_generation_is_ready() {
    let readiness = GenerationReadiness::new("ingestion unavailable");
    let worker = readiness.candidate();
    readiness.install(worker.clone());
    let waiter = readiness.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();

    let thread = thread::spawn(move || {
        entered_tx.send(()).expect("test should observe waiter");
        result_tx
            .send(waiter.wait_ready(Duration::from_secs(5)))
            .expect("test should receive wait result");
    });
    entered_rx.recv().expect("waiter should start");
    assert!(result_rx.try_recv().is_err());

    assert!(readiness.mark_ready(&worker));
    assert_eq!(result_rx.recv().expect("ready should wake waiter"), Ok(()));
    thread.join().expect("waiter should stop");
}

#[test]
fn stale_generation_cannot_unlock_a_replacement_waiter() {
    let readiness = GenerationReadiness::new("ingestion unavailable");
    let stale = readiness.candidate();
    readiness.install(stale.clone());
    let current = readiness.candidate();
    readiness.install(current.clone());
    let waiter = readiness.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();

    let thread = thread::spawn(move || {
        entered_tx.send(()).expect("test should observe waiter");
        result_tx
            .send(waiter.wait_ready(Duration::from_secs(5)))
            .expect("test should receive wait result");
    });
    entered_rx.recv().expect("waiter should start");
    assert!(!readiness.mark_ready(&stale));
    assert!(result_rx.try_recv().is_err());

    assert!(readiness.mark_ready(&current));
    assert_eq!(
        result_rx.recv().expect("current worker should wake waiter"),
        Ok(())
    );
    thread.join().expect("waiter should stop");
}

#[test]
fn wait_reports_timeout_and_worker_stop_separately() {
    let readiness = GenerationReadiness::new("ingestion unavailable");
    let worker = readiness.candidate();
    readiness.install(worker);
    assert_eq!(
        readiness.wait_ready(Duration::ZERO),
        Err(IngestionReadinessError::Timeout { timeout_ms: 0 })
    );

    readiness.stop();
    assert_eq!(
        readiness.wait_ready(Duration::from_secs(1)),
        Err(IngestionReadinessError::WorkerStopped)
    );
}

#[test]
fn joint_wait_revalidates_a_revoked_generation_before_returning() {
    let enforcement = GenerationReadiness::new("enforcement unavailable");
    let old_enforcement = enforcement.candidate();
    enforcement.install(old_enforcement.clone());
    assert!(enforcement.mark_ready(&old_enforcement));
    let security = GenerationReadiness::new("security unavailable");
    let security_worker = security.candidate();
    security.install(security_worker.clone());
    let gate_enforcement = enforcement.clone();
    let gate_security = security.clone();
    let (phase_tx, phase_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();

    let gate = thread::spawn(move || {
        let mut first_ready = true;
        let result = GenerationReadiness::wait_for_both_ready_observed(
            &gate_enforcement,
            &gate_security,
            Duration::from_secs(5),
            |event| {
                phase_tx.send(event).expect("phase should be observed");
                if event == JointReadinessEvent::FirstReady && first_ready {
                    first_ready = false;
                    resume_rx.recv().expect("gate should resume");
                }
            },
        );
        if result.is_ok() {
            started_tx.send(()).expect("start should be observed");
        }
        result
    });

    assert_eq!(
        phase_rx.recv().expect("first generation should be ready"),
        JointReadinessEvent::FirstReady
    );
    let replacement = enforcement.candidate();
    enforcement.install(replacement.clone());
    resume_tx.send(()).expect("gate should continue");
    assert!(security.mark_ready(&security_worker));
    while phase_rx.recv().expect("gate should revalidate") != JointReadinessEvent::Retrying {}
    assert!(started_rx.try_recv().is_err());

    assert!(enforcement.mark_ready(&replacement));
    assert_eq!(started_rx.recv(), Ok(()));
    assert_eq!(gate.join().expect("gate should stop"), Ok(()));
}
