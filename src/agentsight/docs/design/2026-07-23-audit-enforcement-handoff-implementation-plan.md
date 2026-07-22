# Audit-to-Enforcement Handoff Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make one Dashboard containment action deterministically replace an audit binding with an
ActPlane enforcement binding, survive restarts, and restore the exact audit binding at expiry.

**Architecture:** AgentSight claims a containment action in `security.db`, then uses its action ID to
create a durable forward or reverse policy transition in `enforcement.db`. The privileged enforcer
serializes replace under the ActPlane lifecycle lock, restores the source when target installation
fails, and exposes an explicit indeterminate outcome when restoration cannot be proved.

**Tech Stack:** Rust 2024, serde, bounded NDJSON over UDS, rusqlite, ActPlane pinned revision
`a62e5d9d96f91101cda019519053e950d532380a`, BPF LSM, systemd, and Hermes.

## Global Constraints

- Linux-only implementation; do not attempt the ActPlane build on macOS.
- Preserve kernel `>= 5.8` compatibility and validate the final result on the Linux 6.6 host.
- Keep all ActPlane imports inside `crates/agentsight-enforcer/src/actplane.rs`.
- Only an ActPlane acknowledgement may produce `enforced`; only `blocked=true` proves denial.
- Never expose policy DSL, sensitive content, credentials, Dashboard tokens, raw paths, or raw
  backend errors in persisted operator messages or reports.
- `security.db` and `enforcement.db` are separate; use stable IDs and idempotent recovery, not a
  claimed cross-database transaction.
- Existing standalone apply/detach APIs remain compatible.
- Production Rust modules target fewer than 500 lines; split tests into existing `*_tests.rs` files.
- Each task must pass `cargo fmt --all -- --check`, focused Clippy, and its focused tests before
  commit.

---

## File Structure

- `crates/enforcement-protocol/src/lib.rs`: wire-level replacement request and outcome types.
- `crates/agentsight-enforcer/src/backend.rs`: backend replace contract.
- `crates/agentsight-enforcer/src/actplane.rs`: prevalidated, lifecycle-locked ActPlane swap.
- `crates/agentsight-enforcer/src/mock.rs`: deterministic replacement test backend.
- `crates/agentsight-enforcer/src/service.rs`: UDS replace dispatch and safe error mapping.
- `src/enforcement/client.rs`: typed replace client.
- `src/enforcement/transition.rs`: transition model, direction, phase, and stable key.
- `src/enforcement/store/transition.rs`: SQLite transition persistence and atomic binding updates.
- `src/enforcement/coordinator/transition.rs`: begin/resume transition orchestration.
- `src/enforcement/coordinator/reconciliation.rs`: transition-first reconnect recovery.
- `src/security/containment/enforcer.rs`: containment-facing transition boundary.
- `src/security/containment.rs`: forward transition creation and activation.
- `src/security/containment/reconciler.rs`: forward and reverse transition recovery.
- `src/security/store/containment.rs`: durable source audit binding identity.
- `tests/containment_pipeline.rs`: cross-store lifecycle and crash-window coverage.

### Task 1: Define the replacement protocol types

**Files:**
- Modify: `src/agentsight/crates/enforcement-protocol/src/lib.rs`

**Interfaces:**
- Produces: `ReplacementPolicy`, `ReplacePolicy`, `ReplaceFailureCode`, and `ReplaceOutcome`.
- Does not add a `Command` variant yet, so the full workspace remains compilable.

- [ ] **Step 1: Write failing serialization and redaction tests**

Add tests that round-trip both replacement variants and every outcome without free-form failure
text:

