# ACP Task Platform Acceptance Report

[中文版](acceptance-report_zh.md)

## Report identity

| Field | Value |
| --- | --- |
| Baseline | `e90d9d9402c7fa1c8122267eb4e075c0adda51f5` |
| Candidate | Uncommitted shared worktree based on the baseline; no distinct candidate SHA yet |
| Scope | Phase 0, Phase 1, and Phase 2 architecture readiness |
| Code changes assessed | Contracts, schema v9 Task/Run/Outbox/lease/input ledger, scheduler, Runtime ports, ungoverned ACP path, asynchronous approval/input, packaged containment, and local Gateway daemon/client |
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
| Neutral contracts and identities | Side-effect-free `cosh-gateway-contracts` freezes Gateway/Task schema v1 and independent Runtime schema v4, with versioned headers, bounded leaf/aggregate values, distinct ID newtypes, Task/Runtime events, and governance shapes | Complete compatibility manifest, ownership ADR acceptance, and remote identity authority |
| Task reducer | `TaskAggregate` plus the local `TaskCoordinator` serialize submit, read, event-page, cancel, retry, exact pending-input append, scheduler, and settlement paths; the 21-event by 9-state matrix verifies that rejection does not mutate the aggregate | Complete concurrent property/race and universal kill-point suites |
| SQLite Task store | Checksummed storage schema v9 uses WAL/FULL, installation binding, atomic Task/Outbox/governance/input ledgers, no-clobber backup/restore, redacted read-only inspection, `SQLITE_FULL` rollback, typed execution results, a crate-private release writer, 256 KiB per-payload and 1 MiB per-commit bounds, and real-`SIGKILL` local restart evidence | Complete power-loss suite, disk-health/operator runbook, quarantine, and filesystem race hardening |
| Runtime and private core transport | `RuntimeSupervisor`, private COSH JSONL, and provider-neutral Runtime ports provide bounded mapping, identity fences, cancellation, process settlement, and scheduler adaptation; the packaged systemd containment fixture passed on Ubuntu 24.04 arm64 with systemd 255 | Migration from Shell ownership and validation on other supported production environments |
| ACP v1 first slice | Rust 1.88 is the tested minimum and SDK 2.0.0 is pinned; ungoverned `doctor`/`run` profiles descriptor-pin the executable/workspace, and the Session Driver covers sequence/byte RAII and the ACP failure matrix | Signed/offline distribution, accepted real Codex/Claude conformance, manual Terminal validation, governed production admission, and an exact candidate commit |
| Capability | Neutral Capability/Permit/Execution contracts and ledger foundations remain available for a future optional capability profile; the production task-only inventory has no side-effecting ExecutionTarget | Production Broker/ExecutionTarget wiring, checkpoint/ws-ckpt integration, universal production gate, Shell/Skill/MCP/extension-tool coverage, and reconciliation evidence |

The candidate now implements a runnable local Gateway daemon/client. Production
`serve` admits only the contained brokered Core task-only profile; its immutable
inventory contains only `ask_user_question`, with no production
`ExecutionTarget` and no checkpoint/ws-ckpt dependency. ACP remains available
only through the explicitly ungoverned `doctor` and `run` paths. The Unix daemon
derives a stable local actor from peer UID, schedules Outbox work under
renewable fenced Run leases, persists `RuntimeBound` before prompting, and
converges unreconnectable post-restart Runtimes to `runtime_lost`. It also
exposes durable asynchronous provider-native approval whose Delivered replay
does not write to the provider twice. Durable Task/Run/Outbox/lease/input,
cancel, retry, and recovery behavior remains in scope. Generic
Capability/Permit/Execution contracts and ledger rows are future foundations,
not evidence of a production execution loop. Checkpoint/ws-ckpt support is a
follow-up optional capability. Shell attachment, remote/channel APIs, universal
Broker coverage, signed/offline Adapter distribution, accepted real
Codex/Claude evidence, manual Terminal validation, and an exact candidate
commit remain absent. Private COSH JSONL remains separate from ACP.
Runtime input requests and their exact pending identity are durable. Raw responses exist only in
the private typed dispatch ledger; Task events and receipts retain a digest.

