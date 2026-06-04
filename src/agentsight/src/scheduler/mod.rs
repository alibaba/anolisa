//! Agent activity monitor.
//!
//! Tracks the scheduling state of Agent process families using per-thread
//! sleep/wakeup events from the schedmon BPF probe. Exports idle/active
//! transitions as observability signals (log + metrics). Does NOT actuate
//! cgroup changes — CPU scheduling policy belongs in the container spec.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::probes::schedmon::{SCHED_EVENT_SLEEP, SCHED_EVENT_WAKEUP};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityState {
    Active,
    Idle,
}

#[derive(Debug, Clone)]
pub struct ActivityConfig {
    pub enabled: bool,
    pub idle_threshold_ms: u64,
}

impl Default for ActivityConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            idle_threshold_ms: 50,
        }
    }
}

/// Per-family activity metrics.
#[derive(Debug, Clone, Default)]
pub struct FamilyMetrics {
    pub idle_to_active_count: u64,
    pub active_to_idle_count: u64,
    pub last_active_duration_ns: u64,
    pub last_idle_duration_ns: u64,
}

struct FamilyState {
    member_pids: HashSet<u32>,
    active_tids: HashSet<u32>,
    state: ActivityState,
    idle_since: Option<Instant>,
    last_transition: Instant,
    metrics: FamilyMetrics,
}

pub struct ActivityMonitor {
    config: ActivityConfig,
    families: HashMap<u32, FamilyState>,
    pid_to_root: HashMap<u32, u32>,
}

impl ActivityMonitor {
    pub fn new(config: ActivityConfig) -> Self {
        Self {
            config,
            families: HashMap::new(),
            pid_to_root: HashMap::new(),
        }
    }

