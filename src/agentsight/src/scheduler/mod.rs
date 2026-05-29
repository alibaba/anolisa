//! Idle-burst-idle scheduler.
//!
//! Groups the processes of an Agent family (keyed by the Agent root PID) into a
//! cgroup v2 cpu cgroup and drives `cpu.weight` / `cpu.idle` from the family's
//! aggregate scheduling state: ACTIVE while any member is runnable, IDLE once
//! every member has been blocked for longer than `idle_threshold_ms`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::probes::schedmon::{SCHED_EVENT_SLEEP, SCHED_EVENT_WAKEUP};

/// cgroup v2 cpu.weight valid range.
const CPU_WEIGHT_MIN: u32 = 1;
const CPU_WEIGHT_MAX: u32 = 10000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedState {
    Active,
    Idle,
}

#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    pub enabled: bool,
    pub active_weight: u32,
    pub idle_threshold_ms: u64,
    pub cgroup_root: PathBuf,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            active_weight: 200,
            idle_threshold_ms: 50,
            cgroup_root: PathBuf::from("/sys/fs/cgroup/agentsight"),
        }
    }
}

struct FamilyState {
    cgroup_path: PathBuf,
    /// Process ids (tgids) classified into this family, for cgroup membership.
    member_pids: HashSet<u32>,
    /// Thread ids currently runnable. The family is ACTIVE while this is
    /// non-empty, so a multithreaded process stays ACTIVE as long as any one of
    /// its threads is running even if others are blocked.
    active_tids: HashSet<u32>,
    state: SchedState,
    /// Set to the instant `active_tids` became empty while ACTIVE (pending idle).
    /// Cleared as soon as any thread becomes runnable again. The debounced IDLE
    /// transition in `tick()` fires only after this has aged past the threshold.
    idle_since: Option<Instant>,
    cgroup_created: bool,
}

pub struct Scheduler {
    config: SchedulerConfig,
    families: HashMap<u32, FamilyState>,
    pid_to_root: HashMap<u32, u32>,
}

impl Scheduler {
    pub fn new(config: SchedulerConfig) -> Self {
        // Sweep cgroups leaked by a previous non-graceful exit (SIGKILL/crash):
        // remove now-empty agent-* directories under cgroup_root. Non-empty ones
        // (a stale escaped process, or another live instance) are left untouched.
        sweep_stale_cgroups(&config.cgroup_root);
        Self {
            config,
            families: HashMap::new(),
            pid_to_root: HashMap::new(),
        }
    }

    pub fn add_process(&mut self, pid: u32, root_pid: u32) {
        self.pid_to_root.insert(pid, root_pid);

        let active_weight = self.config.active_weight;
        let family = self.families.entry(root_pid).or_insert_with(|| {
            let cgroup_path = self.config.cgroup_root.join(format!("agent-{root_pid}"));
            FamilyState {
                cgroup_path,
                member_pids: HashSet::new(),
                active_tids: HashSet::new(),
                state: SchedState::Active,
                idle_since: None,
                cgroup_created: false,
            }
        });

        family.member_pids.insert(pid);
        // Seed the process's main thread (tid == tgid == pid) as runnable. Per-tid
        // sched events refine the set from here. A family classified late while
        // already idle starts ACTIVE and self-corrects on its next wake/sleep
        // cycle — the cost is at most one delayed idle window.
        family.active_tids.insert(pid);
        family.idle_since = None;

        if !family.cgroup_created {
            if let Err(e) = create_cgroup(&family.cgroup_path) {
                log::warn!("failed to create cgroup {:?}: {e}", family.cgroup_path);
                return;
            }
            if let Err(e) = write_weight(&family.cgroup_path, active_weight) {
                log::warn!("failed to set initial cpu.weight: {e}");
            }
            if let Err(e) = write_cpu_idle(&family.cgroup_path, false) {
                log::debug!("cpu.idle not available: {e}");
            }
            family.cgroup_created = true;
        }

        if let Err(e) = migrate_pid(&family.cgroup_path, pid) {
            log::debug!("failed to migrate pid {pid} to cgroup: {e}");
        }
    }

