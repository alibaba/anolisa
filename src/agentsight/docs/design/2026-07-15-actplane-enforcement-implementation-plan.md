# ActPlane Enforcement Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a real AgentSight-to-ActPlane path that blocks one Linux operation with BPF-LSM and exposes the violation through AgentSight's API.

**Architecture:** AgentSight owns desired state, audit persistence, and HTTP APIs. A separate `agentsight-enforcer` owns the privileged ActPlane runtime behind a versioned UDS protocol. The first acceptance path uses authenticated explicit PID attachment; later procmon automation calls the same coordinator.

**Tech Stack:** Rust 2024, Unix domain sockets, bounded NDJSON, rusqlite, systemd, and official ActPlane 0.1.8 pinned to `a62e5d9d96f91101cda019519053e950d532380a`.

## Global Constraints

- AgentSight and ActPlane enforcement are Linux-only; do not build their BPF backends on macOS.
- Keep AgentSight libbpf-rs probes and ActPlane Aya runtime isolated.
- Do not add `mod.rs` or use `unwrap()`, `expect()`, or `panic!()` in production code.
- New third-party versions belong in `[workspace.dependencies]`.
- All ActPlane imports stay in `crates/agentsight-enforcer/src/actplane.rs`.
- Never depend on moving `master` or add the local `oslevel-harness/actplane` copy.
- Only an ActPlane acknowledgement may produce state `enforced`.
- Attachment failures are fail-open and remain visible as failed or unavailable.
- Remote mutations are limited to AgentSight artifacts, its units, and `/tmp/agentsight-enforcement-e2e` on `11.164.2.206`.

## File Map

- `crates/enforcement-protocol/`: stable types and bounded NDJSON codec.
- `crates/agentsight-enforcer/`: backend trait, UDS daemon, mock, and ActPlane adapter.
- `src/enforcement.rs` and `src/enforcement/`: AgentSight client, store, and coordinator.
- `src/server/{mod.rs,handlers.rs}`: authenticated control and query APIs.
- `scripts/agentsight-enforcer.service`: privileged service unit.
- `scripts/test-enforcement-integration.sh`: real Linux acceptance test.

---

### Task 1: Create the versioned enforcement protocol

**Files:**
- Modify: `src/agentsight/Cargo.toml`
- Create: `src/agentsight/crates/enforcement-protocol/Cargo.toml`
- Create: `src/agentsight/crates/enforcement-protocol/src/lib.rs`

**Interfaces:**
- Produces: `Request`, `Command`, `Response`, `ResponseBody`, `HealthStatus`, `ApplyPolicy`, `Binding`, `BindingState`, `ViolationEvent`, `Effect`, `ProtocolError`, `read_frame`, and `write_frame`.

- [ ] **Step 1: Add the workspace shell and protocol manifest**

Add:

```toml
[workspace]
members = [".", "crates/enforcement-protocol"]
resolver = "2"

[workspace.dependencies]
anyhow = "1"
libc = "0.2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"
uuid = { version = "1", features = ["v4", "serde"] }
```

Create the member manifest with those four serialization/error dependencies using `workspace = true`.

- [ ] **Step 2: Write failing protocol tests**

```rust
#[test]
fn request_round_trips_as_one_frame() {
    let request = Request::new(Command::Health);
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &request).expect("fixture should encode");
    let decoded: Request = read_frame(&mut BufReader::new(Cursor::new(bytes)))
        .expect("fixture should decode")
        .expect("frame should exist");
    assert_eq!(decoded, request);
}

#[test]
fn oversized_frame_is_rejected() {
    let input = vec![b'x'; MAX_FRAME_BYTES + 1];
    let error = read_frame::<_, Request>(&mut BufReader::new(Cursor::new(input)))
        .expect_err("oversized input must fail");
    assert!(matches!(error, ProtocolError::FrameTooLarge { .. }));
}
```

- [ ] **Step 3: Verify the test fails**

