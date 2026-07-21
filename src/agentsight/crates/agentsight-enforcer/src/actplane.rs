//! Official ActPlane adapter and Linux backend.

use std::collections::HashMap;
use std::fs;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use actplane_ifc_compiler::compile_str;
use agentsight_enforcement_protocol::{
    ApplyPolicy, Binding, BindingState, Effect, HealthStatus, SecurityEvent, ViolationEvent,
};
use ebpf_ifc_engine::capability::{
    AUTH_ADD_LABEL, AUTH_BIND_RULE, AUTH_DECLASSIFY, AUTH_DELEGATE, AUTH_NARROW_SCOPE,
    AUTH_REQUIRE_GATE, CapState, TARGET_CHILD, TARGET_SELF,
};
use ebpf_ifc_engine::{GLOBAL_ACTIVE_DOMAIN_ID, PinnedEngine, ReloadHandle, Violation};
use thiserror::Error;
use uuid::Uuid;

use crate::event_hub::SecurityEventHub;
use crate::{BackendError, EnforcementBackend, EventHub};

/// Exact official upstream revision compiled into this adapter.
pub const ACTPLANE_REVISION: &str = "a62e5d9d96f91101cda019519053e950d532380a";

#[derive(Clone)]
struct ActiveBinding {
    binding: Binding,
    reasons: Vec<String>,
    rule_names: Vec<String>,
}

struct RuntimeState {
    bindings: Mutex<HashMap<u32, ActiveBinding>>,
    events: EventHub,
    security_events: SecurityEventHub,
    runtime_error: Mutex<Option<String>>,
}

impl RuntimeState {
    fn new() -> Self {
        Self {
            bindings: Mutex::new(HashMap::new()),
            events: EventHub::default(),
            security_events: SecurityEventHub::default(),
            runtime_error: Mutex::new(None),
        }
    }

    fn bindings(&self) -> MutexGuard<'_, HashMap<u32, ActiveBinding>> {
        self.bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn runtime_error(&self) -> MutexGuard<'_, Option<String>> {
        self.runtime_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Official pinned ActPlane runtime behind the AgentSight backend contract.
pub struct ActPlaneBackend {
    engine: Arc<PinnedEngine>,
    reload: Arc<ReloadHandle>,
    _runtime_lock: fs::File,
    state: Arc<RuntimeState>,
    stop: Arc<AtomicBool>,
    poller: Mutex<Option<JoinHandle<()>>>,
    lifecycle: Mutex<()>,
}

impl ActPlaneBackend {
    /// Opens or installs the singleton and starts its violation poller.
    ///
    /// # Errors
    ///
    /// Returns a kernel error when the pinned engine, exclusive runtime lock,
    /// reload handle, self-protection, initial cleanup, or event drain cannot be
    /// established.
    pub fn open() -> Result<Self, BackendError> {
        let engine = Arc::new(
            PinnedEngine::open_or_install_singleton()
                .map_err(|error| kernel_error("open pinned singleton", error))?,
        );
        let runtime_lock = engine
            .try_lock_runtime()
            .map_err(|error| kernel_error("lock singleton runtime", error))?;
        let reload = Arc::new(
            engine
                .reload_handle()
                .map_err(|error| kernel_error("open reload handle", error))?,
        );
        engine
            .protect_pid(std::process::id() as i32)
            .map_err(|error| kernel_error("protect enforcer pid", error))?;
        let _ = prepare_runtime(
            || {
                reload
                    .clear_runtime_state()
                    .map_err(|error| kernel_error("clear stale runtime state", error))
            },
            || {
                engine
                    .drain_pending_events()
                    .map_err(|error| kernel_error("drain stale pinned events", error))
            },
        )?;

        let state = Arc::new(RuntimeState::new());
        let stop = Arc::new(AtomicBool::new(false));
        let poller = spawn_poller(Arc::clone(&engine), Arc::clone(&state), Arc::clone(&stop));
        Ok(Self {
            engine,
            reload,
            _runtime_lock: runtime_lock,
            state,
            stop,
            poller: Mutex::new(Some(poller)),
            lifecycle: Mutex::new(()),
        })
    }