    pub fn remove_process(&mut self, pid: u32) {
        let root_pid = match self.pid_to_root.remove(&pid) {
            Some(r) => r,
            None => return,
        };

        let should_remove = if let Some(family) = self.families.get_mut(&root_pid) {
            family.member_pids.remove(&pid);
            family.active_tids.remove(&pid);
            family.member_pids.is_empty()
        } else {
            false
        };

        if should_remove {
            if let Some(family) = self.families.remove(&root_pid) {
                if family.cgroup_created {
                    if let Err(e) = remove_cgroup(&family.cgroup_path) {
                        log::warn!("failed to remove cgroup {:?}: {e}", family.cgroup_path);
                    }
                }
            }
        }
    }

    /// Handle a per-thread scheduler event. `tgid` selects the family; `tid` is
    /// tracked in the family's runnable set so the family is ACTIVE while any of
    /// its threads runs.
    pub fn on_sched_event(&mut self, tgid: u32, tid: u32, event_type: u8) {
        let root_pid = match self.pid_to_root.get(&tgid) {
            Some(&r) => r,
            None => return,
        };

        let active_weight = self.config.active_weight;
        let family = match self.families.get_mut(&root_pid) {
            Some(f) => f,
            None => return,
        };

        match event_type {
            SCHED_EVENT_WAKEUP => {
                family.active_tids.insert(tid);
            }
            SCHED_EVENT_SLEEP => {
                family.active_tids.remove(&tid);
            }
            _ => return,
        }

        if family.active_tids.is_empty() {
            // All members blocked. Start the debounce window if we are not
            // already counting; the actual IDLE write happens in tick().
            if family.state == SchedState::Active && family.idle_since.is_none() {
                family.idle_since = Some(Instant::now());
            }
            return;
        }

        // A member is runnable again: cancel any pending idle.
        family.idle_since = None;
        if family.state == SchedState::Idle {
            // ACTIVE transition: clear cpu.idle BEFORE setting cpu.weight (the
            // kernel rejects weight writes while cpu.idle=1).
            family.state = SchedState::Active;
            if family.cgroup_created {
                if let Err(e) = write_cpu_idle(&family.cgroup_path, false) {
                    log::debug!("cpu.idle write failed: {e}");
                }
                if let Err(e) = write_weight(&family.cgroup_path, active_weight) {
                    log::warn!("failed to set active weight: {e}");
                }
                log::debug!("scheduler: family {root_pid} -> ACTIVE (weight={active_weight})");
            }
        }
    }

    /// Reap families whose cgroup has no remaining processes (all members exited).
    /// This does not depend on process-exit events reaching us — proctrace only
    /// emits exit for its own child_pids, so an Agent root detected via AGENT_MODE
    /// would otherwise never be torn down. An empty cgroup.procs unambiguously
    /// means the whole family is gone (sleeping members are still listed).
    fn reap_exited_families(&mut self) {
        let dead: Vec<u32> = self
            .families
            .iter()
            .filter(|(_, f)| f.cgroup_created && cgroup_is_empty(&f.cgroup_path))
            .map(|(&root, _)| root)
            .collect();
        for root in dead {
            if let Some(family) = self.families.remove(&root) {
                if let Err(e) = remove_cgroup(&family.cgroup_path) {
                    log::debug!("reap: failed to remove cgroup {:?}: {e}", family.cgroup_path);
                } else {
                    log::debug!("scheduler: reaped exited family {root}");
                }
            }
            self.pid_to_root.retain(|_, &mut r| r != root);
        }
    }

