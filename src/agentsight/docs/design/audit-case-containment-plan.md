# Audit Case Containment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade a credential-exfiltration audit case into a time-bounded ActPlane enforce binding, prove containment from a real kernel denial, and detach temporary bindings automatically.

**Architecture:** Add a case-level `ContainmentCoordinator` between the security store and the existing enforcement coordinator. Persist every response action in `security.db`, derive policy input only from the persisted original binding, expose two focused Actix endpoints, and render the lifecycle in a dedicated React dialog. Keep review state separate from binding lifecycle and derive “contained” only from `blocked=true` evidence.

**Tech Stack:** Rust 2024, Actix Web, rusqlite, existing AgentSight enforcement UDS protocol, React, TypeScript, Vitest, Linux `/proc`, ActPlane eBPF.

## Global Constraints

- Follow `src/agentsight/AGENTS.md`; Rust production code contains no `unwrap`, `expect`, or unannotated Clippy suppression.
- Keep each review phase below 800 changed lines and complex production logic below 500 lines. Tasks 1–5 form the backend phase; Tasks 6–7 form the Dashboard phase.
- Do not add a third-party dependency or change the pinned ActPlane revision.
- Do not accept raw ActPlane DSL from the Dashboard.
- Recover the canonical source path only from the persisted original binding; never expand a redacted evidence path.
- Validate process identity as PID plus `/proc/<pid>/stat` start time at both AgentSight and enforcer boundaries.
- Temporary duration defaults to 900 seconds and accepts only `60..=86_400`; `null` means explicitly persistent.
- File contents and raw network payloads never enter containment storage or API responses.
- Any BPF-facing behavior must be verified on a real Linux kernel >= 5.8.
- Preserve the user-owned `.superpowers/` and `oslevel-harness/` directories.

---

## File Structure

### Backend phase

- `src/agentsight/src/enforcement/target.rs`: shared canonical path and process-start-time validation.
- `src/agentsight/src/security/query.rs`: public containment domain and API-facing types.
- `src/agentsight/src/security/store/containment.rs`: containment SQL reads and writes only.
- `src/agentsight/src/security/containment.rs`: case-plan, attach, idempotency, expiry, and reconciliation orchestration.
- `src/agentsight/src/server/containment.rs`: Actix containment HTTP boundary; avoids growing the 2,000-line `handlers.rs`.
- `src/agentsight/tests/containment_pipeline.rs`: cross-store and fake-enforcer lifecycle tests.

### Dashboard phase

- `src/agentsight/dashboard/src/components/ContainmentDialog.tsx`: confirmation, stale-PID replacement, duration, and submission states.
- `src/agentsight/dashboard/src/test/ContainmentDialog.test.tsx`: focused dialog interaction tests.
- `src/agentsight/dashboard/src/pages/SystemAuditPage.tsx`: opens the dialog and renders containment status.
- `src/agentsight/dashboard/src/utils/apiClient.ts`: containment types and API methods.

---

### Task 1: Share Enforcement Target Validation

**Files:**
- Create: `src/agentsight/src/enforcement/target.rs`
- Modify: `src/agentsight/src/enforcement.rs`
- Modify: `src/agentsight/src/server/enforcement.rs`
- Test: `src/agentsight/src/enforcement/target.rs`

**Interfaces:**
- Consumes: Linux `/proc/<pid>/stat` and an absolute `Path`.
- Produces: `pub(crate) fn read_process_start_time(pid: i32) -> Result<u64, TargetValidationError>` and `pub(crate) fn canonical_policy_file(path: &Path) -> Result<PathBuf, TargetValidationError>`.

- [ ] **Step 1: Write failing target-validation tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_init_and_agentsight_processes() {
        assert!(matches!(
            read_process_start_time(1),
            Err(TargetValidationError::ProtectedProcess(_))
        ));
        assert!(matches!(
            read_process_start_time(std::process::id() as i32),
            Err(TargetValidationError::ProtectedProcess(_))
        ));
    }

    #[test]
    fn canonicalizes_an_existing_regular_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(
            canonical_policy_file(file.path()).unwrap(),
            file.path().canonicalize().unwrap()
        );
    }
}
```

- [ ] **Step 2: Run the tests and verify the new module is missing**

Run:

```bash
cd src/agentsight
cargo test enforcement::target::tests -- --nocapture
```

Expected: FAIL because `enforcement::target` and `TargetValidationError` do not exist.

- [ ] **Step 3: Implement the shared validator and replace server-local helpers**

```rust
#[derive(Debug, thiserror::Error)]
pub(crate) enum TargetValidationError {
    #[error("PID {0} is not an eligible Agent process")]
    ProtectedProcess(i32),
    #[error("cannot inspect PID {pid}: {source}")]
    ProcessIo { pid: i32, source: std::io::Error },
    #[error("invalid /proc/{0}/stat start time")]
    InvalidStat(i32),
    #[error("invalid policy file {path}: {message}")]
    InvalidPath { path: PathBuf, message: String },
}