    fn lifecycle(&self) -> MutexGuard<'_, ()> {
        self.lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn cleanup_binding(&self, request: &ApplyPolicy, id: u32) -> Vec<String> {
        let mut errors = Vec::new();
        let control_pid = std::process::id() as i32;
        if let Err(error) = self.engine.unbind_pid_from_domain(control_pid, id) {
            errors.push(format!("unbind control pid: {error}"));
        }
        if let Err(error) = self.engine.unbind_pid_from_domain(request.root_pid, id) {
            errors.push(format!("unbind target pid: {error}"));
        }
        if let Err(error) = self.reload.clear_runtime_state() {
            errors.push(format!("clear runtime state: {error}"));
        }
        errors
    }
}

impl EnforcementBackend for ActPlaneBackend {
    fn health(&self) -> Result<HealthStatus, BackendError> {
        let runtime_error = self.state.runtime_error().clone();
        let health = self.state.events.reflect_delivery_loss(HealthStatus {
            ready: runtime_error.is_none(),
            backend: "actplane".into(),
            message: runtime_error,
        });
        Ok(self.state.security_events.reflect_delivery_loss(health))
    }

    fn apply(&self, request: ApplyPolicy) -> Result<Binding, BackendError> {
        let _lifecycle = self.lifecycle();
        let mut bindings = self.state.bindings();
        if let Some(existing) = bindings
            .values()
            .find(|active| active.binding.request.binding_id == request.binding_id)
        {
            return if existing.binding.request == request {
                Ok(existing.binding.clone())
            } else {
                Err(BackendError::BindingConflict(request.binding_id))
            };
        }
        if !bindings.is_empty() {
            return Err(BackendError::BindingConflict(request.binding_id));
        }

        let actual_start = read_process_start_time(request.root_pid)?;
        if actual_start != request.process_start_time {
            return Err(BackendError::StaleProcess {
                pid: request.root_pid,
            });
        }
        let compiled = compile_str(&request.policy_dsl).map_err(BackendError::CompileFailure)?;
        let label = compiled
            .labels
            .get("COMMAND")
            .or_else(|| compiled.labels.get("AGENT"))
            .copied()
            .ok_or_else(|| {
                BackendError::CompileFailure(
                    "policy must declare a COMMAND or AGENT source label".into(),
                )
            })?;
        let id = domain_id(request.binding_id);
        self.engine
            .seed_label_in_domain(request.root_pid, id, label)
            .map_err(|error| kernel_error("seed target process domain", error))?;

        let control_pid = std::process::id() as i32;
        let control_state = CapState {
            scope_id: 1,
            labels: label,
            authority_mask: AUTH_BIND_RULE
                | AUTH_NARROW_SCOPE
                | AUTH_ADD_LABEL
                | AUTH_REQUIRE_GATE
                | AUTH_DECLASSIFY
                | AUTH_DELEGATE,
            target_mask: TARGET_SELF | TARGET_CHILD,
            gate_mask: u64::MAX,
            label_mask: u64::MAX,
            ..CapState::default()
        };
        if let Err(error) = self.engine.bind_state(control_pid, id, control_state) {
            let cleanup = self.cleanup_binding(&request, id);
            return Err(kernel_error_with_cleanup(
                "bind control process",
                error,
                cleanup,
            ));
        }
        if let Err(error) = self
            .reload
            .append_policy_delta(control_pid, id, &compiled.bytes)
        {
            let cleanup = self.cleanup_binding(&request, id);
            return Err(kernel_error_with_cleanup(
                "append policy delta",
                error,
                cleanup,
            ));
        }
        if let Err(error) = self.engine.unbind_pid_from_domain(control_pid, id) {
            let cleanup = self.cleanup_binding(&request, id);
            return Err(kernel_error_with_cleanup(
                "unbind control process",
                error,
                cleanup,
            ));
        }

        let binding = Binding {
            request,
            state: BindingState::Enforced,
            message: None,
            domain_id: Some(id),
        };
        bindings.insert(
            id,
            ActiveBinding {
                binding: binding.clone(),
                reasons: compiled.reasons,
                rule_names: compiled.meta.into_iter().map(|meta| meta.name).collect(),
            },
        );
        Ok(binding)
    }

    fn detach(&self, binding_id: Uuid) -> Result<(), BackendError> {
        let _lifecycle = self.lifecycle();
        let mut bindings = self.state.bindings();
        let Some((id, active)) = bindings
            .iter()
            .find(|(_, active)| active.binding.request.binding_id == binding_id)
            .map(|(id, active)| (*id, active.clone()))
        else {
            return Err(BackendError::MissingBinding(binding_id));
        };
        let cleanup = self.cleanup_binding(&active.binding.request, id);
        if !cleanup.is_empty() {
            return Err(BackendError::KernelFailure(cleanup.join("; ")));
        }
        bindings.remove(&id);
        Ok(())
    }