    /// Called periodically from the event loop to finalize debounced idle transitions.
    pub fn tick(&mut self) {
        self.reap_exited_families();
        let threshold = std::time::Duration::from_millis(self.config.idle_threshold_ms);

        for (&root_pid, family) in &mut self.families {
            if family.state == SchedState::Idle || !family.active_tids.is_empty() {
                continue;
            }
            match family.idle_since {
                Some(t) if t.elapsed() >= threshold => {}
                _ => continue,
            }

            family.state = SchedState::Idle;
            if family.cgroup_created {
                // Idle via cpu.idle (SCHED_IDLE): the family only runs when nothing
                // else wants the CPU. We do NOT write cpu.weight here — the kernel
                // rejects cpu.weight writes while cpu.idle=1 and ignores the value
                // anyway. cpu.weight is restored on the ACTIVE transition.
                if let Err(e) = write_cpu_idle(&family.cgroup_path, true) {
                    log::warn!("failed to set cpu.idle: {e}");
                }
                log::debug!("scheduler: family {root_pid} -> IDLE (cpu.idle=1)");
            }
        }
    }

    pub fn family_count(&self) -> usize {
        self.families.len()
    }

    pub fn family_state(&self, root_pid: u32) -> Option<SchedState> {
        self.families.get(&root_pid).map(|f| f.state)
    }
}

impl Drop for Scheduler {
    /// Evacuate every family's processes back to the cgroup v2 root and remove
    /// the agent-* directories plus the (now-empty) cgroup_root, so we do not
    /// leak cgroups or leave migrated PIDs pinned at idle weight after shutdown.
    fn drop(&mut self) {
        let mut any_created = false;
        for family in self.families.values() {
            if family.cgroup_created {
                any_created = true;
                if let Err(e) = remove_cgroup(&family.cgroup_path) {
                    log::debug!("failed to remove cgroup {:?} on shutdown: {e}", family.cgroup_path);
                }
            }
        }
        if any_created && self.config.cgroup_root.exists() {
            let _ = std::fs::remove_dir(&self.config.cgroup_root);
        }
    }
}

/// Find the cgroup v2 mount root by walking up from `start` while each directory
/// still exposes `cgroup.subtree_control` (i.e. is itself a cgroup v2 node).
fn cgroup_v2_root(start: &Path) -> Option<PathBuf> {
    let mut root: Option<PathBuf> = None;
    let mut cur = Some(start.to_path_buf());
    while let Some(dir) = cur {
        if !dir.join("cgroup.subtree_control").exists() {
            break;
        }
        root = Some(dir.clone());
        cur = dir.parent().map(|p| p.to_path_buf());
    }
    root
}

/// Enable the `cpu` controller in every cgroup from the v2 root down to (and
/// including) `target`, so that `target`'s children expose cpu.weight/cpu.idle.
/// Each level must be enabled before its child can inherit the controller, so we
/// walk top-down. Real write failures are surfaced (the controller silently
/// missing is the failure mode that makes the whole scheduler a no-op).
fn enable_cpu_controller(target: &Path) {
    // Build the chain target -> ... -> v2 root, then enable top-down.
    let mut chain: Vec<PathBuf> = Vec::new();
    let mut cur = Some(target.to_path_buf());
    while let Some(dir) = cur {
        if !dir.join("cgroup.subtree_control").exists() {
            break;
        }
        chain.push(dir.clone());
        cur = dir.parent().map(|p| p.to_path_buf());
    }
    for dir in chain.iter().rev() {
        let subtree_ctl = dir.join("cgroup.subtree_control");
        // Writing "+cpu" when already enabled is a no-op; only a genuine failure
        // (controller not available at this level, permissions) is worth logging.
        if let Err(e) = std::fs::write(&subtree_ctl, "+cpu") {
            log::warn!("failed to enable cpu controller at {subtree_ctl:?}: {e}");
        }
    }
}

/// True if the cgroup currently holds no processes. Unreadable cgroup.procs
/// (e.g. the dir is gone) returns false so we never reap on a transient error.
fn cgroup_is_empty(path: &Path) -> bool {
    match std::fs::read_to_string(path.join("cgroup.procs")) {
        Ok(s) => s.split_whitespace().next().is_none(),
        Err(_) => false,
    }
}

/// Remove empty `agent-*` cgroups left under `cgroup_root` by a previous
/// non-graceful exit. Best-effort: rmdir skips non-empty directories, so a live
/// instance's families or an escaped process are never disturbed.
fn sweep_stale_cgroups(cgroup_root: &Path) {
    let entries = match std::fs::read_dir(cgroup_root) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("agent-"))
        {
            if std::fs::remove_dir(&path).is_ok() {
                log::debug!("scheduler: swept stale cgroup {path:?}");
            }
        }
    }
}

