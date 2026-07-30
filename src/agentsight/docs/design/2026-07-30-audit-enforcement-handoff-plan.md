# Audit-to-Enforcement Handoff Implementation Plan

## Objective

Implement the approved leased atomic replacement and durable audit restoration on top of the
reviewed containment stack. Keep protocol/backend behavior, persistence/orchestration, and the
small explicit-removal API adaptation in independently reviewable commits.

## Constraints

- Do not touch Security Observability files, routes, or tests.
- Preserve protocol version 3 behavior while advancing the wire contract to version 4.
- Replacement mutation must require the current required-violation subscription ID.
- Reuse the existing absolute UDS request deadline and containment readiness generation.
- Keep `public_ipv4`, stable PID/start/session identity, duration tri-state, redacted errors, and
  the exact five-attempt terminal boundary.
- Do not build the Linux-only AgentSight or ActPlane runtime on macOS.

## Phase 1: Protocol and backend

1. Add failing protocol tests for generic and credential replacement, invalid equal IDs,
   non-enforced sources, and stable-code-only outcomes.
2. Add a replacement module and leased protocol-v4 command/response.
3. Add failing mock/service tests for source, target, empty, third-party, stale subscription,
   disconnect rollback, and concurrent detach serialization.
4. Extend the backend trait and client. Service dispatch must reject unleased replacement and hold
   the required-subscription lock across replace and any lease-loss compensation.
5. Implement deterministic mock replacement.
6. Refactor ActPlane apply into preparation helpers and verify whether the pinned ABI can provide
   a no-gap kernel/map handoff.
7. Because it cannot, reject real replacement with `SourceRetained` before runtime mutation;
   remove snapshot/rebind behavior and test PID reuse and concurrent-fork retention.

Portable gate:

```bash
cargo test -p agentsight-enforcement-protocol
cargo test -p agentsight-enforcer --no-default-features --features mock-backend
cargo clippy -p agentsight-enforcement-protocol --all-targets -- -D warnings
cargo clippy -p agentsight-enforcer --no-default-features --features mock-backend --all-targets -- -D warnings
```

## Phase 2: Transition persistence and coordination

1. Add failing store tests for exact-key idempotency, conflicting reuse, atomic source/target/phase
   updates, source retention versus restoration, indeterminate state, domain preservation, and
   exhaustive enum decoding.
2. Add the transition model and SQLite module.
3. Add failing coordinator tests proving transitions run before ordinary replay and participating
   bindings are excluded from UUID reconciliation.
4. Add a leased replacement client boundary. Begin/resume must snapshot readiness, persist intent,
   call leased replace, recheck combined health, and commit only while the generation guard is
   held. Reconnect reconciliation acquires the adopted-subscription generation and applies the
   same fence immediately before persistence.
5. Treat remote required-subscription rejection as generation-fatal. Keep transport and
   indeterminate results retryable and expose degraded health.

Portable gate:

```bash
cargo test -p agentsight --lib enforcement::
cargo test -p agentsight --test security_store
cargo doc -p agentsight --no-deps
```

Linux CI additionally runs `cargo test -p agentsight --test enforcement_pipeline`.

## Phase 3: Containment handoff and restoration

1. Add `source_binding_id` as a nullable migration field, require it for new claims, and cover
   legacy-null fail-closed behavior.
2. Add failing forward lifecycle tests: source audit becomes detached, containment becomes
   enforced, the same case/evidence remains authoritative, and duplicate recovery does not create a
   second action, binding, or case.
3. Replace direct credential apply with forward transition begin/resume.
4. Add failing expiry tests for reverse restoration, restart after remote swap, stale generation,
   and the fifth terminal attempt.
5. Restore from the completed forward transition's immutable source request and structured source
   policy provenance. Treat source-retained reverse results as retryable attempts.
6. Add a store lookup for containment by binding and route enforcement DELETE through targeted
   reverse reconciliation. Never generic-detach a handoff-managed binding, including after the
   retry budget is exhausted; keep standalone detach unchanged for ordinary bindings.

Portable gate:

```bash
cargo test -p agentsight --test security_store
cargo fmt --all -- --check
cargo doc --workspace --no-deps
```

Linux CI additionally runs the complete `containment_pipeline`, `enforcement_pipeline`, and
`security_pipeline` targets.

## Final audits

Run the strongest host-safe focused suites, Rust formatting, focused Clippy and rustdoc, Dashboard
typecheck/embed only if Dashboard sources changed, shell and patch parsing, ordered ActPlane hash
attestation, `git diff --check`, byte comparison for Security Observability, commit subject/trailer
audit, forbidden-path scan, and clean-worktree check.
