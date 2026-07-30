# ActPlane Stale Violation Drain Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent stale records in a reused ActPlane pinned ring buffer from blocking violations for newly applied AgentSight bindings.

**Architecture:** Extend the reviewed ActPlane compatibility patch with a bounded-by-buffer drain operation. Invoke it after runtime-state cleanup and before the enforcer starts its live poller, so readiness implies a clean event boundary.

**Tech Stack:** Rust 2024, Aya ring buffers, ActPlane pinned eBPF runtime, systemd, AgentSight UDS and Actix APIs.

## Global Constraints

- Linux-only implementation and verification; do not build AgentSight on macOS.
- Do not modify ActPlane BPF programs; the change is userspace loader behavior.
- Keep all production code and comments in English.
- Preserve the source-attested upstream revision and reviewed patch queue.
- Do not stage `.superpowers/` or `oslevel-harness/`.

---

### Task 1: Specify startup preparation ordering

**Files:**
- Modify: `src/agentsight/crates/agentsight-enforcer/src/actplane.rs`

**Interfaces:**
- Produces: `prepare_runtime(clear, drain) -> Result<usize, BackendError>` used by `ActPlaneBackend::open`.

- [ ] Add a unit test that records operations and expects `clear` before `drain`.
- [ ] Run the focused test and confirm it fails because `prepare_runtime` is absent.
- [ ] Add a second test proving a cleanup error prevents the drain closure from running.
- [ ] Implement the minimal generic orchestration helper and rerun the focused tests.

### Task 2: Add the pinned event drain API

**Files:**
- Create: `src/agentsight/patches/actplane/0004-drain-stale-pinned-events.patch`
- Modify: `src/agentsight/scripts/build-enforcer.sh`
- Modify: `src/agentsight/crates/agentsight-enforcer/src/actplane.rs`

**Interfaces:**
- Produces: `PinnedEngine::drain_pending_events(&self) -> io::Result<usize>`.
- Consumes: `prepare_runtime` from Task 1.

- [ ] Add a compile-time API assertion to the enforcer test module and confirm the build fails because the method is absent.
- [ ] Add the ActPlane patch implementing a non-blocking loop over all currently queued ring-buffer records.
- [ ] Call `prepare_runtime` from `ActPlaneBackend::open`, mapping cleanup and drain failures to distinct kernel-error contexts.
- [ ] Add patch `0004` to the ordered patch list and update the expected patched blob hash.
- [ ] Build and run focused enforcer tests on Linux; confirm both startup-order tests and the API assertion pass.
- [ ] Commit only the implementation, tests, patch, build attestation, and approved design documents.

### Task 3: Verify and deploy the final closure

**Files:**
- No additional source files expected.

**Interfaces:**
- Consumes: final AgentSight and enforcer release binaries from Task 2.

- [ ] Run `cargo fmt --all -- --check`.
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] Run `cargo test --workspace` and `cargo doc --workspace --no-deps`.
- [ ] Build the embedded Dashboard and both release binaries on Linux.
- [ ] Back up deployed binaries/configuration, deploy exact verified artifacts, and wait for `/api/enforcement/health` to report `ready=true` with backend `actplane`.
- [ ] Execute the dual-subscriber real-file test and require baseline `READ_OK`, enforced `EPERM`, matching UDS/API event IDs, a 2026 epoch timestamp, detach HTTP 204, restored `READ_OK`, and zero active test bindings.
- [ ] Verify `/`, `/enforcement`, and the risk-interception Dashboard route render from the embedded artifact.
- [ ] If any closure condition fails, restore the prior stable binaries and leave both services healthy.