pub(crate) fn read_process_start_time(pid: i32) -> Result<u64, TargetValidationError> {
    if pid <= 1 || pid == std::process::id() as i32 {
        return Err(TargetValidationError::ProtectedProcess(pid));
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|source| TargetValidationError::ProcessIo { pid, source })?;
    let close = stat.rfind(')').ok_or(TargetValidationError::InvalidStat(pid))?;
    stat[close + 1..]
        .split_whitespace()
        .nth(19)
        .ok_or(TargetValidationError::InvalidStat(pid))?
        .parse()
        .map_err(|_| TargetValidationError::InvalidStat(pid))
}
```

Move the existing path character, canonicalization, regular-file, and protected-process-name checks unchanged from `server/enforcement.rs`. Re-export the two functions from `enforcement.rs` as `pub(crate)` and make direct binding handlers use them.

- [ ] **Step 4: Run focused tests and formatting**

```bash
cd src/agentsight
cargo fmt --all -- --check
cargo test enforcement::target::tests server::enforcement::tests -- --nocapture
```

Expected: all selected tests pass and direct binding validation behavior is unchanged.

- [ ] **Step 5: Commit the refactor**

```bash
git add src/agentsight/src/enforcement/target.rs \
  src/agentsight/src/enforcement.rs \
  src/agentsight/src/server/enforcement.rs
git commit -m 'refactor(sight): share target validation'
```

---

### Task 2: Persist Containment Actions

**Files:**
- Modify: `src/agentsight/src/security/query.rs`
- Modify: `src/agentsight/src/security/store.rs`
- Create: `src/agentsight/src/security/store/containment.rs`
- Modify: `src/agentsight/src/security.rs`
- Test: `src/agentsight/tests/security_store.rs`

**Interfaces:**
- Consumes: `Uuid`, case identifiers, binding identifiers, process identity, and nanosecond timestamps.
- Produces: `ContainmentAction`, `ContainmentLifecycle`, `ContainmentFailureStage`, and typed `SecurityStore` CRUD methods.

- [ ] **Step 1: Write failing persistence tests**

```rust
#[test]
fn containment_action_round_trips_and_is_found_by_binding() {
    let store = SecurityStore::open_in_memory().unwrap();
    let action = containment_action(ContainmentLifecycle::Pending);
    store.insert_containment_action(&action).unwrap();

    assert_eq!(store.containment_action(action.action_id).unwrap(), Some(action.clone()));
    assert_eq!(
        store.latest_containment_action(action.case_id).unwrap(),
        Some(action.clone())
    );
    store.mark_containment_blocked(action.binding_id, 500).unwrap();
    assert_eq!(
        store.containment_action(action.action_id).unwrap().unwrap().blocked_at_ns,
        Some(500)
    );
}

#[test]
fn due_actions_exclude_persistent_and_expired_rows() {
    let store = SecurityStore::open_in_memory().unwrap();
    store.insert_containment_action(&containment_action_with_expiry(Some(100))).unwrap();
    store.insert_containment_action(&containment_action_with_expiry(None)).unwrap();
    assert_eq!(store.due_containment_actions(100, 10).unwrap().len(), 1);
}
```

- [ ] **Step 2: Run the store tests and verify the types are missing**

```bash
cd src/agentsight
cargo test --test security_store containment -- --nocapture
```

Expected: FAIL because containment types and store methods are undefined.

- [ ] **Step 3: Add domain types and the additive SQLite schema**

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainmentLifecycle { Pending, Active, Expiring, Expired, Failed }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainmentFailureStage { Attach, Detach, Reconcile }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainmentAction {
    pub action_id: Uuid,
    pub case_id: Uuid,
    pub binding_id: Uuid,
    pub agent_id: String,
    pub root_pid: i32,
    pub process_start_time: u64,
    pub source_path: String,
    pub duration_secs: Option<u64>,
    pub expires_at_ns: Option<u64>,
    pub lifecycle_state: ContainmentLifecycle,
    pub blocked_at_ns: Option<u64>,
    pub requested_by: String,
    pub failure_stage: Option<ContainmentFailureStage>,
    pub failure_reason: Option<String>,
    pub attempt_count: u32,
    pub next_retry_at_ns: Option<u64>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}
```

