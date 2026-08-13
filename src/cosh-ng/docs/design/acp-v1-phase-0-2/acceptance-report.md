# Overall Acceptance Report

[中文版](acceptance-report_zh.md)

## Report identity

| Field | Value |
| --- | --- |
| Baseline | `6c115aefe04ace0d169a24fa7cd55ad7c1befa52` (`up/main`, 2026-08-12 fetch) |
| Candidate | Uncommitted shared worktree based on the baseline; no distinct candidate SHA yet |
| Scope | Phase 0, Phase 1, and Phase 2 architecture readiness |
| Code changes assessed | Contracts, Task reducer/SQLite store, Runtime/private JSONL, ACP v1 bridge/profile slices, and partial Capability slices |
| Overall implementation status | **NOT ACCEPTED** |
| Document integration status | **PASS** after the checks recorded below; not a phase gate |

## Status vocabulary

| Status | Meaning |
| --- | --- |
| `PASS` | Candidate-commit evidence satisfies the stated criterion |
| `PARTIAL` | A bounded source/test slice exists, but the module exit criteria or integration path remain incomplete |
| `FAIL` | Implemented behavior was exercised and violated the criterion |
| `NOT IMPLEMENTED` | Required production surface does not exist on the assessed commit |
| `BLOCKED` | The surface exists, but the required environment or prior decision prevents a valid test |
| `NOT RUN` | A test was applicable but was not requested or executed |

`NOT IMPLEMENTED` is not softened to `BLOCKED`. A completed design is not
runtime evidence. A `PARTIAL` library slice is not a production capability.

## Baseline findings

The baseline already supplies useful implementation foundations:

- five Rust crates with explicit dependency direction;
- a standalone `cosh-shell` that owns PTY and Agent child lifecycle;
- an exact-version internal cosh-core JSONL initialization contract;
- streamed Agent events, approvals, questions, cancellation, session recovery,
  audit identity, and bounded evidence patterns;
- workspace-scoped model conversation persistence;
- typed package, service, checkpoint, and audit operations.

The baseline source and workspace manifests contain no production Gateway
daemon, Task aggregate/store/event store, execution lease, Outbox, Capability
Broker, ACP client dependency or implementation, Web attachment API, or
channel adapter. All Phase 1 and Phase 2 product gates therefore start at
`NOT IMPLEMENTED` even where an existing component can be adapted.

## Candidate-worktree findings

The current worktree adds implementation foundations that are absent from the
pinned baseline:

| Slice | Implemented evidence | Still missing for acceptance |
| --- | --- | --- |
| Neutral contracts and identities | Side-effect-free `cosh-gateway-contracts` with versioned headers, bounded leaf strings/digests/errors, distinct ID newtypes, Task/Runtime events, Capability/Approval/Permit shapes, and serde validation | Aggregate collection/envelope admission limits, canonical schema/golden corpus, complete compatibility manifest, ownership ADR acceptance, authenticated identity resolver, and durable parent/fence enforcement |
| Task reducer | `TaskAggregate` validates schema/correlation, consecutive revisions, active Run, approval/execution lifecycle, cancellation, retry, and terminal transitions without partial mutation | `TaskCoordinator`, command routing, runner leases, durable input, event-admission fencing, complete transition/property/race suite, and restart orchestration |
| SQLite Task store | Local single-writer SQLite schema v1 with WAL/FULL policy, checked migrations, Task projection/event/receipt/Outbox transaction, revision/idempotency checks, and path/symlink/permission validation | Backup/restore, migration artifacts, disk-full/corruption/kill-point suites, Outbox worker/lease recovery, daemon restart reconciliation, and full companion-file race hardening |
| Runtime and private core transport | `RuntimeSupervisor` validates direct launches, clears inherited environment, bounds stdout/stderr, owns process-group shutdown/reap, and emits one process terminal; private COSH JSONL codec negotiates exact v1 and produces typed local observations | Integrated `CoshCoreBridge`, public contract event mapping/fencing/backpressure, restart/deadline policy, provider-session binding, complete descendant/race fixtures, and migration from Shell ownership |
| ACP v1 first slice | Official SDK 2.0.0 is pinned with Rust 1.88; codec/bridge negotiate wire v1; a bounded driver provides independent cancellation; built-in profiles resolve installed adapters with canonical paths and allowlisted environments | Installed COSH entrypoint, local permission UI/evidence, durable governance/mapping, restart/resume, broader conformance, and real-adapter evidence |
| Capability | Neutral contracts plus a package-exposed Broker and mutex-atomic in-memory single-use permit store; targeted tests cover parent, expiry, target, operation, policy, execution binding, and concurrent claims | Durable/audited permit ledger, immutable target resolver, execution target/verifier, reconciliation, and closure of legacy CLI/core/Shell bypasses |

No candidate code implements a Gateway daemon, authenticated Unix/network
API, installed ACP entrypoint, Shell attachment, Web/API presentation,
or channel adapter. The private COSH JSONL v1 codec remains separate from the
new ACP v1 bridge.

## Module readiness summary