Run `cargo test -p agentsight-enforcement-protocol`.

Expected: missing protocol types and codec.

- [ ] **Step 4: Implement the schema and codec**

Use these exact roots:

```rust
pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

pub struct Request { pub protocol_version: u16, pub request_id: Uuid, pub command: Command }
pub enum Command {
    Health,
    ApplyPolicy(ApplyPolicy),
    DetachAgent { binding_id: Uuid },
    ListBindings,
    SubscribeViolations,
}
pub struct ApplyPolicy {
    pub binding_id: Uuid,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub root_pid: i32,
    pub process_start_time: u64,
    pub policy_id: String,
    pub policy_revision: String,
    pub policy_dsl: String,
}
pub enum BindingState { Pending, Enforced, Failed, Degraded, Detaching, Detached }
pub struct Binding {
    pub request: ApplyPolicy,
    pub state: BindingState,
    pub message: Option<String>,
    pub domain_id: Option<u32>,
}
pub enum Effect { Notify, Block, Kill }
pub struct HealthStatus { pub ready: bool, pub backend: String, pub message: Option<String> }
```

All types derive `Clone`, `Debug`, `PartialEq`, `Eq`, `Serialize`, and `Deserialize`. `ViolationEvent` contains event/binding UUIDs, Agent/session/policy identity, PID/PPID/start time, operation/target, effect/blocked/killed, rule metadata, two timestamps, and ActPlane revision. `Response` correlates `request_id` and carries `Result<ResponseBody, RemoteError>`.

`read_frame` reads at most `MAX_FRAME_BYTES + 1`, requires a newline, validates `protocol_version`, and rejects trailing oversized data. `write_frame` serializes one object, appends one newline, and flushes.

- [ ] **Step 5: Run gates and commit**

```bash
cargo fmt --all -- --check
cargo clippy -p agentsight-enforcement-protocol --all-targets -- -D warnings
cargo test -p agentsight-enforcement-protocol
git add src/agentsight/Cargo.toml src/agentsight/crates/enforcement-protocol
git commit -m "feat(sight): add enforcement protocol"
```

---

### Task 2: Build the UDS enforcer with a mock backend

**Files:**
- Create: `src/agentsight/crates/agentsight-enforcer/Cargo.toml`
- Create: `src/agentsight/crates/agentsight-enforcer/src/{lib.rs,backend.rs,event_hub.rs,mock.rs,service.rs,main.rs}`
- Create: `src/agentsight/crates/agentsight-enforcer/tests/service.rs`

**Interfaces:**
- Consumes: Task 1 protocol.
- Produces: `EnforcementBackend`, `BackendError`, `MockBackend`, `EventHub`, and `EnforcerService`.

- [ ] **Step 1: Add the daemon member and mock feature**

Add `crates/agentsight-enforcer` to the root workspace members. Keep this task
independent of Git and BPF dependencies so its protocol and UDS tests are
portable and deterministic.

```toml
[features]
default = ["mock-backend"]
mock-backend = []

[dependencies]
agentsight-enforcement-protocol = { path = "../enforcement-protocol" }
anyhow = { workspace = true }
libc = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
uuid = { workspace = true }
```

- [ ] **Step 2: Write failing idempotency and UDS tests**

```rust
#[test]
fn changed_duplicate_conflicts() {
    let backend = MockBackend::new();
    let request = fixture_apply_policy();
    assert!(backend.apply(request.clone()).is_ok());
    assert!(backend.apply(request.clone()).is_ok());
    let mut changed = request;
    changed.policy_revision = "revision-2".into();
    assert!(matches!(backend.apply(changed), Err(BackendError::BindingConflict(_))));
}
```

The integration test starts a temporary UDS service and exercises `Health`, `ApplyPolicy`, `ListBindings`, `SubscribeViolations`, and `DetachAgent`, asserting correlated request IDs and state transitions.

- [ ] **Step 3: Verify tests fail**