Add `mod containment;` to `security/store.rs` and create this complete table in its existing schema batch:

```sql
CREATE TABLE IF NOT EXISTS containment_actions (
    action_id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL,
    binding_id TEXT NOT NULL UNIQUE,
    agent_id TEXT NOT NULL,
    root_pid INTEGER NOT NULL,
    process_start_time INTEGER NOT NULL,
    source_path TEXT NOT NULL,
    duration_secs INTEGER,
    expires_at_ns INTEGER,
    lifecycle_state TEXT NOT NULL,
    blocked_at_ns INTEGER,
    requested_by TEXT NOT NULL,
    failure_stage TEXT,
    failure_reason TEXT,
    attempt_count INTEGER NOT NULL,
    next_retry_at_ns INTEGER,
    created_at_ns INTEGER NOT NULL,
    updated_at_ns INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_containment_case_time
    ON containment_actions(case_id, created_at_ns DESC);
CREATE INDEX IF NOT EXISTS idx_containment_due
    ON containment_actions(lifecycle_state, expires_at_ns, next_retry_at_ns);
```

Implement `insert_containment_action`, `containment_action`, `latest_containment_action`, `due_containment_actions`, `update_containment_action`, and `mark_containment_blocked` in the child module. Use the parent store mutex and checked `u64` to SQLite `i64` conversion.

- [ ] **Step 4: Run persistence tests**

```bash
cd src/agentsight
cargo fmt --all -- --check
cargo test --test security_store containment -- --nocapture
```

Expected: round-trip, due-action, enum parsing, and idempotent block timestamp tests pass.

- [ ] **Step 5: Commit the persistence layer**

```bash
git add src/agentsight/src/security.rs \
  src/agentsight/src/security/query.rs \
  src/agentsight/src/security/store.rs \
  src/agentsight/src/security/store/containment.rs \
  src/agentsight/tests/security_store.rs
git commit -m 'feat(sight): persist containment actions'
```

---

### Task 3: Orchestrate Case-to-Binding Containment

**Files:**
- Create: `src/agentsight/src/security/containment.rs`
- Modify: `src/agentsight/src/security.rs`
- Test: `src/agentsight/tests/containment_pipeline.rs`

**Interfaces:**
- Consumes: `Arc<SecurityStore>`, `Arc<dyn ContainmentEnforcer>`, `ContainmentRequest`, case evidence, and persisted enforcement bindings.
- Produces: `ContainmentCoordinator::plan`, `ContainmentCoordinator::contain`, `ContainmentPlan`, `ContainmentCandidate`, and typed `ContainmentError`.

- [ ] **Step 1: Write failing orchestration tests with a fake enforcer**

```rust
#[test]
fn contain_uses_original_binding_path_and_confirms_after_ack() {
    let fixture = Fixture::audit_case_with_binding("/root/secret.txt");
    let action = fixture.coordinator.contain(
        fixture.case_id,
        ContainmentRequest { root_pid: fixture.live_pid, duration_secs: Some(900) },
        "dashboard-token",
    ).unwrap();

    assert_eq!(action.source_path, "/root/secret.txt");
    assert_eq!(action.lifecycle_state, ContainmentLifecycle::Active);
    assert_eq!(fixture.store.case_detail(fixture.case_id).unwrap().case.status, RiskCaseStatus::Confirmed);
    assert_eq!(fixture.enforcer.apply_calls(), 1);
}

#[test]
fn missing_original_binding_never_uses_redacted_evidence_path() {
    let fixture = Fixture::audit_case_without_binding("~/secret.txt");
    assert!(matches!(
        fixture.coordinator.plan(fixture.case_id, Vec::new()),
        Err(ContainmentError::SourcePolicyUnavailable(_))
    ));
}

#[test]
fn repeated_active_request_returns_the_existing_action() {
    let fixture = Fixture::audit_case_with_binding("/root/secret.txt");
    let first = fixture.contain_default();
    let second = fixture.contain_default();
    assert_eq!(first.action_id, second.action_id);
    assert_eq!(fixture.enforcer.apply_calls(), 1);
}
```

- [ ] **Step 2: Run the new integration test and verify it fails**