## Module readiness summary

Each detailed report is authoritative for its module.

| Phase | Module | Candidate readiness | Report |
| --- | --- | --- | --- |
| 0 | Protocol contracts | `PARTIAL`; Gateway/Task schema v1, Runtime schema v4, and typed bounded contracts pass targeted checks, while the complete compatibility corpus and full ports remain | [Report](foundations/protocol-contracts/acceptance.md) |
| 0 | Identity and correlation | `PARTIAL`; distinct IDs/bindings exist, while authenticated/durable mapping and fences remain | [Report](foundations/identity-correlation/acceptance.md) |
| 0 | Storage and supervision | `PARTIAL`; storage schema v9, backup/restore, read-only inspection, `SQLITE_FULL`, recovery/fencing, local real-`SIGKILL`, and one-environment containment evidence exist, while complete power-loss, operator, and ownership-migration gates remain | [Report](foundations/storage-supervision/acceptance.md) |
| 1 | Gateway API | `PARTIAL`; authenticated local Unix submit/get/events/cancel/retry/append-input/resolve-approval and scheduled brokered Core execution exist, while remote identity and broader governed execution remain | [Report](task-plane/gateway-api/acceptance.md) |
| 1 | Task Execution Plane | `PARTIAL`; reducer, atomic store, Outbox worker, fenced leases, Runtime binding, cancellation, and fail-closed restart convergence exist; full platform execution and kill-point evidence remain | [Report](task-plane/task-execution-plane/acceptance.md) |
| 1 | Capability Broker | `PARTIAL`; generic contracts/ledger foundations exist, while production `serve` intentionally has no ExecutionTarget or checkpoint/ws-ckpt path | [Report](task-plane/capability-broker/acceptance.md) |
| 1 | CoshCore Bridge | `PARTIAL`; the contained Core profile is task-only with `ask_user_question`, while side-effecting tools and interactive Shell ownership remain outside it | [Report](task-plane/cosh-core-bridge/acceptance.md) |
| 1 | Local ACP Runtime MVP | `PARTIAL`; descriptor-pinned ungoverned profiles, fake conformance, and the failure matrix exist, while governed production admission, signed/offline distribution, real Codex/Claude, and manual Terminal gates remain unaccepted | [Report](task-plane/acp-mvp/acceptance.md) |
| 2 | ACP Client Bridge | `PARTIAL`; official v1 codec and supervised stdio slice pass focused tests, while domain/governance/recovery integration remains | [Report](adapters-and-presentation/acp-client-bridge/acceptance.md) |
| 2 | Shell Attachment | `NOT IMPLEMENTED`; direct Shell mode exists | [Report](adapters-and-presentation/shell-attachment/acceptance.md) |
| 2 | Web and Presentation | `NOT IMPLEMENTED` | [Report](adapters-and-presentation/web-presentation/acceptance.md) |

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

Current status: **NOT ACCEPTED; runnable local ACP slice, incomplete universal Gateway**.

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

The local daemon now schedules only the contained brokered Core task-only profile
through the neutral Runtime port, consumes Outbox rows under fenced leases,
persists Runtime binding before prompting, and fails closed after restart when
the process cannot be reconnected. Its production inventory contains only
`ask_user_question`; no production `ExecutionTarget` or checkpoint/ws-ckpt path
is wired. ACP `doctor` and `run` remain ungoverned interoperability paths.
Generic capability/permit/execution contracts and ledgers remain future
foundations, so no checkpoint approval/permit/audit/execute/result loop is
claimed. The packaged-unit containment fixture passed in a disposable Ubuntu
24.04 arm64 container with systemd 255. G1 remains rejected because this does
not govern side-effecting tools, Shell, Skill, MCP, extension tools, or legacy
mutation paths, and real Codex/Claude, manual Terminal, signed-artifact,
power-loss, and exact-candidate gates remain open.