Run `cargo test -p agentsight-enforcer --no-default-features --features mock-backend`.

- [ ] **Step 4: Implement the backend and bounded event hub**

```rust
pub trait EnforcementBackend: Send + Sync + 'static {
    fn health(&self) -> Result<HealthStatus, BackendError>;
    fn apply(&self, request: ApplyPolicy) -> Result<Binding, BackendError>;
    fn detach(&self, binding_id: Uuid) -> Result<(), BackendError>;
    fn bindings(&self) -> Result<Vec<Binding>, BackendError>;
    fn subscribe(&self) -> Receiver<ViolationEvent>;
}
```

Define named errors for conflict, missing binding, stale process, compile failure, and kernel failure. `EventHub` uses bounded `sync_channel`; publishing drops slow/disconnected subscribers instead of blocking policy lifecycle. `MockBackend` stores one active binding, returns `enforced`, and can inject violations for tests.

- [ ] **Step 5: Implement service dispatch and peer checks**

The service binds mode `0660`, accepts one thread per connection, reads one request for ordinary calls, and keeps subscription connections open. On Linux, authorize `SO_PEERCRED` UID 0, service UID, or configured AgentSight group. Reject malformed, oversized, or version-incompatible frames with typed errors. The binary defaults to `/run/agentsight/enforcer.sock` and logs clearly when compiled with only the mock backend.

- [ ] **Step 6: Run gates and commit**

```bash
cargo fmt --all -- --check
cargo clippy -p agentsight-enforcer --all-targets --no-default-features --features mock-backend -- -D warnings
cargo test -p agentsight-enforcer --no-default-features --features mock-backend
git add src/agentsight/crates/agentsight-enforcer
git commit -m "feat(sight): add enforcer daemon"
```

---

### Task 3: Integrate the official ActPlane backend

**Files:**
- Create: `src/agentsight/crates/agentsight-enforcer/src/actplane.rs`
- Modify: `src/agentsight/crates/agentsight-enforcer/src/{lib.rs,main.rs}`

**Interfaces:**
- Consumes: `EnforcementBackend`, `EventHub`, `actplane_ifc_compiler::compile_str`, and `ebpf_ifc_engine::PinnedEngine`.
- Produces: `ActPlaneBackend::open() -> Result<Self, BackendError>`.

- [ ] **Step 0: Add the pinned upstream feature dependencies**

Add the `actplane` feature and the two optional official Git dependencies to the
enforcer manifest. Declare both in `[workspace.dependencies]`, pinned to
`a62e5d9d96f91101cda019519053e950d532380a`:

```toml
actplane-ifc-compiler = { git = "https://github.com/eunomia-bpf/ActPlane.git", rev = "a62e5d9d96f91101cda019519053e950d532380a" }
ebpf-ifc-engine = { git = "https://github.com/eunomia-bpf/ActPlane.git", rev = "a62e5d9d96f91101cda019519053e950d532380a" }
```

- [ ] **Step 1: Write failing pure-helper tests**

```rust
#[test]
fn domain_id_is_stable_and_nonzero() {
    let id = Uuid::parse_str("00000000-0000-4000-8000-000000000123")
        .expect("fixture UUID should parse");
    assert_eq!(domain_id(id), domain_id(id));
    assert_ne!(domain_id(id), 0);
}

#[test]
fn proc_start_time_handles_parenthesized_comm() {
    let stat = "42 (a tricky) name) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 98765";
    assert_eq!(parse_process_start_time(stat), Ok(98765));
}
```

Also test raw violation conversion with fixture rule metadata, including `blocked`, effect, normalized operation, policy revision, and domain lookup.

- [ ] **Step 2: Verify the feature build fails**

```bash
cargo test -p agentsight-enforcer --no-default-features --features actplane actplane::tests
```

Expected: `ActPlaneBackend` and helpers are missing.

- [ ] **Step 3: Open the pinned singleton safely**