```bash
cd src/agentsight
cargo test --test containment_pipeline -- --nocapture
```

Expected: FAIL because the coordinator and plan types do not exist.

- [ ] **Step 3: Implement the coordinator boundary**

```rust
pub trait ContainmentEnforcer: Send + Sync {
    fn apply_credential_policy(&self, request: ApplyCredentialPolicy) -> Result<Binding, String>;
    fn detach(&self, binding_id: Uuid) -> Result<(), String>;
    fn bindings(&self) -> Result<Vec<Binding>, String>;
}

pub struct ContainmentCoordinator {
    store: Arc<SecurityStore>,
    enforcer: Arc<dyn ContainmentEnforcer>,
    stop: Arc<AtomicBool>,
}

pub struct ContainmentRequest {
    pub root_pid: i32,
    pub duration_secs: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainmentCandidate {
    pub agent_id: String,
    pub root_pid: i32,
    pub process_start_time: u64,
    pub display_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainmentPlan {
    pub case_id: Uuid,
    pub source_path: String,
    pub original_target: Option<ContainmentCandidate>,
    pub original_target_valid: bool,
    pub candidates: Vec<ContainmentCandidate>,
    pub default_duration_secs: u64,
    pub min_duration_secs: u64,
    pub max_duration_secs: u64,
    pub existing_action: Option<ContainmentAction>,
}

#[derive(Debug, thiserror::Error)]
pub enum ContainmentError {
    #[error("risk case {0} does not exist")]
    MissingCase(Uuid),
    #[error("source policy for case {0} is unavailable")]
    SourcePolicyUnavailable(Uuid),
    #[error("root process {0} is stale")]
    RootProcessStale(i32),
    #[error("case {case_id} cannot be contained from state {status:?}")]
    IneligibleCase { case_id: Uuid, status: RiskCaseStatus },
    #[error("duration must be null or between 60 and 86400 seconds")]
    InvalidDuration,
    #[error("enforcement unavailable: {0}")]
    Enforcer(String),
    #[error(transparent)]
    Store(#[from] SecurityStoreError),
}

impl ContainmentCoordinator {
    pub fn plan(
        &self,
        case_id: Uuid,
        candidates: Vec<ContainmentCandidate>,
    ) -> Result<ContainmentPlan, ContainmentError>;

    pub fn contain(
        &self,
        case_id: Uuid,
        request: ContainmentRequest,
        requested_by: &str,
    ) -> Result<ContainmentAction, ContainmentError>;
}
```

Implement the production `ContainmentEnforcer` adapter for `EnforcementCoordinator`. `plan` must load the case, find its evidence binding ID, resolve that binding from `bindings()`, parse the exact `source CREDENTIAL = file "..."` line, and reject missing or malformed bindings. `contain` validates `60..=86_400` or persistent `None`, reads the selected PID start time, persists pending state, applies an `ApplyCredentialPolicy` with `PolicyMode::Enforce`, and changes the case to confirmed only after an enforced acknowledgement.

- [ ] **Step 4: Run coordinator tests**

```bash
cd src/agentsight
cargo fmt --all -- --check
cargo test --test containment_pipeline -- --nocapture
```

Expected: path recovery, stale PID, invalid case state, idempotency, attach failure, and confirm-after-ack tests pass.

- [ ] **Step 5: Commit orchestration**

```bash
git add src/agentsight/src/security.rs \
  src/agentsight/src/security/containment.rs \
  src/agentsight/tests/containment_pipeline.rs
git commit -m 'feat(sight): orchestrate case containment'
```

---

### Task 4: Reconcile Expiry and Kernel Containment Evidence

**Files:**
- Modify: `src/agentsight/src/security/containment.rs`
- Modify: `src/agentsight/src/security/coordinator.rs`
- Modify: `src/agentsight/src/security/store/containment.rs`
- Test: `src/agentsight/tests/containment_pipeline.rs`
- Test: `src/agentsight/tests/security_pipeline.rs`

**Interfaces:**
- Consumes: active actions, `blocked=true` policy decisions, current time, and persisted binding state.
- Produces: `ContainmentCoordinator::reconcile_once(now_ns)`, `start_reconciler(interval)`, `stop`, automatic detachment, and idempotent `blocked_at_ns` updates.

- [ ] **Step 1: Write failing block and expiry tests**

