# AgentSight Protocol v4 Tool Context Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve real application conversation and tool-call identifiers in protocol v4 AgentSight system security events.

**Architecture:** Add optional producer-owned identifiers to the existing enforcement binding structs and HTTP requests, validate them at the API boundary, and copy them into enforcer event identity. Keep `agentsight_read_v2()` and the protocol v4 number unchanged because the JSON additions are backward-compatible.

**Tech Stack:** Rust, Serde, Actix Web, AgentSight enforcement protocol, ActPlane enforcer, Cargo tests.

## Global Constraints

- Keep `PROTOCOL_VERSION` equal to `4`.
- Do not add a new C FFI function or LoongCollector state.
- Never infer a missing tool-call identifier.
- Optional fields must decode as `None` when omitted by old clients.
- Identity values are trimmed, empty values become `None`, and values over 256 bytes are rejected.
- Linux release producer and enforcer must be built from the same commit before deployment.

---

### Task 1: Lock the Protocol v4 Contract

**Files:**
- Modify: `src/agentsight/crates/enforcement-protocol/src/lib.rs`
- Test: `src/agentsight/crates/enforcement-protocol/src/lib.rs`

**Interfaces:**
- Consumes: `ApplyCredentialPolicy` and `ApplyPolicy` protocol v4 JSON frames.
- Produces: optional `conversation_id: Option<String>` and `tool_call_id: Option<String>` fields with Serde defaults.

- [ ] **Step 1: Write failing tests**

Add a protocol round-trip test with both identity fields and an old-frame test that omits both fields but still decodes with protocol version 4.

- [ ] **Step 2: Verify RED**

Run `cargo test -p agentsight-enforcement-protocol` on Linux. Expect compilation or assertions to fail because the request structs do not expose the fields.

- [ ] **Step 3: Implement the optional fields**

Add documented `#[serde(default)]` fields to both binding structs without changing `PROTOCOL_VERSION`.

- [ ] **Step 4: Verify GREEN**

Run `cargo test -p agentsight-enforcement-protocol`. Expect all protocol tests to pass.

### Task 2: Carry Context Through HTTP and Enforcer Events

**Files:**
- Modify: `src/agentsight/src/server/enforcement.rs`
- Modify: `src/agentsight/crates/agentsight-enforcer/src/actplane.rs`
- Modify: `src/agentsight/crates/agentsight-enforcer/src/mock.rs`
- Modify: `src/agentsight/crates/agentsight-enforcer/src/service.rs`
- Test: `src/agentsight/src/server/enforcement.rs`
- Test: `src/agentsight/crates/agentsight-enforcer/tests/security_events.rs`
- Update affected protocol struct fixtures under `src/agentsight/`.

**Interfaces:**
- Consumes: optional HTTP binding fields supplied by the Agent adapter.
- Produces: exact `EventIdentity.conversation_id` and `EventIdentity.tool_call_id` values on all events for the binding.

- [ ] **Step 1: Write failing API and event tests**

Assert trimming, empty normalization, 256-byte validation, and exact propagation across the source/taint/sink/decision chain.

- [ ] **Step 2: Verify RED**

Run the focused server and enforcer tests on Linux. Expect failures because the API drops the fields and event identity is `None`.

- [ ] **Step 3: Implement minimal propagation**

Accept and normalize the two optional request fields, copy them through credential-to-policy conversion, and populate event identity from the active binding.

- [ ] **Step 4: Verify GREEN and compatibility**

Run focused tests, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo doc --workspace --no-deps`. Record any unrelated baseline failures separately and require all changed-component tests plus release builds to pass.

### Task 3: Deploy and Validate Exact Correlation

**Files:**
- No repository source changes.

**Interfaces:**
- Consumes: matching protocol v4 `agentsight` and ActPlane-backed `agentsight-enforcer` binaries.
- Produces: one strict positive incident and one mismatch rejection in the Hangzhou validation space.

- [ ] **Step 1: Build matching Linux release binaries**

Run `cargo build --release -p agentsight` and `make build-enforcer` from the same source revision.

- [ ] **Step 2: Deploy with paired rollback**

Back up both installed binaries, replace them together, restart services, and require `/api/enforcement/health` plus a binding round trip to succeed.

- [ ] **Step 3: Run the strict positive case**

Use the real Hermes application tool-call ID in the binding, release the paused driver, and verify exact equality in `ebpf-event`, `security-event`, and `incident-event` with strict corroboration.

- [ ] **Step 4: Run the mismatch case**

Bind another real Hermes action with a different system tool-call ID and verify no strict corroborated incident is generated.

- [ ] **Step 5: Clean up and re-verify services**

Detach test bindings, stop test processes, remove mock credential directories, and confirm AgentSight, enforcer, LoongCollector, and SLS ingestion remain healthy.