Each detailed report is authoritative for its module.

| Phase | Module | Candidate readiness | Report |
| --- | --- | --- | --- |
| 0 | Protocol contracts | `PARTIAL`; typed leaf contracts pass targeted checks, while frozen schemas/fixtures and full ports remain | [Report](phase-0/protocol-contracts/acceptance.md) |
| 0 | Identity and correlation | `PARTIAL`; distinct IDs/bindings exist, while authenticated/durable mapping and fences remain | [Report](phase-0/identity-correlation/acceptance.md) |
| 0 | Storage and supervision | `PARTIAL`; SQLite/store and supervisor foundations exist, while recovery, fencing, process-tree, and ownership migration remain | [Report](phase-0/storage-supervision/acceptance.md) |
| 1 | Gateway API | `NOT IMPLEMENTED` | [Report](phase-1/gateway-api/acceptance.md) |
| 1 | Task Execution Plane | `PARTIAL`; reducer and atomic local store exist, but coordinator/leases/restart path do not | [Report](phase-1/task-execution-plane/acceptance.md) |
| 1 | Capability Broker | `PARTIAL`; package-exposed in-memory slice passes targeted tests, but is not a universal production gate | [Report](phase-1/capability-broker/acceptance.md) |
| 1 | CoshCore Bridge | `PARTIAL`; supervisor/private codec exist, but the bridge/public mapping does not | [Report](phase-1/cosh-core-bridge/acceptance.md) |
| 1 | Local ACP Runtime MVP | `PARTIAL`; codec, bridge, bounded driver, independent cancel, fake-Agent tests, and fixed profiles exist, but the installed entrypoint, permission UI/evidence, and real-adapter proof do not | [Report](phase-1/acp-mvp/acceptance.md) |
| 2 | ACP Client Bridge | `PARTIAL`; official v1 codec and supervised stdio slice pass focused tests, while domain/governance/recovery integration remains | [Report](phase-2/acp-client-bridge/acceptance.md) |
| 2 | Shell Attachment | `NOT IMPLEMENTED`; direct Shell mode exists | [Report](phase-2/shell-attachment/acceptance.md) |
| 2 | Web and Presentation | `NOT IMPLEMENTED` | [Report](phase-2/web-presentation/acceptance.md) |

## Phase gate report

### G0: contract freeze

Current status: **NOT ACCEPTED**.

Exit requires all of the following:

- canonical v1 schemas for ingress, identity, Task commands/events, approval,
  capability, permits, execution, Runtime events, presentation, delivery, and
  error envelopes;
- machine-readable fixtures with backward/forward compatibility tests;
- explicit ID generation, authority, correlation, and redaction invariants;
- accepted persistence ADR, migration policy, and backup/recovery contract;
- accepted process-supervision ADR with one owner per child process;
- ACP v1 feasibility fixture proving SDK and wire-version separation, with
  official SDK 2.0.0 and Rust 1.88 recorded independently from stable wire v1;
- dependency and crate ownership decision that preserves the existing Shell
  boundary or records its deliberate replacement.

No Phase 1 production API may freeze its own duplicate contract before G0.

The candidate types, SQLite schema, supervision primitives, and ACP feasibility
slice reduce G0 implementation risk, but missing canonical fixtures, ADR
sign-off, identity admission, and recovery artifacts keep G0 rejected.

### G1: local durable Gateway

Current status: **NOT ACCEPTED; partial library foundations only**.

Exit requires:

- local authenticated Unix-socket API and idempotent task submission;
- durable Task command/event/snapshot behavior across process restart;
- atomic Task event and Outbox append;
- renewable runner leases and explicit uncertain-side-effect handling;
- a universal Capability Broker with target-bound, expiring, single-operation
  permits;
- deterministic typed execution through platform operators;
- cosh-core lifecycle accessed only through `AgentRuntimePort`;
- cancellation, approval race, crash recovery, and audit-correlation tests;
- no direct OS execution from handlers, presenters, or Agent bridges.

The reducer/store/supervisor slices do not provide a daemon, API,
TaskCoordinator, runner lease/recovery loop, integrated Runtime Port, or a
universal production Capability gate. They therefore do not satisfy G1.

### GM: local ACP Runtime MVP

Current status: **NOT ACCEPTED; partial library foundations only**.

Exit requires one installed COSH entrypoint to run exactly one canonical
workspace, ACP connection/session, and active bounded text prompt through an
installed `codex-acp` or `claude-agent-acp`. A session driver must keep cancel
independent of a silent or blocked stdout reader, transport failures must fail
closed, and the local permission proxy must expose only correlated
`allow_once` and `reject_once` decisions. At least one real adapter must pass
initialize, multi-chunk prompt, terminal result, independent cancel, allow
once, and reject once on the same candidate revision.

Native Codex/Claude ACP support, `npx` or other package runners, network
bootstrap, filesystem/terminal callbacks, load/resume, Web, and the Gateway
daemon are outside this MVP and cannot be used to satisfy it.

### G2: ACP and interactive attachments