```rust
#[test]
fn blocked_decision_marks_the_matching_action_contained_once() {
    let fixture = Fixture::active_containment(Some(900));
    fixture.security.ingest(fixture.blocked_decision(700)).unwrap();
    fixture.security.ingest(fixture.blocked_decision(800)).unwrap();
    assert_eq!(fixture.action().blocked_at_ns, Some(700));
    let case = fixture.store.case_detail(fixture.case_id).unwrap();
    assert!(case.case.blocked);
    assert_eq!(case.case.severity, RiskSeverity::Critical);
    assert_eq!(fixture.store.list_cases(10, 0).unwrap().len(), 1);
}

#[test]
fn reconciliation_detaches_due_actions_and_survives_reconstruction() {
    let fixture = Fixture::active_containment(Some(60));
    let restarted = fixture.reconstruct_coordinator();
    restarted.reconcile_once(fixture.expires_at_ns).unwrap();
    assert_eq!(restarted.latest_action().lifecycle_state, ContainmentLifecycle::Expired);
    assert_eq!(fixture.enforcer.detach_calls(), 1);
}
```

- [ ] **Step 2: Run tests and verify lifecycle behavior is absent**

```bash
cd src/agentsight
cargo test --test containment_pipeline reconcile -- --nocapture
cargo test --test security_pipeline containment -- --nocapture
```

Expected: FAIL because block correlation and reconciliation are not implemented.

- [ ] **Step 3: Implement correlation and bounded reconciliation**

After inserting every `PolicyDecision`, detect containment bindings before normal case creation:

```rust
if let Some(case_id) = store.case_id_for_containment_binding(event.identity.binding_id)? {
    store.append_containment_evidence(
        case_id,
        &evidence_ids,
        decision.risk_score,
        decision.blocked,
        event.occurred_at_ns,
    )?;
    store.mark_containment_blocked(event.identity.binding_id, event.occurred_at_ns)?;
    return Ok(());
}
```

`append_containment_evidence` keeps the original `case_id`, appends unique evidence links, upgrades
the case to critical/blocked using the enforce risk score, and prevents a second risk case for the
containment binding. Implement `reconcile_once` so it marks a due active action expiring, calls
`detach`, and marks it expired only after acknowledgement. Retry transient errors at 1, 2, 4, 8,
and 16 seconds; after five failed attempts set `Failed`, `failure_stage=Detach`, and preserve the
sanitized reason. Reconcile pending actions against `bindings()` before applying again, and never
detach a persistent action.

- [ ] **Step 4: Run lifecycle tests**

```bash
cd src/agentsight
cargo fmt --all -- --check
cargo test --test containment_pipeline -- --nocapture
cargo test --test security_pipeline -- --nocapture
```

Expected: the first block time is stable, temporary actions expire, persistent actions remain active, and reconstructed coordinators finish pending work.

- [ ] **Step 5: Commit lifecycle handling**

```bash
git add src/agentsight/src/security/containment.rs \
  src/agentsight/src/security/coordinator.rs \
  src/agentsight/src/security/store/containment.rs \
  src/agentsight/tests/containment_pipeline.rs \
  src/agentsight/tests/security_pipeline.rs
git commit -m 'feat(sight): reconcile containment lifecycle'
```

---

### Task 5: Expose the Case Containment API

**Files:**
- Create: `src/agentsight/src/server/containment.rs`
- Modify: `src/agentsight/src/server/handlers.rs`
- Modify: `src/agentsight/src/server/mod.rs`
- Test: `src/agentsight/src/server/containment.rs`

**Interfaces:**
- Consumes: `AppState.containment`, `AppState.health_store`, case IDs, and authenticated Dashboard requests.
- Produces: `GET /api/audit/cases/{case_id}/containment-plan` and `POST /api/audit/cases/{case_id}/contain`.

- [ ] **Step 1: Write failing Actix response tests**

```rust
#[actix_web::test]
async fn stale_plan_returns_candidates_and_contain_is_idempotent() {
    let fixture = ApiFixture::stale_original_with_live_candidate(4242);
    let app = awtest::init_service(
        App::new().app_data(fixture.data()).configure(configure_routes)
    ).await;

    let plan = awtest::call_and_read_body_json::<serde_json::Value, _>(
        &app,
        awtest::TestRequest::get()
            .uri(&format!("/api/audit/cases/{}/containment-plan", fixture.case_id))
            .to_request(),
    ).await;
    assert_eq!(plan["data"]["original_target_valid"], false);
    assert_eq!(plan["data"]["candidates"][0]["root_pid"], 4242);
}
```

- [ ] **Step 2: Run the server test and verify the routes return 404**

