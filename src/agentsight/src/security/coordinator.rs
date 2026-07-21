//! Ingests normalized enforcer events and creates explainable risk cases.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agentsight_enforcement_protocol::{
    DestinationClass, EnforcementStateEvent, EventIdentity, PolicyMode, SecurityEvent,
    SecurityEventKind,
};
use thiserror::Error;

use super::{
    RiskCase, RiskCaseStatus, RiskSeverity, SecurityEventFilter, SecurityStore, SecurityStoreError,
};
use crate::enforcement::EnforcementClient;

/// Security ingestion and correlation failures.
#[derive(Debug, Error)]
pub enum SecurityCoordinatorError {
    /// Local persistence or query failed.
    #[error(transparent)]
    Store(#[from] SecurityStoreError),
    /// A decision referenced evidence that has not been persisted.
    #[error("security decision references missing {kind} event {event_id}")]
    MissingEvidence {
        /// Expected evidence role.
        kind: &'static str,
        /// Missing stable event identifier.
        event_id: uuid::Uuid,
    },
    /// A second subscription worker was requested for the same coordinator.
    #[error("security coordinator is already running")]
    AlreadyRunning,
    /// Spawning the subscription worker failed.
    #[error("failed to start security coordinator: {0}")]
    Thread(#[from] std::io::Error),
}

/// AgentSight-owned normalized-event ingestor and risk correlator.
pub struct SecurityCoordinator {
    client: EnforcementClient,
    store: Arc<SecurityStore>,
    stop: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    last_context: Arc<Mutex<Option<EventContext>>>,
}

#[derive(Clone)]
struct EventContext {
    identity: EventIdentity,
    policy_id: Option<String>,
    policy_revision: Option<u64>,
}

impl SecurityCoordinator {
    /// Creates a coordinator for one local enforcer and security store.
    pub fn new(client: EnforcementClient, store: Arc<SecurityStore>) -> Self {
        Self {
            client,
            store,
            stop: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            last_context: Arc::new(Mutex::new(None)),
        }
    }

    /// Returns the configured enforcer client.
    pub fn client(&self) -> &EnforcementClient {
        &self.client
    }

    /// Persists one event and creates an idempotent case for audit/enforce decisions.
    ///
    /// # Errors
    ///
    /// Returns a typed storage or missing-evidence error.
    pub fn ingest(&self, event: SecurityEvent) -> Result<(), SecurityCoordinatorError> {
        *self
            .last_context
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(event_context(&event));
        ingest_event(&self.store, event)
    }

    /// Starts the reconnecting normalized-event subscription worker.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityCoordinatorError::AlreadyRunning`] for duplicate
    /// workers or a thread-spawn error.
    pub fn start(&self) -> Result<JoinHandle<()>, SecurityCoordinatorError> {
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(SecurityCoordinatorError::AlreadyRunning);
        }
        self.stop.store(false, Ordering::Release);
        let client = self.client.clone();
        let store = Arc::clone(&self.store);
        let stop = Arc::clone(&self.stop);
        let running = Arc::clone(&self.running);
        let last_context = Arc::clone(&self.last_context);
        thread::Builder::new()
            .name("agentsight-security-events".into())
            .spawn(move || {
                subscription_loop(client, store, &stop, &last_context);
                running.store(false, Ordering::Release);
            })
            .map_err(|error| {
                self.running.store(false, Ordering::Release);
                error.into()
            })
    }

    /// Requests a running subscription worker to stop.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn ingest_event(
    store: &SecurityStore,
    event: SecurityEvent,
) -> Result<(), SecurityCoordinatorError> {
    store.insert_event(&event)?;
    let SecurityEventKind::PolicyDecision(decision) = &event.kind else {
        return Ok(());
    };
    if decision.mode == PolicyMode::Observe {
        return Ok(());
    }

    let source = store.event(decision.source_event_id)?.ok_or(
        SecurityCoordinatorError::MissingEvidence {
            kind: "source",
            event_id: decision.source_event_id,
        },
    )?;
    let sink =
        store
            .event(decision.sink_event_id)?
            .ok_or(SecurityCoordinatorError::MissingEvidence {
                kind: "sink",
                event_id: decision.sink_event_id,
            })?;
    let mut transitions = store
        .list_events(&SecurityEventFilter {
            start_ns: Some(source.occurred_at_ns),
            end_ns: Some(event.occurred_at_ns),
            event_type: Some("taint_transition".into()),
            policy_id: Some(decision.policy_id.clone()),
            binding_id: Some(event.identity.binding_id),
            limit: 1_000,
            ..SecurityEventFilter::default()
        })?
        .items;
    transitions.sort_by_key(|item| (item.occurred_at_ns, item.event_id));

    let destination_class = match &sink.kind {
        SecurityEventKind::NetworkAction(network) => network.destination_class,
        _ => DestinationClass::Unknown,
    };
    let severity = if decision.blocked {
        RiskSeverity::Critical
    } else if destination_class == DestinationClass::Trusted {
        RiskSeverity::Medium
    } else {
        RiskSeverity::High
    };
    let correlation_key = risk_correlation_key(&event, decision, &source, &sink);
    let case = RiskCase {
        case_id: event.event_id,
        correlation_key,
        policy_id: decision.policy_id.clone(),
        policy_revision: decision.policy_revision,
        agent_id: event.identity.agent_id.clone(),
        session_id: event.identity.session_id.clone(),
        severity,
        risk_score: decision.risk_score,
        status: RiskCaseStatus::Open,
        blocked: decision.blocked,
        opened_at_ns: source.occurred_at_ns,
        updated_at_ns: event.occurred_at_ns,
        summary: decision.reason.clone(),
    };
    let mut evidence_ids = Vec::with_capacity(transitions.len().saturating_add(3));
    evidence_ids.push(source.event_id);
    evidence_ids.extend(transitions.into_iter().map(|item| item.event_id));
    evidence_ids.push(sink.event_id);
    evidence_ids.push(event.event_id);
    store.upsert_case(&case, &evidence_ids)?;
    Ok(())
}

fn risk_correlation_key(
    decision_event: &SecurityEvent,
    decision: &agentsight_enforcement_protocol::PolicyDecision,
    source: &SecurityEvent,
    sink: &SecurityEvent,
) -> String {
    const BURST_WINDOW_NS: u64 = 5_000_000_000;

    let source_resource = match &source.kind {
        SecurityEventKind::FileAction(action) => action.path.as_str(),
        _ => "unknown-source",
    };
    let burst = source.occurred_at_ns / BURST_WINDOW_NS;
    format!(
        "burst-v1:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        decision_event.identity.binding_id,
        decision.policy_id,
        decision.policy_revision,
        source.identity.pid,
        source.identity.process_start_time,
        sink.identity.pid,
        sink.identity.process_start_time,
        burst,
        source_resource.len(),
        source_resource
    )
}

fn subscription_loop(
    client: EnforcementClient,
    store: Arc<SecurityStore>,
    stop: &AtomicBool,
    last_context: &Mutex<Option<EventContext>>,
) {
    let mut backoff = Duration::from_millis(100);
    let mut disconnected = false;
    while !stop.load(Ordering::Acquire) {
        match client.subscribe_security_events() {
            Ok(mut subscription) => {
                if disconnected {
                    record_subscription_state(&store, last_context, true, "subscription_restored");
                    disconnected = false;
                }
                loop {
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    match subscription.next_event() {
                        Ok(Some(event)) => {
                            backoff = Duration::from_millis(100);
                            *last_context
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                Some(event_context(&event));
                            if let Err(error) = ingest_event(&store, event) {
                                log::error!("security event ingestion failed: {error}");
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            log::warn!("security event subscription disconnected: {error}");
                            if !disconnected {
                                record_subscription_state(
                                    &store,
                                    last_context,
                                    false,
                                    "subscription_lost",
                                );
                            }
                            disconnected = true;
                            break;
                        }
                    }
                }
            }
            Err(error) => {
                log::warn!("security event subscription failed: {error}");
                if !disconnected {
                    record_subscription_state(&store, last_context, false, "subscription_lost");
                    disconnected = true;
                }
            }
        }
        sleep_until_retry(stop, backoff);
        backoff = backoff.saturating_mul(2).min(Duration::from_secs(5));
    }
}

fn record_subscription_state(
    store: &SecurityStore,
    last_context: &Mutex<Option<EventContext>>,
    ready: bool,
    code: &str,
) {
    let context = last_context
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let Some(context) = context else {
        return;
    };
    let now = unix_epoch_ns();
    let event = SecurityEvent {
        event_id: uuid::Uuid::new_v4(),
        occurred_at_ns: now,
        observed_at_ns: now,
        identity: context.identity,
        kind: SecurityEventKind::EnforcementState(EnforcementStateEvent {
            policy_id: context.policy_id,
            policy_revision: context.policy_revision,
            code: code.into(),
            ready,
            message: if ready {
                "normalized security event subscription restored".into()
            } else {
                "normalized security event subscription lost; reconnecting".into()
            },
            dropped_events: None,
        }),
    };
    if let Err(error) = store.insert_event(&event) {
        log::error!("failed to persist security subscription state: {error}");
    }
}

fn event_context(event: &SecurityEvent) -> EventContext {
    let (policy_id, policy_revision) = match &event.kind {
        SecurityEventKind::FileAction(value) => {
            (Some(value.policy_id.clone()), Some(value.policy_revision))
        }
        SecurityEventKind::TaintTransition(value) => {
            (Some(value.policy_id.clone()), Some(value.policy_revision))
        }
        SecurityEventKind::NetworkAction(value) => {
            (Some(value.policy_id.clone()), Some(value.policy_revision))
        }
        SecurityEventKind::PolicyDecision(value) => {
            (Some(value.policy_id.clone()), Some(value.policy_revision))
        }
        SecurityEventKind::EnforcementState(value) => {
            (value.policy_id.clone(), value.policy_revision)
        }
    };
    EventContext {
        identity: event.identity.clone(),
        policy_id,
        policy_revision,
    }
}

fn sleep_until_retry(stop: &AtomicBool, duration: Duration) {
    let mut remaining = duration;
    while !stop.load(Ordering::Acquire) && !remaining.is_zero() {
        let interval = remaining.min(Duration::from_millis(50));
        thread::sleep(interval);
        remaining = remaining.saturating_sub(interval);
    }
}

fn unix_epoch_ns() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    nanos.min(u128::from(u64::MAX)) as u64
}
