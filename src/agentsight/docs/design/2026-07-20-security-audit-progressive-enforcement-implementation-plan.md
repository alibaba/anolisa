# AgentSight Security Audit and Progressive Enforcement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build an AgentSight-owned credential-exfiltration audit loop that receives ActPlane security facts, correlates an explainable risk case, and rejects kernel enforcement when the pinned ABI cannot preserve the product policy without requiring AgentSecCore.

**Architecture:** Complete the foundational UDS enforcer in `2026-07-15-actplane-enforcement-implementation-plan.md` first. This plan extends that boundary with stable security events, an AgentSight-local store and case correlator, a dedicated System Audit workspace, and one product policy translated only inside the ActPlane adapter.

**Tech Stack:** Rust 2024, bounded NDJSON over Unix domain sockets, SQLite/rusqlite, Actix Web, React 18, TypeScript, BPF-LSM, and official ActPlane pinned to `a62e5d9d96f91101cda019519053e950d532380a`.

## Global Constraints

- Complete Tasks 1-6 in `2026-07-15-actplane-enforcement-implementation-plan.md` before Task 1 here.
- AgentSight and the ActPlane backend are Linux-only; mock protocol, storage, API, and Dashboard tests must run without BPF privileges.
- Linux support remains kernel >= 5.8 with BTF; BPF changes require a real-kernel test.
- AgentSecCore must not be installed, contacted, or required by this feature.
- Keep AgentSight libbpf-rs observation probes separate from the ActPlane Aya runtime.
- All ActPlane types and imports remain in `crates/agentsight-enforcer/src/actplane.rs`.
- Do not add `mod.rs`, production `unwrap()`, production `expect()`, or a vendored ActPlane copy.
- The product policy schema is stable; exact upstream DSL syntax stays inside the adapter.
- Only an ActPlane acknowledgement may produce `enforced`; only an observed kernel result may produce `blocked=true`.
- New attachment failures are fail-open and emit an actionable enforcement-state event.
- Default evidence excludes file contents, TLS plaintext, credentials, authorization headers, and memory buffers.
- Initial operating budget: <30 MiB added idle RSS, <1% added idle CPU, and <2 ms P99 policy-decision overhead.
- Rust gates are `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`.
- Dashboard gates are `npm run typecheck` and `npm run build:embed` from `src/agentsight/dashboard`; the foundation intentionally has no Vitest suite.

## Dependency and File Map

- Foundation: `crates/enforcement-protocol/`, `crates/agentsight-enforcer/`, and `src/enforcement.rs` from the earlier plan.
- Stable event contract: `crates/enforcement-protocol/src/security.rs`.
- Enforcer behavior: `crates/agentsight-enforcer/src/{backend.rs,mock.rs,actplane.rs}`.
- AgentSight security domain: `src/security.rs` and `src/security/{store.rs,coordinator.rs,query.rs}`.
- Server integration: `src/server/{mod.rs,system_audit.rs}`.
- Dashboard surface: `dashboard/src/pages/SystemAuditPage.tsx` and narrow additions to `dashboard/src/utils/apiClient.ts`, `App.tsx`, and `components/NavBar.tsx`.
- Protected compatibility surface: existing Security Observability code and routes remain unchanged.
- Linux acceptance: `scripts/test-security-audit-integration.sh`.

---

### Task 1: Add the stable security-event and product-policy contract

**Files:**
- Create: `src/agentsight/crates/enforcement-protocol/src/security.rs`
- Modify: `src/agentsight/crates/enforcement-protocol/src/lib.rs`
- Test: `src/agentsight/crates/enforcement-protocol/src/security.rs`

**Interfaces:**
- Consumes: foundational `Request`, `Command`, `Response`, `Binding`, and `Effect`.
- Produces: `SecurityEvent`, `SecurityEventKind`, `EventIdentity`, `FileAction`, `TaintTransition`, `NetworkAction`, `PolicyDecision`, `EnforcementStateEvent`, `CredentialExfiltrationPolicy`, `DestinationScope`, and `PolicyMode`.

- [ ] **Step 1: Write failing serialization and validation tests**

