use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use super::start_after_ingestion_ready;
use crate::ingestion_readiness::GenerationReadiness;

#[test]
fn delayed_readiness_prevents_dependent_work_from_starting() {
    let (waiting_tx, waiting_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let enforcement = GenerationReadiness::new("enforcement unavailable");
    let enforcement_worker = enforcement.candidate();
    enforcement.install(enforcement_worker.clone());
    let security = GenerationReadiness::new("security unavailable");
    let security_worker = security.candidate();
    security.install(security_worker.clone());
    let gate_enforcement = enforcement.clone();
    let gate_security = security.clone();

    let task = thread::spawn(move || {
        waiting_tx.send(()).map_err(|_| "waiting")?;
        start_after_ingestion_ready(
            &gate_enforcement,
            &gate_security,
            Duration::from_secs(5),
            |_| "readiness",
            || started_tx.send(()).map_err(|_| "start"),
            || {},
        )
    });

    waiting_rx.recv().expect("readiness wait should begin");
    assert!(started_rx.try_recv().is_err());
    assert!(enforcement.mark_ready(&enforcement_worker));
    assert!(security.mark_ready(&security_worker));
    assert_eq!(started_rx.recv(), Ok(()));
    assert_eq!(task.join().expect("startup task should stop"), Ok(()));
}

#[test]
fn readiness_failure_rolls_back_without_starting_dependent_work() {
    let starts = Arc::new(AtomicUsize::new(0));
    let rollbacks = Arc::new(AtomicUsize::new(0));
    let start_count = Arc::clone(&starts);
    let rollback_count = Arc::clone(&rollbacks);
    let enforcement = GenerationReadiness::new("enforcement unavailable");
    let security = GenerationReadiness::new("security unavailable");

    let result = start_after_ingestion_ready(
        &enforcement,
        &security,
        Duration::from_secs(1),
        |_| "readiness unavailable",
        || {
            start_count.fetch_add(1, Ordering::AcqRel);
            Ok(())
        },
        || {
            rollback_count.fetch_add(1, Ordering::AcqRel);
        },
    );

    assert_eq!(result, Err("readiness unavailable"));
    assert_eq!(starts.load(Ordering::Acquire), 0);
    assert_eq!(rollbacks.load(Ordering::Acquire), 1);
}

#[test]
fn successful_readiness_starts_dependent_work_exactly_once() {
    let starts = AtomicUsize::new(0);
    let rollbacks = AtomicUsize::new(0);
    let enforcement = GenerationReadiness::new("enforcement unavailable");
    let enforcement_worker = enforcement.candidate();
    enforcement.install(enforcement_worker.clone());
    assert!(enforcement.mark_ready(&enforcement_worker));
    let security = GenerationReadiness::new("security unavailable");
    let security_worker = security.candidate();
    security.install(security_worker.clone());
    assert!(security.mark_ready(&security_worker));

    let result = start_after_ingestion_ready(
        &enforcement,
        &security,
        Duration::from_secs(1),
        |_| "readiness unavailable",
        || {
            starts.fetch_add(1, Ordering::AcqRel);
            Ok("worker")
        },
        || {
            rollbacks.fetch_add(1, Ordering::AcqRel);
        },
    );

    assert_eq!(result, Ok("worker"));
    assert_eq!(starts.load(Ordering::Acquire), 1);
    assert_eq!(rollbacks.load(Ordering::Acquire), 0);
}