    fn bindings(&self) -> Result<Vec<Binding>, BackendError> {
        let mut bindings: Vec<_> = self
            .state
            .bindings()
            .values()
            .map(|active| active.binding.clone())
            .collect();
        bindings.sort_by_key(|binding| binding.request.binding_id);
        Ok(bindings)
    }

    fn subscribe(&self) -> Receiver<ViolationEvent> {
        self.state.events.subscribe()
    }

    fn subscribe_security_events(&self) -> Receiver<SecurityEvent> {
        self.state.security_events.subscribe()
    }
}

fn prepare_runtime(
    clear: impl FnOnce() -> Result<(), BackendError>,
    drain: impl FnOnce() -> Result<usize, BackendError>,
) -> Result<usize, BackendError> {
    clear()?;
    drain()
}

impl Drop for ActPlaneBackend {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let poller = self
            .poller
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(poller) = poller
            && poller.join().is_err()
        {
            eprintln!("agentsight-enforcer ActPlane poller panicked during shutdown");
        }
    }
}

fn spawn_poller(
    engine: Arc<PinnedEngine>,
    state: Arc<RuntimeState>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let callback_state = Arc::clone(&state);
        if let Err(error) = engine.run(&stop, move |raw| {
            let active = callback_state.bindings().get(&raw.domain_id).cloned();
            if let Some(active) = active {
                callback_state
                    .events
                    .publish(convert_violation(raw, &active));
            }
        }) {
            *state.runtime_error() = Some(format!("violation poller stopped: {error}"));
        }
    })
}

fn domain_id(binding_id: Uuid) -> u32 {
    let bytes = binding_id.as_bytes();
    let id = bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .fold(0, std::ops::BitXor::bitxor);
    match id {
        0 | GLOBAL_ACTIVE_DOMAIN_ID => 1,
        id => id,
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
enum ProcessStatError {
    #[error("process stat is missing the command terminator")]
    MissingCommand,
    #[error("process stat is missing field 22")]
    MissingStartTime,
    #[error("process stat field 22 is not an unsigned integer")]
    InvalidStartTime,
}

fn parse_process_start_time(stat: &str) -> Result<u64, ProcessStatError> {
    let tail = stat
        .rsplit_once(") ")
        .map(|(_, tail)| tail)
        .ok_or(ProcessStatError::MissingCommand)?;
    tail.split_whitespace()
        .nth(19)
        .ok_or(ProcessStatError::MissingStartTime)?
        .parse()
        .map_err(|_| ProcessStatError::InvalidStartTime)
}

fn read_process_start_time(pid: i32) -> Result<u64, BackendError> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|error| BackendError::KernelFailure(format!("read /proc/{pid}/stat: {error}")))?;
    parse_process_start_time(&stat)
        .map_err(|error| BackendError::KernelFailure(format!("parse /proc/{pid}/stat: {error}")))
}

fn convert_violation(raw: Violation, active: &ActiveBinding) -> ViolationEvent {
    let monotonic_now_ns = monotonic_now_ns();
    let observed_at_ns = now_ns();
    convert_violation_at(raw, active, observed_at_ns, monotonic_now_ns)
}

fn convert_violation_at(
    raw: Violation,
    active: &ActiveBinding,
    observed_at_ns: u64,
    monotonic_now_ns: Option<u64>,
) -> ViolationEvent {
    let rule_index = raw.rule_id as usize;
    let occurred_at_ns = monotonic_now_ns
        .map(|monotonic_now_ns| {
            monotonic_to_epoch_ns(raw.timestamp_ns, monotonic_now_ns, observed_at_ns)
        })
        .unwrap_or(observed_at_ns);
    ViolationEvent {
        event_id: Uuid::new_v4(),
        binding_id: active.binding.request.binding_id,
        agent_id: active.binding.request.agent_id.clone(),
        session_id: active.binding.request.session_id.clone(),
        policy_id: active.binding.request.policy_id.clone(),
        policy_revision: active.binding.request.policy_revision.clone(),
        pid: raw.pid,
        ppid: Some(raw.ppid),
        process_start_time: read_process_start_time(raw.pid).unwrap_or(0),
        operation: operation_name(raw.op).into(),
        target: raw.target,
        effect: effect(raw.effect),
        blocked: raw.blocked,
        killed: raw.killed,
        rule_id: active
            .rule_names
            .get(rule_index)
            .cloned()
            .or_else(|| Some(raw.rule_id.to_string())),
        reason: active.reasons.get(rule_index).cloned(),
        occurred_at_ns,
        observed_at_ns,
        actplane_revision: ACTPLANE_REVISION.into(),
    }
}