```rust
#[test]
fn replacement_outcomes_have_only_stable_failure_codes() {
    let outcome = ReplaceOutcome::SourceRestored {
        binding: protocol_fixture_binding(BindingState::Enforced),
        code: ReplaceFailureCode::KernelFailure,
    };
    let json = serde_json::to_string(&outcome).expect("serialize replacement outcome");
    assert!(!json.contains("/root/"));
    assert_eq!(serde_json::from_str::<ReplaceOutcome>(&json), Ok(outcome));
}

#[test]
fn replacement_policy_round_trips_generic_and_credential_targets() {
    for replacement in [
        ReplacementPolicy::Generic(protocol_fixture_apply(Uuid::new_v4())),
        ReplacementPolicy::Credential(protocol_fixture_credential(Uuid::new_v4())),
    ] {
        let request = ReplacePolicy {
            expected: fixture_binding(BindingState::Enforced),
            replacement,
        };
        let json = serde_json::to_vec(&request).expect("serialize replace request");
        assert_eq!(serde_json::from_slice::<ReplacePolicy>(&json), Ok(request));
    }
}
```

Define the three helpers in the protocol test module:

```rust
fn protocol_fixture_apply(binding_id: Uuid) -> ApplyPolicy {
    ApplyPolicy {
        binding_id,
        agent_id: "agent-1".into(),
        session_id: Some("session-1".into()),
        root_pid: 42,
        process_start_time: 101,
        policy_id: "credential-exfiltration".into(),
        policy_revision: "1".into(),
        policy_dsl: "label AGENT".into(),
    }
}

fn protocol_fixture_credential(binding_id: Uuid) -> ApplyCredentialPolicy {
    ApplyCredentialPolicy {
        binding_id,
        agent_id: "agent-1".into(),
        session_id: Some("session-1".into()),
        root_pid: 42,
        process_start_time: 101,
        policy: CredentialExfiltrationPolicy {
            policy_id: "credential-exfiltration".into(),
            revision: 1,
            source_patterns: vec!["/tmp/credential".into()],
            trusted_endpoints: Vec::new(),
            taint_label: "CREDENTIAL".into(),
            taint_ttl_secs: 900,
            mode: PolicyMode::Audit,
        },
    }
}

fn protocol_fixture_binding(state: BindingState) -> Binding {
    Binding {
        request: protocol_fixture_apply(Uuid::new_v4()),
        state,
        message: None,
        domain_id: None,
    }
}
```

- [ ] **Step 2: Run the protocol tests and confirm failure**

Run:

```bash
cd src/agentsight
cargo test -p agentsight-enforcement-protocol replacement_
```

Expected: compilation fails because the four replacement types do not exist.

- [ ] **Step 3: Add the minimal typed contract**

Add public, rustdoc-documented serde types:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "request", rename_all = "snake_case")]
pub enum ReplacementPolicy {
    Generic(ApplyPolicy),
    Credential(ApplyCredentialPolicy),
}