Current status: **NOT ACCEPTED; first ACP library slice only**.

Exit requires:

- ACP v1 initialization and capability negotiation over local stdio;
- baseline ACP session and streaming behavior mapped to Runtime types;
- ACP permission, filesystem, and terminal requests routed through durable
  approval and Capability Broker paths;
- incompatible protocol, missing capability, malformed stdout, child exit,
  cancellation, and session recovery conformance cases;
- Shell attach/detach/replay while preserving PTY ownership and direct mode;
- Web/API cursored replay, approval, cancellation, and safe output views;
- Outbox retry and stable delivery receipt semantics;
- proof that Task, Run, ACP session, Shell session, request, tool, and execution
  identities remain distinct.

## Required evidence package for implementation acceptance

Every module implementation report must include:

1. candidate branch and full commit SHA;
2. reviewed requirement rows and source links;
3. exact commands, environment, test count, and results;
4. versioned fixtures or captured sanitized protocol transcripts;
5. negative and race/failure cases, not only success paths;
6. any untested provider, ECS, platform, or manual UI paths;
7. rollback or compatibility result;
8. reviewer sign-off for security- or wire-contract decisions.

Evidence must not contain credentials, raw prompts, private terminal output,
host identifiers, or unrestricted environment values.

## Cross-module acceptance scenarios

These scenarios cannot be closed by a single unit test.

| Scenario | Expected evidence |
| --- | --- |
| Duplicate DingTalk/Web/CLI submission | One Task state effect and the same returned `TaskId` |
| Gateway crash after event commit | Task and Outbox recover without duplicating the side effect |
| Runner lease expires during an OS write | Execution becomes uncertain or reconciled; it is not blindly replayed |
| Two approval callbacks race | One terminal decision wins and both callers receive the committed state |
| cosh-core exits during a turn | One terminal Runtime event and deterministic Task suspension/failure |
| ACP Agent requests terminal execution | Broker decision and permit precede target execution; full IDs reach audit |
| Shell detaches during approval | Task remains waiting; another authorized client can resolve it without owning the PTY |
| Web delivery is unavailable | Task continues according to state; Outbox retries delivery independently |
| Provider network becomes unavailable | Explicit suspend or configured local fallback without policy downgrade |
| Gateway restarts with active attachments | Clients replay from cursors; no in-memory UI state is treated as durable truth |

## Scope-proportional candidate validation

Implementation owners and the integration owner ran targeted package checks
for the Rust slices present in the shared worktree. The documentation
integration ran the corresponding bilingual and repository-document checks:

- inspect bilingual file pairing and semantic parity;
- validate relative Markdown links;
- run `git diff --check`;
- check that commands and implementation claims agree with baseline and
  candidate source;
- preserve exact commands and results without promoting package evidence to a
  full-system gate.

No full workspace test, workspace-wide Clippy, release build, ECS, provider,
or manual Terminal gate is claimed.

### Recorded targeted implementation evidence

| Slice | Recorded command/result |
| --- | --- |
| Contracts | `cargo test --locked --package cosh-gateway-contracts`: 6 integration tests passed; unit/doc-test targets passed. Package fmt, all-target Clippy, rustdoc, and dependency-tree checks also passed. |
| Gateway library integration | `cargo +1.88.0 test --locked --package cosh-gateway --no-fail-fast`: 84 passed, 0 failed. This is one package suite, not a workspace/full-system gate. |
| Task reducer | The aggregate suite includes 15 focused transition tests, including unresolved and uncertain execution guards. |
| SQLite storage | The storage suite includes 15 focused tests, including normal load/commit snapshot replay verification. |
| Runtime and ACP | The package suite covers private JSONL, ACP v1 codec/bridge, fixed profiles, bounded I/O/supervision, and the independently cancellable session driver. Gateway all-target Clippy and package rustdoc passed. |
| Capability | `cargo +1.88.0 test --locked --package cosh-gateway capability --no-fail-fast`: 12 passed, 0 failed. This validates the in-memory decision/permit slice only. |

### Planning-document evidence

| Check | Result |
| --- | --- |
| Per-module package | PASS: every module has English and Chinese `design` and `acceptance` documents |
| Repository documentation lint | PASS: `bash scripts/docs-lint.sh` |
| Repository link check | PASS: `python3 scripts/docs-link-check.py` |
| Complete owned-document link check | PASS: every relative link in the eight aggregate/developer-guide files resolves |
| Markdown hygiene | PASS: `git diff --check` and owned-file trailing-whitespace checks |
| Implementation-claim review | PASS: baseline and candidate claims are separated; the ACP codec/bridge/profile/driver foundation is distinguished from the missing installed entrypoint, production governance, and real-adapter evidence |

The recorded code results are scope-proportional package gates, not full
workspace or live-system validation. ECS validation, provider calls, and
manual Terminal UX were not run.

## Acceptance owner and update rule

The architecture owner maintains this overall report. Module owners update
their detailed reports in the implementation pull request that produces the
evidence. A phase is accepted only when every module report reaches its exit
criteria and this report records the exact aggregate candidate commit.
