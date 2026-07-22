use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::{GenerationReadiness, IngestionReadinessError};

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