### GM: local ACP Runtime MVP

Current status: **NOT ACCEPTED; installed and fake paths exist, external runtime proof is incomplete**.

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

The release build and fake conformance were exercised. Dirty-worktree
real-adapter and interactive observations are not accepted real Codex/Claude or
manual Terminal evidence. No ECS gate is claimed. Workspace package suites
passed under their canonical serialized
gates; the default parallel workspace run still exposed two timing-sensitive
shell-host assertions, and workspace-wide Clippy remains blocked by unrelated
pre-existing warnings.

### Recorded targeted implementation evidence

| Slice | Recorded command/result |
| --- | --- |
| Contracts | `cargo test --locked --package cosh-gateway-contracts`; package fmt, all-target Clippy, rustdoc, and dependency-tree checks. |
| Gateway library integration | `cargo test --locked --package cosh-gateway --lib`; all-target Clippy and package rustdoc. |
| Task reducer | The exhaustive 21-event by 9-state reducer matrix covers every current state/event combination and verifies that illegal transitions do not mutate the aggregate. |
| SQLite storage | The storage suite covers schema v9, WAL/FULL, `SQLITE_FULL` rollback, checksummed migrations, read-only inspection, source-bound no-clobber backup/restore, writer bounds, and local real-`SIGKILL` reopen/replay. |
| Runtime and ACP | The package suite covers private JSONL, ACP v1 codec/bridge, descriptor-pinned profiles, workspace inode digests, sequence/byte RAII, the ACP failure matrix, bounded supervision, and independent cancellation. |
| Capability | Generic Broker/Permit/Execution contract and ledger checks remain future-foundation evidence only; no production checkpoint adapter or ws-ckpt execution loop is claimed. |
| Systemd containment | `scripts/test-gateway-containment.sh` reported PASS in a disposable Ubuntu 24.04 arm64 container with systemd 255. It verified the rendered packaged unit, same-UID user-manager positive control, failed transient-unit escape, direct/grandchild/double-fork/`setsid` cleanup after Gateway `SIGKILL`, and replacement readiness only after the old cgroup emptied. |
| External runtimes | The release and fake harness paths were exercised. Dirty-worktree Codex/Claude and interactive observations are retained only as exploratory notes; real Codex/Claude conformance and manual Terminal validation remain unaccepted. |

### Planning-document evidence

| Check | Result |
| --- | --- |
| Per-module package | PASS: every module has English and Chinese `design` and `acceptance` documents |
| Repository documentation lint | PASS: `bash scripts/docs-lint.sh` |
| Repository link check | PASS: `python3 scripts/docs-link-check.py` |
| Complete owned-document link check | PASS: every relative link in the eight aggregate/developer-guide files resolves |
| Markdown hygiene | PASS: `git diff --check` and owned-file trailing-whitespace checks |
| Implementation-claim review | PASS: the task-only `ask_user_question` inventory, absent production ExecutionTarget, future checkpoint/ws-ckpt capability, and one recorded containment environment are separated from universal tool governance, accepted real adapters, manual Terminal, remote channels, and an exact candidate commit |

The recorded commands are scope-proportional package gates plus one destructive
containment run in the environment stated above. Real Codex/Claude, manual
Terminal, signed artifacts, power-loss, and ECS validation remain unaccepted. The worktree is still
uncommitted, so these results
cannot satisfy criteria that require one exact candidate commit.

## Acceptance owner and update rule

The architecture owner maintains this overall report. Module owners update
their detailed reports in the implementation pull request that produces the
evidence. A phase is accepted only when every module report reaches its exit
criteria and this report records the exact aggregate candidate commit.