```rust
#[test]
fn policy_decision_round_trip_preserves_requested_and_observed_results() {
    let event = SecurityEvent::policy_decision(
        fixture_identity(),
        PolicyDecision {
            policy_id: "credential-exfiltration".into(),
            policy_revision: 3,
            source_event_id: Uuid::new_v4(),
            sink_event_id: Uuid::new_v4(),
            mode: PolicyMode::Enforce,
            requested_effect: Effect::Block,
            blocked: true,
            killed: false,
            errno: Some(1),
            risk_score: 85,
            reason: "credential taint reached unknown public endpoint".into(),
        },
    );
    let json = serde_json::to_string(&event).expect("fixture should serialize");
    let decoded: SecurityEvent =
        serde_json::from_str(&json).expect("fixture should deserialize");
    assert_eq!(decoded, event);
}

#[test]
fn credential_policy_rejects_zero_ttl() {
    let mut policy = fixture_policy();
    policy.taint_ttl_secs = 0;
    assert_eq!(policy.validate(), Err(PolicyValidationError::ZeroTaintTtl));
}
```

- [ ] **Step 2: Run the focused test and verify failure**

Run:

```bash
cd src/agentsight
cargo test -p agentsight-enforcement-protocol security::tests
```

Expected: FAIL because the security contract and validation error do not exist.

- [ ] **Step 3: Implement the tagged schema**