fn create_cgroup(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    // Enable the cpu controller down to the parent so this leaf gets cpu.* files.
    if let Some(parent) = path.parent() {
        enable_cpu_controller(parent);
    }
    Ok(())
}

/// Move any processes still resident in `path` back to the cgroup v2 root, then
/// rmdir it. rmdir fails with EBUSY while a cgroup still holds processes (e.g.
/// fork-without-exec children that were never tracked), so evacuation first.
fn remove_cgroup(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if let Some(root) = cgroup_v2_root(path) {
        let root_procs = root.join("cgroup.procs");
        if let Ok(procs) = std::fs::read_to_string(path.join("cgroup.procs")) {
            for pid in procs.split_whitespace() {
                // Best-effort: a pid may have exited between read and write.
                let _ = std::fs::write(&root_procs, pid);
            }
        }
    }
    std::fs::remove_dir(path)
}

fn write_weight(cgroup_path: &Path, weight: u32) -> std::io::Result<()> {
    let clamped = weight.clamp(CPU_WEIGHT_MIN, CPU_WEIGHT_MAX);
    if clamped != weight {
        log::warn!("cpu.weight {weight} out of range [{CPU_WEIGHT_MIN}, {CPU_WEIGHT_MAX}], clamped to {clamped}");
    }
    std::fs::write(cgroup_path.join("cpu.weight"), clamped.to_string())
}

fn write_cpu_idle(cgroup_path: &Path, idle: bool) -> std::io::Result<()> {
    let val = if idle { "1" } else { "0" };
    std::fs::write(cgroup_path.join("cpu.idle"), val)
}