fn operation_name(operation: u32) -> &'static str {
    match operation {
        0 => "exec",
        1 => "open",
        2 => "write",
        3 => "connect",
        4 => "recv",
        _ => "unknown",
    }
}

fn effect(value: u32) -> Effect {
    match value {
        1 => Effect::Block,
        2 => Effect::Kill,
        _ => Effect::Notify,
    }
}

fn monotonic_to_epoch_ns(
    event_monotonic_ns: u64,
    monotonic_now_ns: u64,
    realtime_now_ns: u64,
) -> u64 {
    let Some(elapsed_ns) = monotonic_now_ns.checked_sub(event_monotonic_ns) else {
        return realtime_now_ns;
    };
    realtime_now_ns
        .checked_sub(elapsed_ns)
        .unwrap_or(realtime_now_ns)
}

fn monotonic_now_ns() -> Option<u64> {
    let mut timestamp = MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: `clock_gettime` initializes the provided timespec on success.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, timestamp.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: The successful call above initialized `timestamp`.
    let timestamp = unsafe { timestamp.assume_init() };
    let seconds = u64::try_from(timestamp.tv_sec).ok()?;
    let nanoseconds = u64::try_from(timestamp.tv_nsec).ok()?;
    if nanoseconds >= 1_000_000_000 {
        return None;
    }
    seconds.checked_mul(1_000_000_000)?.checked_add(nanoseconds)
}

fn now_ns() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    nanos.min(u64::MAX as u128) as u64
}

fn kernel_error(context: &str, error: std::io::Error) -> BackendError {
    BackendError::KernelFailure(format!("{context}: {error}"))
}