```bash
cd src/agentsight
cargo test server::containment::tests -- --nocapture
```

Expected: FAIL because the module and routes do not exist.

- [ ] **Step 3: Implement focused handlers and lifecycle startup**

```rust
#[derive(Debug, Deserialize)]
pub(super) struct ContainCaseRequest {
    root_pid: i32,
    duration_secs: Option<u64>,
}

#[get("/audit/cases/{case_id}/containment-plan")]
pub(super) async fn containment_plan(
    data: web::Data<AppState>,
    case_id: web::Path<Uuid>,
) -> HttpResponse;

#[post("/audit/cases/{case_id}/contain")]
pub(super) async fn contain_case(
    data: web::Data<AppState>,
    case_id: web::Path<Uuid>,
    body: web::Json<ContainCaseRequest>,
) -> HttpResponse;
```

Add `containment: Arc<ContainmentCoordinator>` to `AppState`. Build candidates from non-offline `HealthStore::all_agents()`, register both routes, start a five-second reconciler after the security ingestion handshake, and stop/join it during every server shutdown path. Make `handlers::local_security_response` and `handlers::security_store_error` `pub(super)` for the sibling module instead of duplicating response envelopes.

Map typed errors exactly as follows:

```rust
match error {
    ContainmentError::MissingCase(_) => containment_error(
        StatusCode::NOT_FOUND, "case_not_found", false, error,
    ),
    ContainmentError::SourcePolicyUnavailable(_) => containment_error(
        StatusCode::CONFLICT, "source_policy_unavailable", false, error,
    ),
    ContainmentError::RootProcessStale(_) => containment_error(
        StatusCode::CONFLICT, "root_process_stale", true, error,
    ),
    ContainmentError::IneligibleCase { .. } => containment_error(
        StatusCode::CONFLICT, "case_not_eligible", false, error,
    ),
    ContainmentError::InvalidDuration => containment_error(
        StatusCode::BAD_REQUEST, "invalid_duration", false, error,
    ),
    ContainmentError::Enforcer(_) => containment_error(
        StatusCode::SERVICE_UNAVAILABLE, "enforcer_unavailable", true, error,
    ),
    ContainmentError::Store(_) => containment_error(
        StatusCode::INTERNAL_SERVER_ERROR, "security_store_unavailable", true, error,
    ),
}
```

The handler always supplies `requested_by="dashboard-token"` until authentication exposes a
named principal.

- [ ] **Step 4: Run backend phase verification**

```bash
cd src/agentsight
cargo fmt --all -- --check
cargo test --test security_store --test containment_pipeline --test security_pipeline
cargo test server::containment::tests
cargo clippy -p agentsight --all-targets -- -D warnings
```

Expected: all containment tests pass. If strict Clippy still reports the documented unrelated Rust 1.97 baseline, verify no diagnostic points to files changed by Tasks 1–5 and record the baseline separately; do not add `#[allow]` to new code.

- [ ] **Step 5: Commit and review the backend phase**

```bash
git add src/agentsight/src/server/containment.rs \
  src/agentsight/src/server/handlers.rs \
  src/agentsight/src/server/mod.rs
git commit -m 'feat(sight): add case containment API'
git diff --stat HEAD~5..HEAD
```

Expected: the backend phase is independently testable and remains below the 800-line review limit. Split test fixtures into a follow-up backend commit before proceeding if the diff exceeds the limit.

---

### Task 6: Build the Containment Confirmation Dialog

**Files:**
- Modify: `src/agentsight/dashboard/src/utils/apiClient.ts`
- Create: `src/agentsight/dashboard/src/components/ContainmentDialog.tsx`
- Create: `src/agentsight/dashboard/src/test/ContainmentDialog.test.tsx`

**Interfaces:**
- Consumes: `SecurityContainmentPlan`, `SecurityContainmentAction`, `fetchContainmentPlan(caseId)`, and `containSecurityCase(caseId, request)`.
- Produces: `ContainmentDialog` with `caseId`, `open`, `onClose`, and `onContained` props.

- [ ] **Step 1: Write failing dialog tests**