`ActPlaneBackend::open` must retain the runtime lock for its full lifetime:

```rust
let engine = Arc::new(PinnedEngine::open_or_install_singleton().map_err(kernel_error)?);
let runtime_lock = engine.try_lock_runtime().map_err(kernel_error)?;
let reload = engine.reload_handle().map_err(kernel_error)?;
engine.protect_pid(std::process::id() as i32).map_err(kernel_error)?;
reload.clear_runtime_state().map_err(kernel_error)?;
```

Spawn one thread calling `PinnedEngine::run`. Convert each raw violation through a domain-to-binding map and publish it to `EventHub`. `Drop` sets an atomic stop flag and joins the poller.

- [ ] **Step 4: Implement single-binding apply and detach**

`apply` performs these ordered operations:

1. Compare `/proc/<pid>/stat` field 22 to `process_start_time`.
2. Return an identical binding idempotently; reject changed or second active bindings.
3. Compile DSL with `compile_str` and require `COMMAND` or `AGENT` label.
4. Derive a stable nonzero domain ID from the binding UUID.
5. Seed the target PID into the domain with that label.
6. Bind the enforcer PID temporarily using:

```rust
CapState {
    scope_id: 1,
    labels: agent_label,
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
}
```

7. Call `append_policy_delta(control_pid, domain_id, &compiled.bytes)`.
8. Unbind the control PID and store rule metadata.

On partial failure, unbind and clear runtime state before returning `failed`. `detach` unbinds the target and clears runtime state; this is safe because the first milestone deliberately allows one active binding.

- [ ] **Step 5: Run Linux gates and commit**

```bash
cargo fmt --all -- --check
cargo clippy -p agentsight-enforcer --all-targets --no-default-features --features actplane -- -D warnings
cargo test -p agentsight-enforcer --no-default-features --features actplane --lib
git add src/agentsight/Cargo.toml src/agentsight/Cargo.lock src/agentsight/crates/agentsight-enforcer
git commit -m "feat(sight): integrate ActPlane backend"
```

---

### Task 4: Add AgentSight coordination, persistence, and APIs

**Files:**
- Modify: `src/agentsight/Cargo.toml`
- Modify: `src/agentsight/src/lib.rs`
- Create: `src/agentsight/src/enforcement.rs`
- Create: `src/agentsight/src/enforcement/{client.rs,store.rs,coordinator.rs}`
- Modify: `src/agentsight/src/server/{mod.rs,handlers.rs}`
- Create: `src/agentsight/tests/enforcement_pipeline.rs`

**Interfaces:**
- Consumes: protocol and UDS daemon.
- Produces: `EnforcementClient`, `EnforcementStore`, `EnforcementCoordinator`, and `/api/enforcement/*`.

- [ ] **Step 1: Add the protocol path dependency and failing pipeline test**

```rust
#[test]
fn apply_persists_enforced_state_and_deduplicates_violation() {
    let fixture = TestEnforcer::start();
    let store = EnforcementStore::open(fixture.database_path())
        .expect("temporary store should open");
    let coordinator = EnforcementCoordinator::new(
        EnforcementClient::new(fixture.socket_path()),
        store,
    );
    let binding = coordinator.apply(fixture.apply_request())
        .expect("mock apply should work");
    assert_eq!(binding.state, BindingState::Enforced);
    fixture.publish_same_violation_twice();
    fixture.wait_for_ingestion();
    assert_eq!(coordinator.violations(100).expect("query should work").len(), 1);
}
```

Run `cargo test -p agentsight --test enforcement_pipeline` and expect missing-module failures.

- [ ] **Step 2: Implement the bounded synchronous client**

```rust
pub fn health(&self) -> Result<HealthStatus, EnforcementError>;
pub fn apply(&self, request: ApplyPolicy) -> Result<Binding, EnforcementError>;
pub fn detach(&self, binding_id: Uuid) -> Result<(), EnforcementError>;
pub fn bindings(&self) -> Result<Vec<Binding>, EnforcementError>;
pub fn subscribe(&self) -> Result<ViolationSubscription, EnforcementError>;
```