    pub fn add_process(&mut self, pid: u32, root_pid: u32) {
        self.pid_to_root.insert(pid, root_pid);

        let family = self.families.entry(root_pid).or_insert_with(|| FamilyState {
            member_pids: HashSet::new(),
            active_tids: HashSet::new(),
            state: ActivityState::Active,
            idle_since: None,
            last_transition: Instant::now(),
            metrics: FamilyMetrics::default(),
        });

        family.member_pids.insert(pid);
        family.active_tids.insert(pid);
        family.idle_since = None;
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
                log::debug!(
                    "activity: family {root_pid} removed (idle_to_active={}, active_to_idle={})",
                    family.metrics.idle_to_active_count,
                    family.metrics.active_to_idle_count,
                );
            }
        }
    }

    pub fn on_sched_event(&mut self, tgid: u32, tid: u32, event_type: u8) {
        let root_pid = match self.pid_to_root.get(&tgid) {
            Some(&r) => r,
            None => return,
        };

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
            if family.state == ActivityState::Active && family.idle_since.is_none() {
                family.idle_since = Some(Instant::now());
            }
            return;
        }

        // A member is runnable: cancel pending idle.
        family.idle_since = None;
        if family.state == ActivityState::Idle {
            let idle_duration = family.last_transition.elapsed();
            family.state = ActivityState::Active;
            family.last_transition = Instant::now();
            family.metrics.idle_to_active_count += 1;
            family.metrics.last_idle_duration_ns = idle_duration.as_nanos() as u64;
            log::debug!(
                "activity: family {root_pid} IDLE -> ACTIVE (idle_ms={})",
                idle_duration.as_millis(),
            );
        }
    }

    pub fn tick(&mut self) {
        let threshold = std::time::Duration::from_millis(self.config.idle_threshold_ms);

        for (&root_pid, family) in &mut self.families {
            if family.state == ActivityState::Idle || !family.active_tids.is_empty() {
                continue;
            }
            match family.idle_since {
                Some(t) if t.elapsed() >= threshold => {}
                _ => continue,
            }

            let active_duration = family.last_transition.elapsed();
            family.state = ActivityState::Idle;
            family.last_transition = Instant::now();
            family.metrics.active_to_idle_count += 1;
            family.metrics.last_active_duration_ns = active_duration.as_nanos() as u64;
            log::debug!(
                "activity: family {root_pid} ACTIVE -> IDLE (active_ms={})",
                active_duration.as_millis(),
            );
        }
    }

    pub fn family_count(&self) -> usize {
        self.families.len()
    }

    pub fn family_state(&self, root_pid: u32) -> Option<ActivityState> {
        self.families.get(&root_pid).map(|f| f.state)
    }

    pub fn family_metrics(&self, root_pid: u32) -> Option<&FamilyMetrics> {
        self.families.get(&root_pid).map(|f| &f.metrics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_config() -> ActivityConfig {
        ActivityConfig {
            enabled: true,
            idle_threshold_ms: 10,
        }
    }

    fn make_monitor() -> ActivityMonitor {
        ActivityMonitor::new(test_config())
    }

    fn force_idle_window(monitor: &mut ActivityMonitor, root_pid: u32) {
        if let Some(family) = monitor.families.get_mut(&root_pid) {
            family.idle_since = Some(Instant::now() - Duration::from_millis(20));
        }
    }

    #[test]
    fn test_add_remove_process() {
        let mut mon = make_monitor();

        mon.add_process(100, 100);
        assert_eq!(mon.family_count(), 1);
        assert_eq!(mon.pid_to_root.get(&100), Some(&100));

        mon.add_process(200, 100);
        assert_eq!(mon.family_count(), 1);

        mon.remove_process(200);
        assert_eq!(mon.family_count(), 1);

        mon.remove_process(100);
        assert_eq!(mon.family_count(), 0);
    }

    #[test]
    fn test_sched_event_active() {
        let mut mon = make_monitor();
        mon.add_process(100, 100);

        assert_eq!(mon.family_state(100), Some(ActivityState::Active));

        mon.on_sched_event(100, 100, SCHED_EVENT_SLEEP);
        assert_eq!(mon.family_state(100), Some(ActivityState::Active));

        mon.on_sched_event(100, 100, SCHED_EVENT_WAKEUP);
        assert_eq!(mon.family_state(100), Some(ActivityState::Active));
    }

    #[test]
    fn test_sched_event_idle_after_threshold() {
        let mut mon = make_monitor();
        mon.add_process(100, 100);

        mon.on_sched_event(100, 100, SCHED_EVENT_SLEEP);
        force_idle_window(&mut mon, 100);

        mon.tick();
        assert_eq!(mon.family_state(100), Some(ActivityState::Idle));
        assert_eq!(mon.family_metrics(100).unwrap().active_to_idle_count, 1);
    }

    #[test]
    fn test_wakeup_cancels_pending_idle() {
        let mut mon = make_monitor();
        mon.add_process(100, 100);

        mon.on_sched_event(100, 100, SCHED_EVENT_SLEEP);
        mon.on_sched_event(100, 100, SCHED_EVENT_WAKEUP);
        force_idle_window(&mut mon, 100);
        mon.tick();
        assert_eq!(mon.family_state(100), Some(ActivityState::Active));
    }

    #[test]
    fn test_multithreaded_active_while_any_thread_runs() {
        let mut mon = make_monitor();
        mon.add_process(100, 100);

        mon.on_sched_event(100, 101, SCHED_EVENT_WAKEUP);
        mon.on_sched_event(100, 100, SCHED_EVENT_SLEEP);
        assert_eq!(mon.family_state(100), Some(ActivityState::Active));

        mon.on_sched_event(100, 101, SCHED_EVENT_SLEEP);
        force_idle_window(&mut mon, 100);
        mon.tick();
        assert_eq!(mon.family_state(100), Some(ActivityState::Idle));

        mon.on_sched_event(100, 101, SCHED_EVENT_WAKEUP);
        assert_eq!(mon.family_state(100), Some(ActivityState::Active));
        assert_eq!(mon.family_metrics(100).unwrap().idle_to_active_count, 1);
    }

    #[test]
    fn test_family_active_if_any_active() {
        let mut mon = make_monitor();
        mon.add_process(100, 100);
        mon.add_process(200, 100);

        assert_eq!(mon.family_state(100), Some(ActivityState::Active));

        mon.on_sched_event(100, 100, SCHED_EVENT_SLEEP);
        assert_eq!(mon.family_state(100), Some(ActivityState::Active));

        mon.on_sched_event(200, 200, SCHED_EVENT_SLEEP);
        assert_eq!(mon.family_state(100), Some(ActivityState::Active));

        force_idle_window(&mut mon, 100);
        mon.tick();
        assert_eq!(mon.family_state(100), Some(ActivityState::Idle));

        mon.on_sched_event(200, 200, SCHED_EVENT_WAKEUP);
        assert_eq!(mon.family_state(100), Some(ActivityState::Active));
    }

    #[test]
    fn test_remove_nonexistent_process() {
        let mut mon = make_monitor();
        mon.remove_process(999);
        assert_eq!(mon.family_count(), 0);
    }

    #[test]
    fn test_sched_event_unknown_pid() {
        let mut mon = make_monitor();
        mon.on_sched_event(999, 999, SCHED_EVENT_WAKEUP);
        assert_eq!(mon.family_count(), 0);
    }

    #[test]
    fn test_multiple_families() {
        let mut mon = make_monitor();
        mon.add_process(100, 100);
        mon.add_process(200, 200);
        assert_eq!(mon.family_count(), 2);

        mon.on_sched_event(100, 100, SCHED_EVENT_SLEEP);
        force_idle_window(&mut mon, 100);
        mon.tick();

        assert_eq!(mon.family_state(100), Some(ActivityState::Idle));
        assert_eq!(mon.family_state(200), Some(ActivityState::Active));
    }
}