```tsx
it('defaults to a 15 minute temporary block and submits the valid PID', async () => {
  render(<ContainmentDialog caseId="case-1" open onClose={vi.fn()} onContained={onContained} />);
  expect(await screen.findByText('确认升级为内核拦截')).toBeInTheDocument();
  expect(screen.getByLabelText('临时拦截 15 分钟')).toBeChecked();
  fireEvent.click(screen.getByRole('button', { name: '确认并下发' }));
  await waitFor(() => expect(containSecurityCase).toHaveBeenCalledWith('case-1', {
    root_pid: 1832,
    duration_secs: 900,
  }));
});

it('requires a live Agent selection when the original PID is stale', async () => {
  fetchContainmentPlan.mockResolvedValue(stalePlanWithCandidate(4242));
  render(<ContainmentDialog caseId="case-1" open onClose={vi.fn()} onContained={vi.fn()} />);
  expect(await screen.findByLabelText('选择在线 Agent')).toBeRequired();
  expect(screen.getByRole('button', { name: '确认并下发' })).toBeDisabled();
});
```

- [ ] **Step 2: Run the dialog test and verify imports fail**

```bash
cd src/agentsight/dashboard
npx vitest run src/test/ContainmentDialog.test.tsx
```

Expected: FAIL because the component and API functions do not exist.

- [ ] **Step 3: Add exact client types and implement the dialog**

```ts
export interface SecurityContainmentPlan {
  case_id: string;
  source_path: string;
  original_target?: SecurityContainmentCandidate | null;
  original_target_valid: boolean;
  candidates: SecurityContainmentCandidate[];
  default_duration_secs: 900;
  min_duration_secs: 60;
  max_duration_secs: 86400;
  existing_action?: SecurityContainmentAction | null;
}

export type SecurityContainmentLifecycle =
  | 'pending' | 'active' | 'expiring' | 'expired' | 'failed';

export interface SecurityContainmentCandidate {
  agent_id: string;
  root_pid: number;
  process_start_time: number;
  display_name: string;
}

export interface SecurityContainmentAction {
  action_id: string;
  case_id: string;
  binding_id: string;
  agent_id: string;
  root_pid: number;
  process_start_time: number;
  source_path: string;
  duration_secs?: number | null;
  expires_at_ns?: number | null;
  lifecycle_state: SecurityContainmentLifecycle;
  blocked_at_ns?: number | null;
  requested_by: string;
  failure_stage?: 'attach' | 'detach' | 'reconcile' | null;
  failure_reason?: string | null;
  created_at_ns: number;
  updated_at_ns: number;
}

export interface SecurityContainmentRequest {
  root_pid: number;
  duration_secs: number | null;
}
```

Add `fetchContainmentPlan(caseId)` and `containSecurityCase(caseId, request)` beside the existing
case APIs. Map `source_policy_unavailable` and `root_process_stale` to actionable dialog messages.
Implement the approved modal: case-derived path and scope are read-only, temporary 900 seconds is
selected by default, persistent mode requires an explicit radio selection, stale targets require
a live candidate, and submission is disabled while loading or pending. Display typed server
errors without exposing raw policy DSL.

- [ ] **Step 4: Run component tests and type checking**

```bash
cd src/agentsight/dashboard
npx vitest run src/test/ContainmentDialog.test.tsx
npm run typecheck
```

Expected: dialog tests and TypeScript validation pass.

- [ ] **Step 5: Commit the dialog**

```bash
git add src/agentsight/dashboard/src/utils/apiClient.ts \
  src/agentsight/dashboard/src/components/ContainmentDialog.tsx \
  src/agentsight/dashboard/src/test/ContainmentDialog.test.tsx
git commit -m 'feat(sight): add containment confirmation'
```

---

### Task 7: Integrate Containment Into System Audit

**Files:**
- Modify: `src/agentsight/dashboard/src/pages/SystemAuditPage.tsx`
- Modify: `src/agentsight/dashboard/src/test/SystemAuditPage.test.tsx`

**Interfaces:**
- Consumes: `ContainmentDialog` and the containment summary returned by case detail.
- Produces: the approved “Upgrade to enforcement” action and awaiting-review, active, contained, expired, and failed UI states.

- [ ] **Step 1: Write failing page integration tests**

```tsx
it('opens containment from an eligible audit case and refreshes the detail', async () => {
  render(<MemoryRouter><SystemAuditPage /></MemoryRouter>);
  fireEvent.click(await screen.findByText('疑似凭据外传'));
  fireEvent.click(await screen.findByRole('button', { name: '升级为拦截' }));
  expect(await screen.findByText('确认升级为内核拦截')).toBeInTheDocument();
});

it('shows contained only after blocked_at_ns exists', async () => {
  fetchSecurityCase.mockResolvedValue(containedCaseDetail({ blocked_at_ns: 900 }));
  render(<MemoryRouter><SystemAuditPage /></MemoryRouter>);
  fireEvent.click(await screen.findByText('疑似凭据外传'));
  expect(await screen.findByText('已遏制')).toBeInTheDocument();
  expect(screen.getByText(/内核确认阻断/)).toBeInTheDocument();
});
```

