use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use super::start_after_ingestion_ready;

#[test]
fn delayed_readiness_prevents_dependent_work_from_starting() {
    let (waiting_tx, waiting_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();

    let task = thread::spawn(move || {
        start_after_ingestion_ready(
            || {
                waiting_tx.send(()).map_err(|_| "waiting")?;
                release_rx.recv().map_err(|_| "release")
            },
            || Ok(()),
            || started_tx.send(()).map_err(|_| "start"),
            || {},
        )
    });

    waiting_rx.recv().expect("readiness wait should begin");
    assert!(started_rx.try_recv().is_err());
    release_tx.send(()).expect("readiness should be released");
    assert_eq!(started_rx.recv(), Ok(()));
    assert_eq!(task.join().expect("startup task should stop"), Ok(()));
}

#[test]
fn readiness_failure_rolls_back_without_starting_dependent_work() {
    let starts = Arc::new(AtomicUsize::new(0));
    let rollbacks = Arc::new(AtomicUsize::new(0));
    let start_count = Arc::clone(&starts);
    let rollback_count = Arc::clone(&rollbacks);

    let result = start_after_ingestion_ready(
        || Err("readiness timeout"),
        || Ok(()),
        || {
            start_count.fetch_add(1, Ordering::AcqRel);
            Ok(())
        },
        || {
            rollback_count.fetch_add(1, Ordering::AcqRel);
        },
    );

    assert_eq!(result, Err("readiness timeout"));
    assert_eq!(starts.load(Ordering::Acquire), 0);
    assert_eq!(rollbacks.load(Ordering::Acquire), 1);
}

#[test]
fn successful_readiness_starts_dependent_work_exactly_once() {
    let starts = AtomicUsize::new(0);
    let rollbacks = AtomicUsize::new(0);

    let result = start_after_ingestion_ready(
        || Ok::<_, &'static str>(()),
        || Ok(()),
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