Each ordinary call opens one Unix connection with read/write timeouts. Subscription owns a separate connection and uses the shared bounded codec.

- [ ] **Step 3: Implement SQLite state**

Use `enforcement.db` under AgentSight's default base directory:

```sql
CREATE TABLE IF NOT EXISTS enforcement_bindings (
  binding_id TEXT PRIMARY KEY,
  desired_json TEXT NOT NULL,
  state TEXT NOT NULL,
  message TEXT,
  domain_id INTEGER,
  updated_at_ns INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS enforcement_violations (
  event_id TEXT PRIMARY KEY,
  binding_id TEXT NOT NULL,
  occurred_at_ns INTEGER NOT NULL,
  event_json TEXT NOT NULL
);
```

Expose `upsert_binding`, `get_binding`, `list_bindings`, `insert_violation`, and `list_violations`. Use `INSERT OR IGNORE` for event IDs and clamp query limits to `1..=1000`.

- [ ] **Step 4: Implement coordinator state transitions**

`apply` persists `pending`, calls UDS, then persists acknowledged `enforced` or sanitized `failed`. `detach` persists `detaching` and only then acknowledged `detached`. `start_ingestion` reconnects with bounded exponential backoff and idempotently inserts violations; loss of subscription marks active state `degraded` without discarding desired state.

- [ ] **Step 5: Register authenticated API operations**

```text
GET    /api/enforcement/health
POST   /api/enforcement/bindings
GET    /api/enforcement/bindings
DELETE /api/enforcement/bindings/{binding_id}
GET    /api/enforcement/violations?limit=100
```

POST generates a missing binding UUID, verifies `/proc/<pid>/stat`, and rejects PID 1, AgentSight, and enforcer processes. Return 201 only for `enforced`, 409 for conflict, 422 for stale PID/invalid DSL, and 503 for unavailable enforcement. Initialize one coordinator in `AppState` using `AGENTSIGHT_ENFORCER_SOCKET` or `/run/agentsight/enforcer.sock` and start ingestion with the server.

- [ ] **Step 6: Run Linux gates and commit**

```bash
cargo fmt --all -- --check
cargo clippy -p agentsight --all-targets -- -D warnings
cargo test -p agentsight --test enforcement_pipeline
cargo test -p agentsight
git add src/agentsight/Cargo.toml src/agentsight/Cargo.lock src/agentsight/src src/agentsight/tests/enforcement_pipeline.rs
git commit -m "feat(sight): add enforcement control API"
```

---

### Task 5: Package and supervise the enforcer

**Files:**
- Create: `src/agentsight/scripts/agentsight-enforcer.service`
- Modify: `src/agentsight/scripts/agentsight.service`
- Modify: `src/agentsight/Makefile`
- Modify: `scripts/build-all.sh`

**Interfaces:**
- Produces: installed production daemon and ordered systemd units.

- [ ] **Step 1: Create the unit**

```ini
[Unit]
Description=AgentSight ActPlane Enforcement Engine
Before=agentsight.service
PartOf=agentsight.service
After=local-fs.target

[Service]
Type=simple
ExecStart=/usr/local/bin/agentsight-enforcer
Restart=on-failure
RestartSec=2
RuntimeDirectory=agentsight
RuntimeDirectoryMode=0750
Environment=AGENTSIGHT_ENFORCER_SOCKET=/run/agentsight/enforcer.sock
NoNewPrivileges=no
PrivateTmp=yes
ProtectHome=yes
ProtectSystem=strict
ReadWritePaths=/run/agentsight /sys/fs/bpf
TimeoutStopSec=15s

[Install]
WantedBy=multi-user.target
```

