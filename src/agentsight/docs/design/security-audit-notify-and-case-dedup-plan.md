# Security Audit Notify and Case Deduplication Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Emit real observe/audit credential-exfiltration events and group DNS retry destinations into one risk case.

**Architecture:** Extend the existing lightweight pinned ActPlane profile to attach only `sys_enter_connect` in addition to its LSM hooks. Build a stable burst correlation key in the existing security coordinator and append immutable evidence in the existing store.

**Tech Stack:** Rust, SQLite/rusqlite, ActPlane eBPF loader patches, Cargo integration tests, Linux BPF LSM.

## Global Constraints

- Linux 5.8+ compatibility is required.
- No new runtime dependency on AgentSecCore.
- The ActPlane revision stays pinned to `a62e5d9d96f91101cda019519053e950d532380a`.
- Production code is added only after a focused test fails for the expected reason.
- BPF changes are verified on a real Linux kernel.

---

### Task 1: Group DNS retries into one risk case

**Files:**
- Modify: `src/agentsight/tests/security_pipeline.rs`
- Modify: `src/agentsight/src/security/coordinator.rs`
- Modify: `src/agentsight/src/security/store.rs`

**Interfaces:**
- Consumes: `SecurityCoordinator::ingest(SecurityEvent)` and `SecurityStore::upsert_case(&RiskCase, &[Uuid])`.
- Produces: a stable burst correlation key and append-only ordered evidence links.

- [ ] Add an integration test that ingests two audit chains with different destination IPs in the same five-second burst and expects one case with eight evidence events.
- [ ] Run the focused integration test and confirm it fails with two risk cases.
- [ ] Replace event-ID correlation with binding, policy, source resource, process identity, and five-second bucket correlation.
- [ ] Append evidence positions after existing links when a case already exists.
- [ ] Run the security pipeline and store tests and confirm they pass.
- [ ] Commit the independently testable change.

### Task 2: Enable the real notify connect path

**Files:**
- Create: `src/agentsight/patches/actplane/0010-enable-credential-notify-connect.patch`
- Modify: `src/agentsight/scripts/build-enforcer.sh`

**Interfaces:**
- Consumes: ActPlane `tracepoint_needed(&TracepointSpec, HookBudget)` and the `credential-exfiltration` pinned profile.
- Produces: a profile that attaches `trace_connect` while continuing to suppress every other non-core dataflow tracepoint.

- [ ] Add a patch-queue test asserting `trace_connect` is needed and `trace_connect_exit` plus file tracepoints are not needed.
- [ ] Apply the test patch to a clean pinned ActPlane checkout and confirm the test fails.
- [ ] Add the minimal profile-specific `trace_connect` selection.
- [ ] Update every patch-queue attestation state and final blob hash.
- [ ] Run the focused ActPlane library tests and confirm they pass.
- [ ] Commit the independently testable change.

### Task 3: Build, deploy, and verify both modes

**Files:**
- Modify only generated build artifacts and remote service binaries; do not commit artifacts.

**Interfaces:**
- Consumes: `agentsight-enforcer`, `agentsight serve`, the Dashboard enforcement API, and Hermes.
- Produces: persisted audit and enforce cases visible in System Audit.

- [ ] Run formatting, focused Clippy, Rust workspace tests, frontend typecheck, page tests, and embedded build.
- [ ] Build the release enforcer and AgentSight server on the Linux instance.
- [ ] Restart services and verify the enforcement health endpoint reports `ready=true`.
- [ ] Apply an audit policy to Hermes, read the sensitive file, connect externally, and verify the connection succeeds while one unblocked case is stored.
- [ ] Apply an enforce policy to Hermes, repeat the operation, and verify the kernel returns `EPERM` while one blocked critical case is stored.
- [ ] Verify DNS retries remain one case and both Dashboard routes render the evidence.
- [ ] Commit any verification-driven fixes and leave services healthy with test bindings detached.