impl ReplacementPolicy {
    pub fn binding_id(&self) -> Uuid {
        match self {
            Self::Generic(request) => request.binding_id,
            Self::Credential(request) => request.binding_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplacePolicy {
    pub expected: Binding,
    pub replacement: ReplacementPolicy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplaceFailureCode {
    BindingConflict,
    StaleProcess,
    CompileFailure,
    KernelFailure,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", content = "data", rename_all = "snake_case")]
pub enum ReplaceOutcome {
    Applied(Binding),
    SourceRetained { binding: Binding, code: ReplaceFailureCode },
    SourceRestored { binding: Binding, code: ReplaceFailureCode },
    Conflict { code: ReplaceFailureCode },
    Indeterminate { code: ReplaceFailureCode },
}
```

Reject equal source and target IDs in `ReplacePolicy::validate()`. Require the expected snapshot to
be `Enforced` and validate credential targets with their existing validator.

- [ ] **Step 4: Verify protocol quality**

Run:

```bash
cargo fmt --all -- --check
cargo clippy -p agentsight-enforcement-protocol --all-targets -- -D warnings
cargo test -p agentsight-enforcement-protocol
```

Expected: all commands exit 0.

- [ ] **Step 5: Commit**

```bash
git add src/agentsight/crates/enforcement-protocol/src/lib.rs
git commit -m "feat(sight): define policy replacement"
```

### Task 2: Implement the serialized enforcer replacement

**Files:**
- Modify: `src/agentsight/crates/enforcement-protocol/src/lib.rs`
- Modify: `src/agentsight/crates/agentsight-enforcer/src/backend.rs`
- Modify: `src/agentsight/crates/agentsight-enforcer/src/mock.rs`
- Modify: `src/agentsight/crates/agentsight-enforcer/src/actplane.rs`
- Modify: `src/agentsight/crates/agentsight-enforcer/src/service.rs`
- Modify: `src/agentsight/crates/agentsight-enforcer/tests/service.rs`
- Modify: `src/agentsight/src/enforcement/client.rs`

**Interfaces:**
- Consumes: Task 1 replacement types.
- Produces: protocol v2 `Command::ReplacePolicy`, `ResponseBody::Replaced`,
  `EnforcementBackend::replace`, and `EnforcementClient::replace`.

- [ ] **Step 1: Write failing service and backend behavior tests**

Cover source, target, empty, and third-party actual states; prevalidation side-effect freedom; target
failure with source restoration; and target plus restoration failure:

```rust
#[test]
fn replace_restores_source_when_target_install_fails() {
    let source = prepared_fixture("source");
    let target = prepared_fixture("target");
    let mut installs = vec![Err(BackendError::KernelFailure("target".into())), Ok(source.binding())]
        .into_iter();
    let outcome = replace_prepared_runtime(
        Some(source.binding()),
        &source,
        &target,
        || Ok(()),
        |_| installs.next().expect("scripted install result"),
    ).expect("replace outcome");
    assert!(matches!(outcome, ReplaceOutcome::SourceRestored { .. }));
}

#[test]
fn replace_never_detaches_a_third_party_binding() {
    let backend = MockBackend::new();
    let third_party = backend.apply(fixture_apply_policy()).expect("third-party apply");
    let expected = fixture_binding(fixture_apply_policy(), BindingState::Enforced);
    let outcome = backend
        .replace(fixture_replace_policy(expected))
        .expect("replace outcome");
    assert_eq!(outcome, ReplaceOutcome::Conflict {
        code: ReplaceFailureCode::BindingConflict,
    });
    assert_eq!(backend.bindings().unwrap(), vec![third_party]);
}
```

Define `fixture_binding(ApplyPolicy, BindingState) -> Binding` and
`fixture_replace_policy(Binding) -> ReplacePolicy` beside the existing fixture helpers. The first
test belongs beside the production ActPlane replacement helper. Define this internal operation
boundary and use it from the real adapter:

```rust
fn replace_prepared_runtime(
    actual: Option<Binding>,
    source: &PreparedBinding,
    target: &PreparedBinding,
    detach_source: impl FnOnce() -> Result<(), BackendError>,
    install: impl FnMut(&PreparedBinding) -> Result<Binding, BackendError>,
) -> Result<ReplaceOutcome, BackendError>;
```

The helper is production code used by `ActPlaneBackend::replace`; tests inject closures at the real
kernel-operation boundary. Do not add test-only fields or methods to `MockBackend`. Its replace
implementation covers deterministic source, target, empty, and third-party state, while failure and
restoration behavior is tested through `replace_prepared_runtime`. Add a private
`prepared_fixture(name: &str) -> PreparedBinding` in the ActPlane unit-test module that compiles a
valid policy for the current live test process and uses the supplied name only to create distinct
binding and policy IDs.

Add a service test that sends `Command::ReplacePolicy`, requires a correlated
`ResponseBody::Replaced`, and asserts protocol v1 is rejected after the version becomes 2.

- [ ] **Step 2: Run focused tests and confirm failure**

```bash
cargo test -p agentsight-enforcer replace_
cargo test -p agentsight-enforcer --test service replace_
```

Expected: compilation fails because replace is not implemented.

- [ ] **Step 3: Add the wire command and backend contract**

Bump `PROTOCOL_VERSION` to `2`, then add:

```rust
pub enum Command {
    // existing variants
    ReplacePolicy(ReplacePolicy),
}

pub enum ResponseBody {
    // existing variants
    Replaced(ReplaceOutcome),
}
```

Extend `EnforcementBackend` with:

```rust
fn replace(&self, request: ReplacePolicy) -> Result<ReplaceOutcome, BackendError>;
```

Dispatch the command in `service.rs`, and add `EnforcementClient::replace` that accepts only
`ResponseBody::Replaced`.

- [ ] **Step 4: Refactor ActPlane apply into prevalidation and locked install helpers**

Introduce private prepared state without exposing ActPlane types outside the adapter:

```rust
struct PreparedBinding {
    request: ApplyPolicy,
    compiled: actplane_ifc_compiler::Compiled,
}

fn prepare_replacement(&self, replacement: ReplacementPolicy)
    -> Result<PreparedBinding, BackendError>;

fn install_prepared_locked(
    &self,
    bindings: &mut HashMap<u32, ActiveBinding>,
    prepared: PreparedBinding,
) -> Result<Binding, BackendError>;
```

Both source restoration and target installation must be compiled and process-validated before
source detachment. `replace` holds `self.lifecycle()` once, recognizes target/source/empty/third-party
actual state, and maps failures to the typed outcomes. Do not call public `apply` or `detach` while
holding the non-reentrant mutex.

If target installation and source restoration both fail, store one fixed, bounded runtime error in
`RuntimeState` so `health()` returns `ready=false`; never store either raw kernel error. Clear that
degraded marker only after a later replacement or runtime preparation establishes one exact active
binding.

- [ ] **Step 5: Implement the same deterministic matrix in MockBackend**

Implement the same successful-state matrix in `MockBackend`. Add a real lifecycle mutex and a
barrier-based unit test that starts replace and apply concurrently and proves the lock prevents
interleaving; keep barriers in the test harness, not as production methods.

- [ ] **Step 6: Verify the enforcer boundary**

```bash
cargo fmt --all -- --check
cargo clippy -p agentsight-enforcer --all-targets --features actplane -- -D warnings
cargo test -p agentsight-enforcement-protocol
cargo test -p agentsight-enforcer
```

Expected: all commands exit 0 on Linux; protocol and mock tests also pass on non-Linux hosts.

- [ ] **Step 7: Commit**

```bash
git add src/agentsight/crates/enforcement-protocol \
  src/agentsight/crates/agentsight-enforcer \
  src/agentsight/src/enforcement/client.rs
git commit -m "feat(sight): replace active policy"
```

### Task 3: Persist and reconcile replacement transitions

**Files:**
- Create: `src/agentsight/src/enforcement/transition.rs`
- Create: `src/agentsight/src/enforcement/store/transition.rs`
- Create: `src/agentsight/src/enforcement/coordinator/transition.rs`
- Modify: `src/agentsight/src/enforcement.rs`
- Modify: `src/agentsight/src/enforcement/store.rs`
- Modify: `src/agentsight/src/enforcement/coordinator.rs`
- Modify: `src/agentsight/src/enforcement/coordinator/reconciliation.rs`
- Modify: `src/agentsight/src/enforcement/coordinator/reconciliation_tests.rs`
- Test: `src/agentsight/tests/enforcement_pipeline.rs`

**Interfaces:**
- Consumes: `EnforcementClient::replace(ReplacePolicy) -> Result<ReplaceOutcome, EnforcementError>`.
- Produces: `TransitionKey`, `TransitionDirection`, `TransitionPhase`, `PolicyTransition`,
  `EnforcementCoordinator::begin_transition`, and `resume_transition`.

- [ ] **Step 1: Write failing store atomicity and reconnect tests**

Use an in-memory store and a scripted desired-state client:

```rust
#[test]
fn completing_forward_transition_updates_both_bindings_atomically() {
    let store = EnforcementStore::open(":memory:").expect("store");
    let transition = fixture_forward_transition();
    let source_id = transition.request.expected.request.binding_id;
    let target_ack = fixture_target_ack(&transition.request.replacement);
    store.upsert_binding(&transition.request.expected).expect("source");
    store.begin_transition(&transition).expect("begin");
    store.complete_transition(&transition.key, &target_ack).expect("complete");
    assert_eq!(store.binding(source_id).unwrap().unwrap().state,
        BindingState::Detached);
    assert_eq!(store.binding(target_ack.request.binding_id).unwrap().unwrap().state,
        BindingState::Enforced);
    assert_eq!(store.transition(&transition.key).unwrap().unwrap().phase,
        TransitionPhase::Completed);
}

#[test]
fn reconnect_resumes_transition_before_uuid_ordered_bindings() {
    let database = TestDatabase::new();
    let store = EnforcementStore::open(&database.path).expect("store");
    let transition = fixture_forward_transition();
    seed_pending_transition(&store, &transition);
    let client = ScriptedClient::with_replace_results(vec![Ok(ReplaceOutcome::Applied(
        fixture_target_ack(&transition.request.replacement),
    ))]);
    reconcile_desired_state(&client, &store).expect("reconcile");
    assert_eq!(client.replaced(), vec![transition.request.replacement.binding_id()]);
}
```

Define `fixture_forward_transition() -> PolicyTransition`,
`fixture_target_ack(&ReplacementPolicy) -> Binding`, and
`seed_pending_transition(&EnforcementStore, &PolicyTransition)` in the corresponding test modules.
Extend the existing `ScriptedClient` with a `replace_results` queue and `replaced: Vec<Uuid>`; its
`replace(ReplacePolicy)` implementation pops exactly one scripted outcome and records the
replacement binding ID.

Also test exact-id idempotency, conflicting reuse, compare-and-swap loss, source-restored failure,
indeterminate retry, and exclusion of participating bindings from ordinary replay.

- [ ] **Step 2: Run focused tests and confirm failure**

```bash
cargo test -p agentsight --lib enforcement::coordinator::
cargo test -p agentsight --test enforcement_pipeline transition
```

Expected: compilation fails because transition APIs do not exist.

- [ ] **Step 3: Define the transition state model**

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransitionDirection { Forward, Reverse }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransitionPhase { Pending, Completed, SourceRestored, Indeterminate }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionKey {
    pub action_id: Uuid,
    pub direction: TransitionDirection,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyTransition {
    pub key: TransitionKey,
    pub request: ReplacePolicy,
    pub phase: TransitionPhase,
    pub acknowledgement: Option<Binding>,
    pub failure_code: Option<ReplaceFailureCode>,
    pub updated_at_ns: u64,
}
```

Use string names in SQLite and exhaustive parsers; reject unknown state rather than defaulting.

- [ ] **Step 4: Add the transition table and atomic methods**

Create `enforcement_transitions` with primary key `(action_id, direction)`, serialized request and
acknowledgement, phase, failure code, and timestamp. Implement transactions for:

```rust
begin_transition(&PolicyTransition) -> Result<PolicyTransition, EnforcementStoreError>
transition(&TransitionKey) -> Result<Option<PolicyTransition>, EnforcementStoreError>
pending_transitions() -> Result<Vec<PolicyTransition>, EnforcementStoreError>
complete_transition(&TransitionKey, &Binding) -> Result<(), EnforcementStoreError>
restore_transition(&TransitionKey, &Binding, ReplaceFailureCode)
    -> Result<(), EnforcementStoreError>
mark_transition_indeterminate(&TransitionKey, ReplaceFailureCode)
    -> Result<(), EnforcementStoreError>
```

`complete_transition` and `restore_transition` use one rusqlite transaction and a phase CAS. Factor
binding upsert SQL into a transaction-aware private helper; keep immutable-request conflict checks.

- [ ] **Step 5: Add coordinator begin/resume and transition-first reconciliation**

`begin_transition` persists before UDS. `resume_transition` loads the exact persisted request and
never accepts caller-supplied replacement details. Both convert `ReplaceOutcome` as follows:

```text
Applied        -> complete_transition
SourceRetained -> restore_transition
SourceRestored -> restore_transition
Conflict       -> mark_transition_indeterminate and return retryable unavailable
Indeterminate  -> mark_transition_indeterminate and return retryable unavailable
transport      -> leave phase unchanged and return unavailable
```

Before ordinary binding reconciliation, resume every Pending or Indeterminate transition. Exclude
both expected and replacement binding IDs from the ordinary actual/desired loops until its phase is
terminal. Overlay enforcement health as not ready while an indeterminate transition exists.

- [ ] **Step 6: Verify persistence and recovery**

```bash
cargo fmt --all -- --check
cargo clippy -p agentsight --lib -- -D warnings
cargo test -p agentsight --lib enforcement::
cargo test -p agentsight --test enforcement_pipeline
```

Expected: all commands exit 0.

- [ ] **Step 7: Commit**

```bash
git add src/agentsight/src/enforcement src/agentsight/src/enforcement.rs \
  src/agentsight/tests/enforcement_pipeline.rs
git commit -m "feat(sight): persist policy transitions"
```

### Task 4: Drive forward containment through the transition

**Files:**
- Modify: `src/agentsight/src/security/containment.rs`
- Modify: `src/agentsight/src/security/containment/enforcer.rs`
- Modify: `src/agentsight/src/security/containment/policy.rs`
- Modify: `src/agentsight/src/security/containment/reconciler.rs`
- Modify: `src/agentsight/src/security/store.rs`
- Modify: `src/agentsight/src/security/store/containment.rs`
- Modify: `src/agentsight/src/security/containment_adapter_tests.rs`
- Modify: `src/agentsight/src/server/containment_tests.rs`
- Test: `src/agentsight/tests/containment_pipeline.rs`

**Interfaces:**
- Consumes: Task 3 `begin_transition` and `resume_transition`.
- Produces: persisted `ContainmentAction::source_binding_id` and transition-backed forward
  activation.

- [ ] **Step 1: Write failing forward lifecycle and crash-window tests**

Add cross-store tests for successful replacement, rejection with source retained, indeterminate
retry, action claim before transition creation, transition completion before action activation, and
source provenance mismatch:

```rust
#[test]
fn containment_replaces_audit_before_becoming_active() {
    let (_process, fixture) = live_fixture();
    let action = fixture.contain(Some(60)).expect("containment");
    assert_eq!(action.source_binding_id, Some(fixture.binding_id));
    assert_eq!(action.lifecycle_state, ContainmentLifecycle::Active);
    assert_eq!(fixture.enforcer.binding_state(fixture.binding_id), BindingState::Detached);
    assert_eq!(fixture.enforcer.binding_state(action.binding_id), BindingState::Enforced);
}

#[test]
fn claimed_action_without_transition_rebuilds_only_from_enforced_audit() {
    let (_process, fixture) = live_fixture();
    fixture.seed_claimed_action_without_transition();
    fixture.coordinator.reconcile_once(now_ns()).expect("recover transition");
    assert_eq!(fixture.enforcer.forward_transition_count(), 1);
    assert_eq!(fixture.latest_action().lifecycle_state, ContainmentLifecycle::Active);
}
```

Extend the existing `Fixture` with `contain(duration_secs)`, `latest_action()`, and
`seed_claimed_action_without_transition()`. Extend the existing `FakeEnforcer` with transition
storage and `binding_state`/`forward_transition_count`; implement the new containment enforcer
boundary by returning scripted stamped replacement outcomes. Reuse the file's existing live helper
process rather than inventing PID/start-time values.

- [ ] **Step 2: Run focused tests and confirm failure**

```bash
cargo test -p agentsight --test containment_pipeline forward_
cargo test -p agentsight --lib security::containment_adapter_tests
```

Expected: compilation or assertions fail because containment still calls direct apply.

- [ ] **Step 3: Migrate containment source identity safely**

Add nullable `source_binding_id` to legacy schema migration, make it required for new claims, and
decode legacy nulls as `Option<Uuid>`. A legacy pending action without exact source identity must be
failed with a safe provenance message; it must not infer trust from an arbitrary detached binding.

- [ ] **Step 4: Replace direct apply with forward begin/resume**

Initial plan and contain continue to call `resolve_policy`, which still requires an `Enforced` audit
binding. Persist its binding ID in the action, then call:

```rust
let key = TransitionKey {
    action_id: action.action_id,
    direction: TransitionDirection::Forward,
};
let outcome = enforcer.begin_transition(key, ReplacePolicy {
    expected: context.binding.clone(),
    replacement: ReplacementPolicy::Credential(apply),
})?;
```

Activate only an exact `Applied` acknowledgement under the existing readiness lease. A retained or
restored source records a terminal attach failure. Conflict, indeterminate, and transport outcomes
remain Pending with bounded retry. Recovery first calls `resume_transition`; only a typed
missing-transition result may rebuild from the still-Enforced source case.

- [ ] **Step 5: Verify forward containment**

```bash
cargo fmt --all -- --check
cargo clippy -p agentsight --lib -- -D warnings
cargo test -p agentsight --lib security::containment
cargo test -p agentsight --test containment_pipeline
```

Expected: all commands exit 0 and direct containment apply is no longer used.

- [ ] **Step 6: Commit**

```bash
git add src/agentsight/src/security src/agentsight/src/server/containment_tests.rs \
  src/agentsight/tests/containment_pipeline.rs
git commit -m "feat(sight): hand off audit containment"
```

### Task 5: Restore audit through a reverse transition at expiry

**Files:**
- Modify: `src/agentsight/src/enforcement/coordinator/transition.rs`
- Modify: `src/agentsight/src/security/containment/enforcer.rs`
- Modify: `src/agentsight/src/security/containment/reconciler.rs`
- Modify: `src/agentsight/src/security/containment_adapter_tests.rs`
- Test: `src/agentsight/tests/containment_pipeline.rs`

**Interfaces:**
- Consumes: completed forward transition and its immutable source request.
- Produces: `begin_reverse_transition(action_id)` and expiry that completes only after audit is
  acknowledged again.

- [ ] **Step 1: Write failing reverse transition tests**

```rust
#[test]
fn expiry_replaces_containment_with_original_audit() {
    let (_process, fixture) = live_fixture();
    let action = fixture.contain(Some(60)).expect("containment");
    fixture.coordinator.reconcile_once(action.expires_at_ns.unwrap()).expect("expire");
    assert_eq!(fixture.latest_action().lifecycle_state, ContainmentLifecycle::Expired);
    assert_eq!(fixture.enforcer.binding_state(action.binding_id), BindingState::Detached);
    assert_eq!(fixture.enforcer.binding_state(fixture.binding_id), BindingState::Enforced);
}

#[test]
fn failed_audit_restore_keeps_action_expiring() {
    let (_process, fixture) = live_fixture();
    let action = fixture.contain(Some(60)).expect("containment");
    fixture.enforcer.make_reverse_indeterminate();
    assert!(fixture.coordinator.reconcile_once(action.expires_at_ns.unwrap()).is_err());
    assert_eq!(fixture.latest_action().lifecycle_state, ContainmentLifecycle::Expiring);
}
```

Add `binding_state(Uuid) -> BindingState` and `make_reverse_indeterminate()` to the existing
`FakeEnforcer`; keep these tests inside the existing Linux module so `live_fixture()` supplies real
PID start-time validation.

Also cover restart after reverse backend swap but before action expiry persistence and verify repeated
reconciliation is idempotent.

- [ ] **Step 2: Run tests and confirm failure**

```bash
cargo test -p agentsight --test containment_pipeline expiry_
cargo test -p agentsight --lib security::containment_adapter_tests reverse_
```

Expected: assertions show expiry detaches containment without restoring audit.

- [ ] **Step 3: Build reverse requests only from the completed forward transition**

Add `begin_reverse_transition(action_id)` to enforcement coordinator. It loads the completed forward
transition, requires its acknowledged containment binding, and creates:

```rust
ReplacePolicy {
    expected: forward.acknowledgement,
    replacement: ReplacementPolicy::Generic(forward.request.expected.request),
}
```

Persist the reverse transition before UDS. Do not accept source policy input from containment, the
case response, or the Dashboard.

- [ ] **Step 4: Gate Expired on reverse acknowledgement**

When Active becomes due, CAS it to Expiring and begin the reverse transition. Expiring recovery
always resumes that transition. Only `Applied(original_audit)` permits `Expired`; retained/restored
containment, conflict, and indeterminate outcomes remain Expiring with bounded backoff.

- [ ] **Step 5: Verify the complete lifecycle**

```bash
cargo fmt --all -- --check
cargo clippy -p agentsight --lib -- -D warnings
cargo test -p agentsight --lib security::containment
cargo test -p agentsight --test containment_pipeline
cargo test --workspace
```

Expected: all commands exit 0.

- [ ] **Step 6: Commit**

```bash
git add src/agentsight/src/enforcement/coordinator/transition.rs \
  src/agentsight/src/security/containment \
  src/agentsight/src/security/containment_adapter_tests.rs \
  src/agentsight/tests/containment_pipeline.rs
git commit -m "feat(sight): restore audit after expiry"
```

### Task 6: Review, deploy, and prove the native Hermes closure

**Files:**
- Modify: `.superpowers/sdd/progress.md` (untracked execution ledger only)
- Modify: `.superpowers/sdd/containment-task-8-report.md` (untracked evidence report only)

**Interfaces:**
- Consumes: reviewed Tasks 1-5 and remote host `47.110.39.158`.
- Produces: exact deployed hashes and redacted proof of audit, enforcement, block, expiry, and audit
  restoration.

- [ ] **Step 1: Run final local/static checks**

```bash
cd src/agentsight
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd dashboard
npm run typecheck
npm test -- --run
npm run build:embed
```

Expected: all new-code checks pass. Any pre-existing full-workspace Clippy baseline must be listed
separately and must not be hidden or attributed to this change.

- [ ] **Step 2: Request independent spec and quality reviews**

Review each task commit against this plan and the accepted design. Critical or important findings
return to a fresh fix subagent and are re-reviewed before deployment.

- [ ] **Step 3: Build and deploy exact reviewed artifacts on Linux**

Archive the reviewed commit without `.superpowers/` or `oslevel-harness/`, build release binaries on
the host, record SHA-256 hashes, back up existing binaries/configuration/databases, install both
AgentSight and enforcer together because protocol v2 is not compatible with v1, restart systemd
units, and require:

```bash
curl -fsS http://127.0.0.1:7396/api/enforcement/health
```

Expected: HTTP 200 with `ready=true` and `backend="actplane"`.

- [ ] **Step 4: Run the redacted Hermes acceptance story**

Use a temporary mock credential file and a controlled untrusted endpoint. Verify, in order:

```text
audit read -> taint -> allowed network decision
POST containment -> source audit detached, target enforce active
Hermes read -> taint -> connect returns EPERM
persisted decision has blocked=true and case blocked_at_ns
wait for configured expiry
reverse transition -> containment detached, original audit enforced
Hermes read -> taint -> network allowed and audit event observed
```

Do not print file content, API key, Dashboard token, raw DSL, or unredacted network payload.

- [ ] **Step 5: Verify restart recovery at both transition directions**

Repeat with a service restart after forward replace acknowledgement and after reverse replace
acknowledgement, before the containment worker can persist its next lifecycle state. Both runs must
converge without duplicate live bindings or UUID-order dependence.

- [ ] **Step 6: Complete the ledger only with evidence**

Record commit, protocol version, binary hashes, kernel version, ActPlane revision, systemd status,
health response, redacted case/action/binding IDs, EPERM, `blocked=true`, expiry, restored audit event,
and rollback locations. Mark Task 8 complete only after every assertion is observed.