Do not guess `CapabilityBoundingSet`; first prove the needed set on the acceptance host, then tighten it without breaking BPF-LSM loading.

- [ ] **Step 2: Update build and service ordering**

Build production with `cargo build --release -p agentsight-enforcer --no-default-features --features actplane`. Install/uninstall both binaries and units. Add `Wants=agentsight-enforcer.service` and ordering to `agentsight.service`. Reuse root build script systemd helpers to restart enforcer before AgentSight.

- [ ] **Step 3: Verify packaging and commit**

```bash
make -C src/agentsight build
DESTDIR="$(mktemp -d)" make -C src/agentsight install INSTALL_PROFILE=system SETCAP=0
systemd-analyze verify src/agentsight/scripts/agentsight-enforcer.service src/agentsight/scripts/agentsight.service
git add src/agentsight/Makefile src/agentsight/scripts scripts/build-all.sh
git commit -m "build(sight): install enforcement service"
```

---

### Task 6: Prove real BPF-LSM denial and document usage

**Files:**
- Create: `src/agentsight/scripts/test-enforcement-integration.sh`
- Modify: `src/agentsight/README.md`
- Modify: `src/agentsight/README_zh.md`

**Interfaces:**
- Produces: reproducible attach, denial, audit, detach, and cleanup evidence.

- [ ] **Step 1: Write the guarded acceptance script**

The script uses `set -euo pipefail`, a cleanup trap, and refuses to run unless BPF-LSM, BTF, and both services are active. It operates only in `/tmp/agentsight-enforcement-e2e` and applies:

```text
source COMMAND = exec "**/bash"
rule block_secret_read:
  block open file "/tmp/agentsight-enforcement-e2e/secret" if COMMAND
  because "AgentSight ActPlane integration test"
```

Start a stopped bash, capture `/proc/<pid>/stat` field 22, POST the binding, resume it, and require protected read to fail with `EPERM`. Poll violations for `blocked=true`, detach, then require the read to succeed.

- [ ] **Step 2: Deploy scoped sources and run Linux gates**

```bash
rsync -a --delete --exclude target/ src/agentsight/ root@11.164.2.206:/tmp/agentsight-fusion-src/
ssh root@11.164.2.206 'cd /tmp/agentsight-fusion-src && make build-all'
ssh root@11.164.2.206 '/usr/local/bin/test-enforcement-integration.sh'
ssh root@11.164.2.206 'systemctl restart agentsight-enforcer.service && curl -fsS http://127.0.0.1:7396/api/enforcement/health'
```

Also submit invalid DSL and a stale PID; both must return 422 and never become `enforced`.

- [ ] **Step 3: Run the full workspace gate**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
```

- [ ] **Step 4: Update bilingual component docs**

Document AgentSight port 7396, the internal privileged service, BPF-LSM/BTF prerequisites, explicit fail-open state, and the verified curl request. Do not claim automatic procmon policy binding in this milestone.

- [ ] **Step 5: Restore the host and commit**

Restore pre-test service states, remove the test directory and staged source, then commit:

```bash
git add src/agentsight/scripts/test-enforcement-integration.sh src/agentsight/README.md src/agentsight/README_zh.md
git commit -m "test(sight): verify ActPlane enforcement"
```

---

## Final Review Checklist

- [ ] `git diff --check` succeeds.
- [ ] No vendored or path-based ActPlane dependency was added.
- [ ] `cargo tree -p agentsight-enforcer --features actplane` resolves the pinned SHA.
- [ ] Mock tests pass without BPF privileges.
- [ ] Linux workspace format, clippy, test, and doc gates pass.
- [ ] The protected operation returns `EPERM` and AgentSight reports `blocked=true`.
- [ ] Detach restores access.
- [ ] Invalid DSL, stale PID, and enforcer absence never appear as `enforced`.
- [ ] Remote cleanup restores pre-test state.
- [ ] Existing AgentSight trace, token, AgentSecCore security, and Dashboard smoke checks still work.