fn kernel_error_with_cleanup(
    context: &str,
    error: std::io::Error,
    cleanup: Vec<String>,
) -> BackendError {
    let mut message = format!("{context}: {error}");
    if !cleanup.is_empty() {
        message.push_str("; cleanup failed: ");
        message.push_str(&cleanup.join("; "));
    }
    BackendError::KernelFailure(message)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use agentsight_enforcement_protocol::{
        ApplyPolicy, Binding, BindingState, Effect, ViolationEvent,
    };
    use ebpf_ifc_engine::Violation;
    use uuid::Uuid;

    use super::*;

    const _: fn(&PinnedEngine) -> std::io::Result<usize> = PinnedEngine::drain_pending_events;

    fn active_binding() -> ActiveBinding {
        ActiveBinding {
            binding: Binding {
                request: ApplyPolicy {
                    binding_id: Uuid::new_v4(),
                    agent_id: "agent-1".into(),
                    session_id: Some("session-1".into()),
                    root_pid: 42,
                    process_start_time: 98765,
                    policy_id: "policy-1".into(),
                    policy_revision: "revision-1".into(),
                    policy_dsl: "fixture".into(),
                },
                state: BindingState::Enforced,
                message: None,
                domain_id: Some(7),
            },
            reasons: vec!["credential reached an external sink".into()],
            rule_names: vec!["block-exfiltration".into()],
        }
    }

    fn raw_violation(timestamp_ns: u64) -> Violation {
        Violation {
            effect: 1,
            blocked: true,
            killed: false,
            comm: "curl".into(),
            pid: 43,
            ppid: 42,
            target: "198.51.100.10".into(),
            rule_id: 0,
            op: 3,
            domain_id: 7,
            session_root: 42,
            label: 1,
            matched_label: 1,
            matched_labels: 1,
            provenance: None,
            timestamp_ns,
        }
    }

    #[test]
    fn domain_id_is_stable_and_nonzero() {
        let id = Uuid::parse_str("00000000-0000-4000-8000-000000000123")
            .expect("fixture UUID should parse");
        assert_eq!(domain_id(id), domain_id(id));
        assert_ne!(domain_id(id), 0);
    }

    #[test]
    fn prepare_runtime_clears_stale_state_before_draining_events() {
        let operations = RefCell::new(Vec::new());

        let drained = prepare_runtime(
            || {
                operations.borrow_mut().push("clear");
                Ok(())
            },
            || {
                operations.borrow_mut().push("drain");
                Ok(3)
            },
        )
        .expect("runtime preparation should succeed");

        assert_eq!(drained, 3);
        assert_eq!(operations.into_inner(), ["clear", "drain"]);
    }

    #[test]
    fn prepare_runtime_skips_drain_when_cleanup_fails() {
        let drain_called = Cell::new(false);

        let result = prepare_runtime(
            || Err(BackendError::KernelFailure("clear failed".into())),
            || {
                drain_called.set(true);
                Ok(0)
            },
        );

        assert!(matches!(
            result,
            Err(BackendError::KernelFailure(message)) if message == "clear failed"
        ));
        assert!(!drain_called.get());
    }

    #[test]
    fn domain_id_never_uses_the_reserved_global_domain() {
        let id = Uuid::parse_str("ffffffff-0000-0000-0000-000000000000")
            .expect("fixture UUID should parse");
        assert_ne!(domain_id(id), GLOBAL_ACTIVE_DOMAIN_ID);
    }

    #[test]
    fn proc_start_time_handles_parenthesized_comm() {
        let stat = "42 (a tricky) name) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 98765";
        assert_eq!(parse_process_start_time(stat), Ok(98765));
    }

    #[test]
    fn monotonic_event_time_is_converted_to_unix_epoch() {
        let occurred_at_ns =
            monotonic_to_epoch_ns(270_000_000_000, 271_000_000_000, 1_784_000_000_000_000_000);

        assert_eq!(occurred_at_ns, 1_783_999_999_000_000_000);
    }

    #[test]
    fn invalid_clock_relationships_fall_back_to_observation_time() {
        assert_eq!(monotonic_to_epoch_ns(30, 20, 5), 5);
        assert_eq!(monotonic_to_epoch_ns(10, 20, 5), 5);
    }

    #[test]
    fn violation_adapter_maps_fields_and_converts_fixed_observation() {
        let active = active_binding();
        let raw = raw_violation(270_000_000_000);

        let event: ViolationEvent = convert_violation_at(
            raw,
            &active,
            1_784_000_000_000_000_000,
            Some(271_000_000_000),
        );
        assert_eq!(event.binding_id, active.binding.request.binding_id);
        assert_eq!(event.agent_id, "agent-1");
        assert_eq!(event.session_id.as_deref(), Some("session-1"));
        assert_eq!(event.policy_id, "policy-1");
        assert_eq!(event.policy_revision, "revision-1");
        assert_eq!(event.pid, 43);
        assert_eq!(event.ppid, Some(42));
        assert_eq!(event.effect, Effect::Block);
        assert!(event.blocked);
        assert!(!event.killed);
        assert_eq!(event.operation, "connect");
        assert_eq!(event.target, "198.51.100.10");
        assert_eq!(event.rule_id.as_deref(), Some("block-exfiltration"));
        assert_eq!(
            event.reason.as_deref(),
            Some("credential reached an external sink")
        );
        assert_eq!(event.occurred_at_ns, 1_783_999_999_000_000_000);
        assert_eq!(event.observed_at_ns, 1_784_000_000_000_000_000);
        assert_eq!(event.actplane_revision, ACTPLANE_REVISION);
    }

    #[test]
    fn violation_adapter_falls_back_when_monotonic_read_fails() {
        let active = active_binding();
        let observed_at_ns = 1_784_000_000_000_000_000;

        let event = convert_violation_at(
            raw_violation(270_000_000_000),
            &active,
            observed_at_ns,
            None,
        );

        assert_eq!(event.occurred_at_ns, observed_at_ns);
        assert_eq!(event.observed_at_ns, observed_at_ns);
    }

    #[test]
    fn violation_adapter_falls_back_for_future_monotonic_event() {
        let observed_at_ns = 1_784_000_000_000_000_000;
        let event = convert_violation_at(
            raw_violation(272_000_000_000),
            &active_binding(),
            observed_at_ns,
            Some(271_000_000_000),
        );

        assert_eq!(event.occurred_at_ns, observed_at_ns);
    }

    #[test]
    fn violation_adapter_falls_back_when_epoch_subtraction_underflows() {
        let observed_at_ns = 5;
        let event = convert_violation_at(
            raw_violation(10),
            &active_binding(),
            observed_at_ns,
            Some(20),
        );

        assert_eq!(event.occurred_at_ns, observed_at_ns);
    }
}