fn migrate_pid(cgroup_path: &Path, pid: u32) -> std::io::Result<()> {
    std::fs::write(cgroup_path.join("cgroup.procs"), pid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_config() -> SchedulerConfig {
        // Unique cgroup_root per scheduler so the (real) filesystem operations in
        // sweep/reap/migrate don't race across tests that share pid numbers.
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        SchedulerConfig {
            enabled: true,
            active_weight: 200,
            idle_threshold_ms: 10,
            cgroup_root: PathBuf::from(format!("/tmp/test-cgroup-agentsight-{n}")),
        }
    }

    fn make_scheduler() -> Scheduler {
        Scheduler::new(test_config())
    }

    /// Force a family into the "all members idle for long enough" condition.
    fn force_idle_window(sched: &mut Scheduler, root_pid: u32) {
        if let Some(family) = sched.families.get_mut(&root_pid) {
            family.idle_since = Some(Instant::now() - Duration::from_millis(20));
        }
    }

    #[test]
    fn test_add_remove_process() {
        let mut sched = make_scheduler();

        sched.add_process(100, 100);
        assert_eq!(sched.family_count(), 1);
        assert_eq!(sched.pid_to_root.get(&100), Some(&100));

        sched.add_process(200, 100);
        assert_eq!(sched.family_count(), 1);

        sched.remove_process(200);
        assert_eq!(sched.family_count(), 1);

        sched.remove_process(100);
        assert_eq!(sched.family_count(), 0);
    }

    #[test]
    fn test_sched_event_active() {
        let mut sched = make_scheduler();
        sched.add_process(100, 100);

        // Initially active
        assert_eq!(sched.family_state(100), Some(SchedState::Active));

        // Sleep -> all idle, but debounced (not yet idle)
        sched.on_sched_event(100, 100, SCHED_EVENT_SLEEP);
        assert_eq!(sched.family_state(100), Some(SchedState::Active));

        // Wakeup -> stays active
        sched.on_sched_event(100, 100, SCHED_EVENT_WAKEUP);
        assert_eq!(sched.family_state(100), Some(SchedState::Active));
    }

    #[test]
    fn test_sched_event_idle_after_threshold() {
        let mut sched = make_scheduler();
        sched.add_process(100, 100);

        sched.on_sched_event(100, 100, SCHED_EVENT_SLEEP);
        force_idle_window(&mut sched, 100);

        sched.tick();
        assert_eq!(sched.family_state(100), Some(SchedState::Idle));
    }

    #[test]
    fn test_wakeup_cancels_pending_idle() {
        let mut sched = make_scheduler();
        sched.add_process(100, 100);

        // Sleep starts the debounce window.
        sched.on_sched_event(100, 100, SCHED_EVENT_SLEEP);
        // A wakeup before the threshold must cancel the pending idle.
        sched.on_sched_event(100, 100, SCHED_EVENT_WAKEUP);
        force_idle_window(&mut sched, 100); // would be stale, but window was cleared
        sched.tick();
        assert_eq!(sched.family_state(100), Some(SchedState::Active));
    }

    #[test]
    fn test_multithreaded_active_while_any_thread_runs() {
        // One process (tgid 100) with a main thread (tid 100) and a worker (tid 101).
        let mut sched = make_scheduler();
        sched.add_process(100, 100);

        // Worker starts running.
        sched.on_sched_event(100, 101, SCHED_EVENT_WAKEUP);
        // Main (coordinator) thread blocks, but the worker keeps running:
        // the family MUST stay ACTIVE (the bug this fix addresses).
        sched.on_sched_event(100, 100, SCHED_EVENT_SLEEP);
        assert_eq!(sched.family_state(100), Some(SchedState::Active));

        // Worker also blocks -> all threads idle -> after threshold -> IDLE.
        sched.on_sched_event(100, 101, SCHED_EVENT_SLEEP);
        force_idle_window(&mut sched, 100);
        sched.tick();
        assert_eq!(sched.family_state(100), Some(SchedState::Idle));

        // Worker wakes -> family ACTIVE again (multithreaded wakeup re-activates).
        sched.on_sched_event(100, 101, SCHED_EVENT_WAKEUP);
        assert_eq!(sched.family_state(100), Some(SchedState::Active));
    }

    #[test]
    fn test_family_active_if_any_active() {
        let mut sched = make_scheduler();
        sched.add_process(100, 100);
        sched.add_process(200, 100);

        // Both active initially
        assert_eq!(sched.family_state(100), Some(SchedState::Active));

        // One sleeps, family still active
        sched.on_sched_event(100, 100, SCHED_EVENT_SLEEP);
        assert_eq!(sched.family_state(100), Some(SchedState::Active));

        // Both sleep -> still active due to debounce
        sched.on_sched_event(200, 200, SCHED_EVENT_SLEEP);
        assert_eq!(sched.family_state(100), Some(SchedState::Active));

        // After threshold
        force_idle_window(&mut sched, 100);
        sched.tick();
        assert_eq!(sched.family_state(100), Some(SchedState::Idle));

        // One wakes -> immediately active
        sched.on_sched_event(200, 200, SCHED_EVENT_WAKEUP);
        assert_eq!(sched.family_state(100), Some(SchedState::Active));
    }

    #[test]
    fn test_weight_values() {
        let config = test_config();
        assert_eq!(config.active_weight, 200);
    }

    #[test]
    fn test_remove_nonexistent_process() {
        let mut sched = make_scheduler();
        sched.remove_process(999);
        assert_eq!(sched.family_count(), 0);
    }

    #[test]
    fn test_sched_event_unknown_pid() {
        let mut sched = make_scheduler();
        sched.on_sched_event(999, 999, SCHED_EVENT_WAKEUP);
        assert_eq!(sched.family_count(), 0);
    }

    #[test]
    fn test_multiple_families() {
        let mut sched = make_scheduler();
        sched.add_process(100, 100);
        sched.add_process(200, 200);
        assert_eq!(sched.family_count(), 2);

        sched.on_sched_event(100, 100, SCHED_EVENT_SLEEP);
        force_idle_window(&mut sched, 100);
        sched.tick();

        assert_eq!(sched.family_state(100), Some(SchedState::Idle));
        assert_eq!(sched.family_state(200), Some(SchedState::Active));
    }
}