- [ ] **Step 2: Run the page tests and verify the action is absent**

```bash
cd src/agentsight/dashboard
npx vitest run src/test/SystemAuditPage.test.tsx
```

Expected: FAIL because the containment action and lifecycle presentation are not rendered.

- [ ] **Step 3: Integrate the approved interaction**

Add dialog state to `SystemAuditPage`, render “升级为拦截” only for high/critical audit cases that are neither false positive, accepted risk, nor resolved, and reload both list and detail after success. Render a containment card containing lifecycle, PID, binding, countdown, first block time, failure stage, failure reason, expiry, and the existing manual “标记已处置” action.

Use `blocked_at_ns` as the sole condition for “已遏制”; never infer containment from `lifecycle_state === 'active'`.

- [ ] **Step 4: Run Dashboard phase verification**

```bash
cd src/agentsight/dashboard
npx vitest run src/test/ContainmentDialog.test.tsx src/test/SystemAuditPage.test.tsx
npm run typecheck
npm run build:embed
```

Expected: all selected tests pass, TypeScript reports no errors, and Webpack completes successfully.

- [ ] **Step 5: Commit and review the Dashboard phase**

```bash
git add src/agentsight/dashboard/src/pages/SystemAuditPage.tsx \
  src/agentsight/dashboard/src/test/SystemAuditPage.test.tsx
git commit -m 'feat(sight): expose case containment'
git diff --stat HEAD~2..HEAD
```

Expected: the Dashboard phase remains below the 800-line review limit.

---

### Task 8: Verify the Real Hermes Containment Closure

**Files:**
- No repository files are modified.
- Runtime target: Linux instance with AgentSight, `agentsight-enforcer`, ActPlane, and Hermes.

**Interfaces:**
- Consumes: release binaries and the two Dashboard containment endpoints.
- Produces: reproducible audit, block, containment, expiry, and post-expiry allow evidence.

- [ ] **Step 1: Run the complete local verification set**

```bash
cd src/agentsight
cargo fmt --all -- --check
cargo test --workspace
cargo clippy -p agentsight-enforcement-protocol -p agentsight-enforcer --all-targets -- -D warnings

cd dashboard
npm run typecheck
npx vitest run src/test/ContainmentDialog.test.tsx src/test/SystemAuditPage.test.tsx
npm run build:embed
```

Expected: every command exits zero. Record any unrelated full-package Clippy baseline separately and ensure no diagnostic points to changed files.

- [ ] **Step 2: Build and deploy release binaries on Linux**

```bash
cd /root/agentsight-violation-debug/src/agentsight
cargo build --release --bin agentsight
./scripts/build-enforcer.sh
install -m 0755 target/release/agentsight /usr/local/bin/agentsight
install -m 0755 target/release/agentsight-enforcer /usr/local/bin/agentsight-enforcer
systemctl restart agentsight-enforcer agentsight
systemctl is-active agentsight-enforcer agentsight
```

Expected: both services report `active`, and `/api/enforcement/health` reports `ready=true` with backend `actplane`.

- [ ] **Step 3: Create an audit case with Hermes**

Start a waiting Hermes process, bind the existing credential-exfiltration policy in audit mode, and let Hermes execute the file-read plus external-connect helper. Verify one high-risk unblocked case with file, taint, network, and policy-decision evidence.

- [ ] **Step 4: Upgrade the case with a 60-second temporary binding**

Call the case containment API from the Dashboard, repeat the same Hermes operation under the selected live PID, and verify:

```text
kernel connect result = EPERM
policy decision blocked = true
containment lifecycle = active
blocked_at_ns is present
case display = 已遏制
```

- [ ] **Step 5: Verify automatic detachment and recovery**

Wait until the action reports `expired`, confirm the binding reports detached, then repeat the network operation and verify it is no longer denied by the expired containment binding. Leave the case confirmed/contained until a user explicitly marks it resolved.

- [ ] **Step 6: Confirm repository and service hygiene**

```bash
git status --short
git diff --check
systemctl is-active agentsight-enforcer agentsight
```

Expected: only the user-owned `.superpowers/` and `oslevel-harness/` paths remain untracked, no whitespace errors exist, and both services remain active.