Use exact discriminators instead of an untagged payload:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityEvent {
    pub event_id: Uuid,
    pub occurred_at_ns: u64,
    pub observed_at_ns: u64,
    pub identity: EventIdentity,
    #[serde(flatten)]
    pub kind: SecurityEventKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event_type", content = "event", rename_all = "snake_case")]
pub enum SecurityEventKind {
    FileAction(FileAction),
    TaintTransition(TaintTransition),
    NetworkAction(NetworkAction),
    PolicyDecision(PolicyDecision),
    EnforcementState(EnforcementStateEvent),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventIdentity {
    pub binding_id: Uuid,
    pub agent_id: String,
    pub agent_name: Option<String>,
    pub session_id: Option<String>,
    pub conversation_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub pid: i32,
    pub process_start_time: u64,
    pub ppid: Option<i32>,
    pub cgroup_id: Option<u64>,
    pub protocol_version: u16,
    pub enforcer_version: String,
    pub actplane_revision: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode { Observe, Audit, Enforce }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationScope { PublicIpv4, PublicIp }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub policy_id: String,
    pub policy_revision: u64,
    pub source_event_id: Uuid,
    pub sink_event_id: Uuid,
    pub mode: PolicyMode,
    pub requested_effect: Effect,
    pub blocked: bool,
    pub killed: bool,
    pub errno: Option<i32>,
    pub risk_score: u8,
    pub reason: String,
}
```

`CredentialExfiltrationPolicy` contains policy ID, numeric revision, redacted source patterns, trusted endpoints, taint label, TTL seconds, destination scope, and mode. Version 3 accepts only `public_ipv4`; the reserved all-family `public_ip` scope fails validation before attachment because the pinned runtime does not emit IPv6 connects. Validation also rejects empty IDs, revision zero, empty sources, empty taint, TTL outside `1..=86400`, and more than 1,024 source or destination patterns.

Add `SubscribeSecurityEvents` to `Command` and `SecurityEvent` to the subscription response frame. Keep legacy `ViolationEvent` decoding for protocol version 1 during this plan; the enforcer converts it before publishing.

- [ ] **Step 4: Run protocol gates**

```bash
cd src/agentsight
cargo fmt --all -- --check
cargo clippy -p agentsight-enforcement-protocol --all-targets -- -D warnings
cargo test -p agentsight-enforcement-protocol
```

Expected: all protocol tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/agentsight/crates/enforcement-protocol
git commit -m "feat(sight): add security event contract"
```

---

### Task 2: Emit a deterministic credential-exfiltration chain from the mock enforcer

**Files:**
- Modify: `src/agentsight/crates/agentsight-enforcer/src/backend.rs`
- Modify: `src/agentsight/crates/agentsight-enforcer/src/event_hub.rs`
- Modify: `src/agentsight/crates/agentsight-enforcer/src/mock.rs`
- Modify: `src/agentsight/crates/agentsight-enforcer/src/service.rs`
- Test: `src/agentsight/crates/agentsight-enforcer/tests/security_events.rs`

**Interfaces:**
- Consumes: Task 1 `SecurityEvent` and foundational backend/service types.
- Produces: `EnforcementBackend::subscribe_security_events` and `MockBackend::emit_credential_exfiltration`.

- [ ] **Step 1: Write the failing UDS event-chain test**

```rust
#[test]
fn subscription_returns_ordered_source_taint_sink_and_decision() {
    let fixture = EnforcerFixture::start();
    let binding = fixture.apply_policy(fixture.audit_policy());
    let mut subscription = fixture.subscribe_security_events();
    fixture.backend.emit_credential_exfiltration(
        binding.request.binding_id,
        "/home/test/.ssh/id_rsa",
        "198.51.100.10:443",
    ).expect("fixture event chain should publish");

    let events = subscription.read_n(4);
    assert!(matches!(events[0].kind, SecurityEventKind::FileAction(_)));
    assert!(matches!(events[1].kind, SecurityEventKind::TaintTransition(_)));
    assert!(matches!(events[2].kind, SecurityEventKind::NetworkAction(_)));
    let SecurityEventKind::PolicyDecision(decision) = &events[3].kind else {
        panic!("fourth fixture event must be the decision");
    };
    assert_eq!(decision.mode, PolicyMode::Audit);
    assert!(!decision.blocked);
}
```

- [ ] **Step 2: Verify the test fails**

Run:

```bash
cd src/agentsight
cargo test -p agentsight-enforcer --no-default-features --features mock-backend --test security_events
```

Expected: FAIL because the security subscription and fixture emitter are missing.

- [ ] **Step 3: Change the event hub to publish stable security events**

```rust
pub trait EnforcementBackend: Send + Sync + 'static {
    fn health(&self) -> Result<HealthStatus, BackendError>;
    fn apply(&self, request: ApplyPolicy) -> Result<Binding, BackendError>;
    fn detach(&self, binding_id: Uuid) -> Result<(), BackendError>;
    fn bindings(&self) -> Result<Vec<Binding>, BackendError>;
    fn subscribe_security_events(&self) -> Receiver<SecurityEvent>;
}
```

`EventHub` remains bounded. It never blocks attach/detach; it increments a dropped-event counter and emits one `evidence_loss` enforcement-state event when a subscriber recovers. The mock emitter derives all four events from one binding, uses monotonically increasing timestamps, redacts the home prefix to `~`, and sets `blocked=false` in audit mode and `blocked=true` with `errno=EPERM` in enforce mode.

- [ ] **Step 4: Run enforcer mock gates**

```bash
cd src/agentsight
cargo fmt --all -- --check
cargo clippy -p agentsight-enforcer --all-targets --no-default-features --features mock-backend -- -D warnings
cargo test -p agentsight-enforcer --no-default-features --features mock-backend
```

Expected: all mock-backend tests pass without root or BPF.

- [ ] **Step 5: Commit**

```bash
git add src/agentsight/crates/agentsight-enforcer
git commit -m "feat(sight): stream mock security events"
```

---

### Task 3: Add the AgentSight-local security store

**Files:**
- Create: `src/agentsight/src/security.rs`
- Create: `src/agentsight/src/security/store.rs`
- Create: `src/agentsight/src/security/query.rs`
- Modify: `src/agentsight/src/lib.rs`
- Modify: `src/agentsight/src/storage/sqlite/mod.rs`
- Test: `src/agentsight/tests/security_store.rs`

**Interfaces:**
- Consumes: `SecurityEvent` and `CredentialExfiltrationPolicy`.
- Produces: `SecurityStore`, `RiskCase`, `RiskCaseStatus`, `SecurityEventFilter`, `SecuritySummary`, and `SecurityCountBy`.

- [ ] **Step 1: Write failing persistence and privacy tests**

```rust
#[test]
fn duplicate_event_is_idempotent_and_secret_content_is_absent() {
    let store = SecurityStore::open_in_memory().expect("fixture store should open");
    let event = fixture_file_action("~/.ssh/id_rsa");
    assert_eq!(store.insert_event(&event).expect("first insert should work"), true);
    assert_eq!(store.insert_event(&event).expect("duplicate should work"), false);

    let stored = store.event(event.event_id).expect("query should work")
        .expect("event should exist");
    let json = serde_json::to_string(&stored).expect("fixture should serialize");
    assert!(!json.contains("PRIVATE KEY"));
    assert!(json.contains("~/.ssh/id_rsa"));
}

#[test]
fn list_events_clamps_limit_and_orders_newest_first() {
    let store = fixture_store_with_events(3);
    let page = store.list_events(&SecurityEventFilter {
        limit: 5_000,
        ..SecurityEventFilter::default()
    }).expect("query should work");
    assert_eq!(page.limit, 1_000);
    assert!(page.items.windows(2).all(|pair| pair[0].occurred_at_ns >= pair[1].occurred_at_ns));
}
```

- [ ] **Step 2: Verify the store test fails**

Run `cd src/agentsight && cargo test -p agentsight --test security_store`.

Expected: FAIL because the local security module and store do not exist.

- [ ] **Step 3: Implement the SQLite schema and named errors**

Use AgentSight's existing SQLite connection helper and one `security.db` file:

```sql
CREATE TABLE IF NOT EXISTS security_events (
  event_id TEXT PRIMARY KEY,
  event_type TEXT NOT NULL,
  occurred_at_ns INTEGER NOT NULL,
  observed_at_ns INTEGER NOT NULL,
  agent_id TEXT NOT NULL,
  agent_name TEXT,
  session_id TEXT,
  pid INTEGER NOT NULL,
  process_start_time INTEGER NOT NULL,
  binding_id TEXT NOT NULL,
  policy_id TEXT,
  policy_revision INTEGER,
  result TEXT,
  destination_class TEXT,
  event_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_security_events_time
  ON security_events(occurred_at_ns DESC);
CREATE INDEX IF NOT EXISTS idx_security_events_session_time
  ON security_events(session_id, occurred_at_ns DESC);

CREATE TABLE IF NOT EXISTS risk_cases (
  case_id TEXT PRIMARY KEY,
  correlation_key TEXT NOT NULL UNIQUE,
  policy_id TEXT NOT NULL,
  policy_revision INTEGER NOT NULL,
  agent_id TEXT NOT NULL,
  session_id TEXT,
  severity TEXT NOT NULL,
  risk_score INTEGER NOT NULL,
  status TEXT NOT NULL,
  blocked INTEGER NOT NULL,
  opened_at_ns INTEGER NOT NULL,
  updated_at_ns INTEGER NOT NULL,
  summary TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS risk_evidence_links (
  case_id TEXT NOT NULL,
  event_id TEXT NOT NULL,
  position INTEGER NOT NULL,
  PRIMARY KEY(case_id, event_id)
);
CREATE TABLE IF NOT EXISTS policy_revisions (
  policy_id TEXT NOT NULL,
  revision INTEGER NOT NULL,
  policy_json TEXT NOT NULL,
  created_at_ns INTEGER NOT NULL,
  PRIMARY KEY(policy_id, revision)
);
```

Add named `SecurityStoreError` variants for SQLite, serialization, invalid filter, and missing case. Use `INSERT OR IGNORE` for event IDs, transactions for case plus evidence links, bound parameters for every filter, `limit` clamped to `1..=1000`, and `offset` clamped to non-negative.

Keep the stored summary and the expanded detail separate so list queries do not deserialize every evidence event:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskCase {
    pub case_id: Uuid,
    pub correlation_key: String,
    pub policy_id: String,
    pub policy_revision: u64,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub severity: RiskSeverity,
    pub risk_score: u8,
    pub status: RiskCaseStatus,
    pub blocked: bool,
    pub opened_at_ns: u64,
    pub updated_at_ns: u64,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskCaseDetail {
    #[serde(flatten)]
    pub case: RiskCase,
    pub evidence: Vec<SecurityEvent>,
}
```

- [ ] **Step 4: Implement summary, grouping, event detail, and session queries**

`SecurityEventFilter` supports time range, event type, result, policy, agent, session, binding, limit, and offset. `count_by` accepts only `event_type`, `result`, `policy_id`, or `destination_class`; unknown fields return `InvalidFilter` instead of interpolating column names.

- [ ] **Step 5: Run store gates**

```bash
cd src/agentsight
cargo fmt --all -- --check
cargo clippy -p agentsight --all-targets -- -D warnings
cargo test -p agentsight --test security_store
```

Expected: all security-store tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/agentsight/src/security.rs src/agentsight/src/security src/agentsight/src/lib.rs src/agentsight/src/storage/sqlite/mod.rs src/agentsight/tests/security_store.rs
git commit -m "feat(sight): store local security events"
```

---

### Task 4: Correlate events into one explainable risk case

**Files:**
- Create: `src/agentsight/src/security/coordinator.rs`
- Modify: `src/agentsight/src/security.rs`
- Test: `src/agentsight/tests/security_pipeline.rs`

**Interfaces:**
- Consumes: `EnforcementClient::subscribe_security_events`, `SecurityStore`, and Task 1 event kinds.
- Produces: `SecurityCoordinator::start`, `SecurityCoordinator::ingest`, and persisted `RiskCase` evidence order.

- [ ] **Step 1: Write the failing end-to-end mock pipeline test**

```rust
#[test]
fn audit_chain_creates_one_open_unblocked_case_with_ordered_evidence() {
    let fixture = SecurityPipelineFixture::start(PolicyMode::Audit);
    fixture.emit_credential_exfiltration();
    let case = fixture.wait_for_one_case();

    assert_eq!(case.case.status, RiskCaseStatus::Open);
    assert_eq!(case.case.risk_score, 85);
    assert!(!case.case.blocked);
    assert_eq!(case.evidence.len(), 4);
    assert!(matches!(case.evidence[0].kind, SecurityEventKind::FileAction(_)));
    assert!(matches!(case.evidence[3].kind, SecurityEventKind::PolicyDecision(_)));
}

#[test]
fn repeated_decision_does_not_duplicate_the_case() {
    let fixture = SecurityPipelineFixture::start(PolicyMode::Audit);
    fixture.emit_same_chain_twice();
    assert_eq!(fixture.wait_for_cases().len(), 1);
}
```

- [ ] **Step 2: Verify the pipeline test fails**

Run `cd src/agentsight && cargo test -p agentsight --test security_pipeline`.

Expected: FAIL because the coordinator and risk-case correlation are missing.

- [ ] **Step 3: Implement ingestion and deterministic correlation**

```rust
pub struct SecurityCoordinator {
    client: EnforcementClient,
    store: SecurityStore,
    stop: Arc<AtomicBool>,
}

impl SecurityCoordinator {
    pub fn start(&self) -> Result<JoinHandle<()>, SecurityCoordinatorError>;
    pub fn ingest(&self, event: SecurityEvent) -> Result<(), SecurityCoordinatorError>;
}
```

Persist every event before correlation. A `PolicyDecision` creates or updates one case using correlation key `binding_id:policy_id:policy_revision:source_event_id:sink_event_id`. Link evidence in occurred-time order and return it as `RiskCaseDetail`. Severity is `critical` for blocked high-risk sinks, `high` for unknown-public matches, and `medium` for trusted-destination audit matches. Do not create a formal case in observe mode.

Subscription reconnect uses bounded exponential backoff from 100 ms to 5 seconds. A disconnect inserts `enforcement_state=subscription_lost` and marks active binding health degraded; reconnect inserts `subscription_restored`. Never delete desired policy state.

- [ ] **Step 4: Run pipeline and regression gates**

```bash
cd src/agentsight
cargo fmt --all -- --check
cargo clippy -p agentsight --all-targets -- -D warnings
cargo test -p agentsight --test security_pipeline
cargo test -p agentsight
```

Expected: the new pipeline and existing AgentSight tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/agentsight/src/security.rs src/agentsight/src/security/coordinator.rs src/agentsight/tests/security_pipeline.rs
git commit -m "feat(sight): correlate security risk cases"
```

---

### Task 5: Serve a dedicated local System Audit API

**Files:**
- Modify: `src/agentsight/src/server/mod.rs`
- Create: `src/agentsight/src/server/system_audit.rs`
- Test: `src/agentsight/src/server/system_audit.rs`

**Interfaces:**
- Consumes: `SecurityStore` and `SecurityCoordinator`.
- Produces: local `/api/audit/*` event, session, summary, and case responses with no changes to compatibility routes.

- [ ] **Step 1: Add failing dedicated audit-route tests**

```rust
#[actix_web::test]
async fn audit_summary_is_local_ready_without_agent_sec_socket() {
    let app = test_app_with_security_events(vec![]).await;
    let request = test::TestRequest::get().uri("/api/audit/summary").to_request();
    let body: Value = test::call_and_read_body_json(&app, request).await;
    assert_eq!(body["state"], "empty");
    assert_eq!(body["data"]["source"], "agentsight");
}

#[actix_web::test]
async fn case_detail_returns_ordered_evidence() {
    let fixture = test_app_with_one_case().await;
    let request = test::TestRequest::get()
        .uri(&format!("/api/audit/cases/{}", fixture.case_id))
        .to_request();
    let body: Value = test::call_and_read_body_json(&fixture.app, request).await;
    assert_eq!(body["data"]["evidence"].as_array().map(Vec::len), Some(4));
}
```

- [ ] **Step 2: Verify focused System Audit tests fail**

Run `cd src/agentsight && cargo test -p agentsight server::system_audit::tests` on Linux.

Expected: FAIL because the dedicated local routes and grouped store queries are missing.

- [ ] **Step 3: Put the local store in `AppState` and add narrow routes**

Add the store without removing compatibility state:

```rust
pub struct AppState {
    // existing fields remain
    pub security_store: Arc<SecurityStore>,
    pub enforcement: Option<Arc<EnforcementCoordinator>>,
}
```

Leave all existing compatibility endpoints and handlers untouched. Add:

```text
GET  /api/audit/summary
GET  /api/audit/events
GET  /api/audit/sessions
GET  /api/audit/cases
GET  /api/audit/cases/{case_id}
POST /api/audit/cases/{case_id}/review
```

The review body accepts only `confirmed`, `false_positive`, `accepted_risk`, or `resolved`. It updates review state but never publishes a policy revision. Session grouping and pagination happen in SQLite before the response is built; no endpoint may group a capped event page.

- [ ] **Step 4: Run server gates**

```bash
cd src/agentsight
cargo fmt --all -- --check
cargo clippy -p agentsight --all-targets -- -D warnings
cargo test -p agentsight server::system_audit::tests
cargo test -p agentsight --test security_store
cargo test -p agentsight
```

Expected: dedicated local audit APIs pass without an AgentSecCore daemon or socket.

- [ ] **Step 5: Commit**

```bash
git add src/agentsight/src/server src/agentsight/src/security
git commit -m "feat(sight): serve local security audit"
```

---

### Task 6: Add the dedicated System Audit Dashboard workspace

**Files:**
- Modify: `src/agentsight/dashboard/src/utils/apiClient.ts`
- Create: `src/agentsight/dashboard/src/pages/SystemAuditPage.tsx`
- Modify: `src/agentsight/dashboard/src/App.tsx`
- Modify: `src/agentsight/dashboard/src/components/NavBar.tsx`

**Interfaces:**
- Consumes: Task 5 API envelopes.
- Produces: a capability-gated System Audit route with event/session/case navigation, ordered evidence, and review actions.

- [ ] **Step 1: Define the manual acceptance contract**

```text
- /audit is visible only with the existing Linux security capability.
- Empty, loading, error, audit-allow, and kernel-blocked states are distinct.
- Case detail renders source, taint, sink, and decision evidence in order.
- Review actions refresh the case without changing policy state.
- Existing compatibility pages render byte-for-byte as before.
```

- [ ] **Step 2: Add local API types and interactions**

Add dedicated audit summary/event/session/case response types plus `fetchAuditSummary`, `fetchAuditEvents`, `fetchAuditSessions`, `fetchSecurityCases`, `fetchSecurityCase`, and `reviewSecurityCase`.

`SystemAuditPage` renders four explicit answers: what happened, observed block result, reason, and impact. Render evidence in source-read, taint, network-sink, decision order. Use distinct copy for audit allow, requested-but-failed deny, confirmed kernel block, and enforcer unavailable. Exception creation is out of scope and must not call a policy-publish API.

- [ ] **Step 3: Run Dashboard gates and manual acceptance**

```bash
cd src/agentsight/dashboard
npm run typecheck
npm run build:embed
```

Expected: typecheck and the embedded production build pass, followed by the acceptance checks from Step 1 in a browser.

- [ ] **Step 5: Commit**

```bash
git add src/agentsight/dashboard
git commit -m "feat(sight): show local security risks"
```

---

### Task 7: Translate the product policy and normalize real ActPlane evidence

**Files:**
- Modify: `src/agentsight/crates/agentsight-enforcer/src/actplane.rs`
- Test: `src/agentsight/crates/agentsight-enforcer/src/actplane.rs`
- Create: `src/agentsight/integration-tests/fixtures/credential-exfiltration-policy.json`

**Interfaces:**
- Consumes: `CredentialExfiltrationPolicy`, official ActPlane compiler/runtime, and raw violation provenance.
- Produces: `compile_credential_exfiltration_policy` and real `SecurityEvent` conversion.

- [ ] **Step 1: Write failing pure translation and conversion tests**

```rust
#[test]
fn trusted_endpoint_becomes_an_unless_exception() {
    let policy = fixture_audit_policy_with_trusted_endpoint("10.0.0.8");
    let dsl = compile_credential_exfiltration_policy(&policy)
        .expect("fixture policy should compile");
    assert!(dsl.contains("source CREDENTIAL"));
    assert!(dsl.contains("notify connect endpoint \"*\" if CREDENTIAL"));
    assert!(dsl.contains("10.0.0.8"));
}

#[test]
fn enforce_rejects_unrepresentable_ttl_and_public_scope() {
    let policy = fixture_policy(PolicyMode::Enforce);
    let error = compile_credential_exfiltration_policy(&policy)
        .expect_err("kernel policy must not weaken product semantics");
    assert!(error.to_string().contains("taint TTL"));
    assert!(error.to_string().contains("public_ipv4 destinations"));
}

#[test]
fn raw_block_violation_preserves_provenance_and_eperm() {
    let event = convert_violation(fixture_block_violation(), &fixture_binding())
        .expect("fixture violation should convert");
    let SecurityEventKind::PolicyDecision(decision) = event.kind else {
        panic!("fixture must produce a policy decision");
    };
    assert!(decision.blocked);
    assert_eq!(decision.errno, Some(libc::EPERM));
}
```

- [ ] **Step 2: Verify Linux feature tests fail**

```bash
cd src/agentsight
cargo test -p agentsight-enforcer --no-default-features --features actplane actplane::tests
```

Expected: FAIL because product-policy translation and stable event conversion are missing.

- [ ] **Step 3: Implement adapter-only translation**

Generate deterministic DSL with sorted source and trusted-endpoint patterns. Escape only characters accepted by the pinned parser and reject newline, NUL, and quote injection before compilation. The pinned ABI has no duration primitive and only one endpoint exception, so observe/audit use a notify rule followed by adapter-side provenance-age and `public_ipv4` filtering. Reject all-family scope and enforce mode explicitly: the runtime lacks IPv6 connect events, post-LSM filtering cannot restore an expired or excluded connection, and silently using a wildcard block would weaken the product contract.

Store the original product mode, TTL, and scope with the active binding rather than inferring mode from the notify effect. Convert in-scope raw provenance into ordered `FileAction`, `TaintTransition`, `NetworkAction`, and `PolicyDecision` events. Accept only globally routable IPv4 sinks; suppress IPv6, unspecified, private, loopback, link-local, multicast, broadcast, documentation, benchmarking, shared-address, reserved, expired, and trusted sinks. Preserve raw kernel operation, target, label name, origin PID/time, rule ID/reason, domain, and observed blocked/killed bits. Redact configured home prefixes before publishing.

- [ ] **Step 4: Run Linux adapter gates**

```bash
cd src/agentsight
cargo fmt --all -- --check
cargo clippy -p agentsight-enforcer --all-targets --no-default-features --features actplane -- -D warnings
cargo test -p agentsight-enforcer --no-default-features --features actplane --lib
```

Expected: all adapter unit tests pass on Linux.

- [ ] **Step 5: Commit**

```bash
git add src/agentsight/crates/agentsight-enforcer/src/actplane.rs src/agentsight/integration-tests/fixtures
git commit -m "feat(sight): translate exfiltration policy"
```

---

### Task 8: Prove observe, audit, safe enforce rejection, expiry, and restart on real Linux

**Files:**
- Create: `src/agentsight/scripts/test-security-audit-integration.sh`
- Modify: `src/agentsight/README.md`
- Modify: `src/agentsight/README_zh.md`
- Modify: `src/agentsight/docs/design/security-audit-progressive-enforcement.md`

**Interfaces:**
- Produces: reproducible Linux evidence for the accepted credential-exfiltration flow and measured resource budgets.

- [ ] **Step 1: Write the guarded acceptance script**

The script uses `set -euo pipefail`, an EXIT cleanup trap, and only `/tmp/agentsight-security-e2e`. It creates a fake SSH key containing no real credential, starts a dedicated Agent process tree, records PID plus `/proc/<pid>/stat` start time, and uses a local TCP sink plus a trusted local endpoint so the test does not exfiltrate data.

Assertions:

```bash
assert_json '.data.mode == "observe" and .data.case_count == 0'
assert_json '.data.mode == "audit" and .data.blocked == false'
assert_json '.error.code == "compile_failure" and (.error.message | contains("taint TTL"))'
assert_json '.data.evidence | map(.event_type) == ["file_action","taint_transition","network_action","policy_decision"]'
```

The script also verifies private and loopback endpoints are excluded, a trusted endpoint is allowed, a child process inherits taint, an unrelated process remains unaffected, TTL expiry restores access, unsupported enforce application fails explicitly, and enforcer restart reconciliation never reports unknown state as enforced.

- [ ] **Step 2: Run the full local mock and Dashboard gate**

```bash
cd src/agentsight
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
cd dashboard
npm run typecheck
npm run build:embed
```

Expected: every command exits 0.

- [ ] **Step 3: Deploy to the configured Linux acceptance host**

Use `AGENTSIGHT_TEST_HOST` rather than committing an IP address:

```bash
test -n "${AGENTSIGHT_TEST_HOST:?set the isolated Linux acceptance host}"
rsync -a --delete --exclude target/ src/agentsight/ "root@${AGENTSIGHT_TEST_HOST}:/tmp/agentsight-security-src/"
ssh "root@${AGENTSIGHT_TEST_HOST}" 'cd /tmp/agentsight-security-src && make build-all'
ssh "root@${AGENTSIGHT_TEST_HOST}" '/usr/local/bin/test-security-audit-integration.sh'
```

Expected: observe, audit, trusted exception, isolation, expiry, safe enforce rejection, and restart assertions pass. Enforce application returns an actionable compile failure rather than installing a broader wildcard block.

- [ ] **Step 4: Measure the release budget**

Record idle RSS/CPU before and after enabling the enforcer and measure P99 decision latency over at least 10,000 in-scope audit notifications plus 10,000 excluded internal or expired operations. Fail the acceptance script when added idle RSS is >=30 MiB, added idle CPU is >=1%, or P99 overhead is >=2 ms. Record event drop counters and require zero loss for the acceptance load.

- [ ] **Step 5: Update bilingual usage docs and measured design status**

Document the internal enforcer, BPF-LSM/BTF requirements, observe/audit semantics, explicit enforce rejection, default evidence exclusions, fail-open attachment behavior, and the exact supported credential policy. Mark only measured capabilities as implemented and move unimplemented items to later planning.

- [ ] **Step 6: Restore the host and commit**

The cleanup removes `/tmp/agentsight-security-e2e`, restores previous unit states and policy bindings, and removes staged source. Then commit:

```bash
git add src/agentsight/scripts/test-security-audit-integration.sh src/agentsight/README.md src/agentsight/README_zh.md src/agentsight/docs/design/security-audit-progressive-enforcement.md
git commit -m "test(sight): verify security audit loop"
```

---

## Final Review Checklist

- [ ] The foundational ActPlane integration plan is complete and its Linux denial still passes.
- [ ] `git diff --check` succeeds.
- [ ] No AgentSecCore socket or process is required for the independent `/api/audit/*` surface; existing `/api/security/*` compatibility behavior remains untouched.
- [ ] No vendored or path-based ActPlane dependency was added.
- [ ] Protocol and mock tests pass without BPF privileges.
- [ ] Local security events are idempotent and evidence order is deterministic.
- [ ] Observe creates metrics only; audit creates an unblocked case; unsupported enforce fails before attachment.
- [ ] Product policies accept only `public_ipv4`; globally routable IPv4 emits evidence while IPv6 and special-purpose ranges do not.
- [ ] Requested effect and observed result remain separate in protocol, storage, API, and UI.
- [ ] Sensitive contents and authentication values do not appear in fixtures, SQLite, logs, API, or UI.
- [ ] Trusted endpoints, process-tree isolation, PID reuse checks, TTL expiry, disconnect, and restart reconciliation pass.
- [ ] Existing process, session, LLM, Token, interruption, and Dashboard tests pass.
- [ ] Rust format, clippy, test, and doc gates pass.
- [ ] Dashboard typecheck, embedded build, and manual acceptance checks pass.
- [ ] Real Linux acceptance proves no wildcard block is attached for unsupported enforce semantics.
- [ ] Resource budgets and event-loss counters meet the documented thresholds.
- [ ] The acceptance host is restored to its pre-test state.
