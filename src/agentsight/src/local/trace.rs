//! macOS trace implementation — trajectory collector only (no eBPF).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agentsight_trajectory_collector::{
    CollectorConfig, TrajectoryStore, run_collector_loop, scan_once,
};

/// Run the trajectory collector loop on macOS.
///
/// Opens (or creates) the trajectory SQLite DB, runs one immediate scan,
/// then enters the polling loop until Ctrl+C.
pub fn run_local_trace() {
    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("agentsight")
        .join("trajectories.db");

    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let store = match TrajectoryStore::new_with_path(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to open trajectory store at {db_path:?}: {e}");
            std::process::exit(1);
        }
    };

    let scan_dirs = crate::local::server::local_trajectory_scan_dirs();
    let config = CollectorConfig {
        scan_interval_secs: 300,
        scan_dirs,
        db_path: db_path.clone(),
    };

    scan_once(&store, &config);

    let stop = Arc::new(AtomicBool::new(true));
    let stop_clone = Arc::clone(&stop);

    ctrlc::set_handler(move || {
        println!("\nShutting down trajectory collector...");
        stop_clone.store(false, Ordering::SeqCst);
    })
    .ok();

    println!("Trajectory collector running. Press Ctrl+C to stop.");
    run_collector_loop(&config, &stop);
}
