# ADR: AW interface freeze baseline

[中文](interface-freeze_zh.md) · [Schema reference](schema-reference.md)

Status: **proposed, awaiting interface review**. This change introduces an independent contract component and does not switch existing component execution paths. Acceptance establishes the stable protocol baseline; a branch or passing tests alone does not constitute acceptance.

## Problem

Native extension points have different powers. Some can await and replace tool results, others synchronously transform a text field, and others only observe events. Later extensions may still alter inspected arguments. A uniformly named hook does not create uniform authority.

Evidence also has different strengths: provider production, adapter return, local history and a final model request are distinct observations. A process identifier neither proves incarnation identity nor grants control. An external effect timing out must not be automatically retried as though it certainly failed.

Freeze capability semantics, authority boundaries, correlation and result meaning, rather than framework hook names or a component's private RPC.

## Decision and ownership

| Layer | Responsibility | Facts it produces |
| --- | --- | --- |
| Native adapter | Discover actual powers, map native identities, extract a text slot, preserve surrounding results, capture final observations | boundary, scope, adoption |
| AW Core | Select capabilities using policy and version negotiation; interpret results and request environment actions | invocation, validated candidate or decision |
| Provider Host | Map manifest-defined native protocols, enforce budgets, normalize success and failure | receipt and separate output |
| Component provider | Execute compression, inspection or another concrete algorithm | candidate, inspection, decision |
| Environment executor / original owner | Final dispatch, process lifecycle and state actions; authenticate current authority and generation | final result, runtime binding, operation state |
| Ledger | Persist and index facts, constrain duplicate writes, support reads and acknowledgements | traceable evidence records |

This component only provides schemas and pure validation between these layers. The Rust crate is a reference validator, not a requirement that adapters use Rust. No transport, such as in-process calls, stdio or HTTP, is mandated.

Capability contracts remain separate from native provider protocols. Components such as Tokenless retain their own requests, responses and manifests. A future Host explicitly converts those into AW capabilities; renaming fields does not establish integration. Core does not understand a compressor's command line, and a provider does not own an agent's conversation writes.

## Gaps addressed

| Gap | Resource / check | Accepted boundary |
| --- | --- | --- |
| Unclear extension authority | `boundary-descriptor/v1` | Explicit wait mode, mutation, final guard, proof location and ledger ordering |
| Process discovery confused with control | `runtime-binding/v1`, `control-grant/v1` | Native owner retains PID mapping; generation prevents reuse; owner grants control |
| Calls mixed across sessions | `common/v1` scope, `capability-invocation/v1` | Execution context separate from optional session; tool calls bind turn/tool IDs |
| Silent version mismatch | Schema references, `provider-descriptor/v1` | Exact ID and resource digest; unsupported revisions fail explicitly |
| Ambiguous recovery | Projection v2 | Mode, decoder or resolver, source digest and expiry; independent recovery before adoption |
| Candidates counted as savings | `provider-receipt/v1`, `context-adoption/v1` | Only verified, ledger-backed adoption yields attributable savings |
| Ambiguous scanner coverage | Content/code/command inspection v2 | Input/scanned bytes, completeness, rulesets and actual languages |
| Arguments changed after inspection | `execution-intent/v1` | Bind full arguments, target, cwd, environment, executor and OS policy |
| Missing capability order | `capability-plan/v1`, `plan-execution/v1` | Core pins steps, routes and failure rules; journal every terminal outcome |
| Hooks confused with system protection | `os-protection-binding/v1` | Independent authority supplies control coverage; missing requirements deny dispatch |
| Duplicate effects after timeout | `operation-record/v1` | Durable approval, idempotency and uncertainty reconciliation; no blind retry |

## Framework compatibility

Adapters implement capabilities of boundaries. Not every framework supports every feature.

| Native extension class | Integration | Limitation to preserve |
| --- | --- | --- |
| Awaitable tool wrapper | Inspect before dispatch, project after return, observe at final return | A location is not final if downstream code can still modify it |
| Synchronous transformation | Apply an already prepared replacement or perform a bounded synchronous call | Do not assume returned promises are awaited or asynchronous writes precede delivery |
| Event observer | Record observations with `observe_only` | No denial, replacement or `required_before_delivery` claim |
| Multiple result blocks | Extract one text-slot artifact | Preserve images, error status, structured fields and other blocks |
| Subagents / multiple sessions | Distinct execution contexts with parent links | An OS process is not a conversation identity |
| Model request proxy | Process independently at `before_model` or `proxy` | Visibility into a request cannot block an already executed tool |

AW preserves native events and messages while defining AW-owned capability ordering, decision composition and critical boundary constraints. Each adapter proves its declared powers and composition guarantees. Missing final observation produces `unverified`. Concrete plan and OS protection contracts follow below.

## Freeze and extension policy

After acceptance, resource bytes, IDs, field meanings, state transitions and AW JSON encoding are versioned together. Objects and enums are closed: even an optional field or enum addition can break readers and requires a new schema ID and explicit negotiation. Retain old resources unchanged.

Capability inputs and outputs use v2 so stricter recovery and coverage semantics do not rewrite experimental v1 contracts. New orchestration/evidence resources begin at v1. These numbers do not claim prior stable releases or an automatic upgrader. This change contains neither old v1 validators nor compatibility converters; existing callers keep their current paths.

A new capability adds its own name and input/output schemas without changing the invocation envelope. The reference validator only accepts four registered capabilities; advertising a new name does not authorize execution. Driver, lifecycle, ruleset and decoder/resolver identifiers must also be negotiated through trusted Host or adapter registries. Unknown identifiers fail; they are not dynamic loading paths.

Transport, SDK APIs, provider launch mechanics, scheduling, approval UI, ledger storage, credential formats, binary/multimodal projection, state capability payloads, recovery/OS policy enforcement implementations and token estimation algorithms are outside this freeze. The records retain necessary identities, versions and evidence relationships. In particular, `operation-record` is a generic transaction boundary, not a completed State Provider contract.

## Incremental migration and acceptance

This branch provides the implementation for step 0; interface review remains pending. Steps 1 through 6 require subsequent integration and acceptance.

| Step | Scope | Required verification |
| --- | --- | --- |
| 0: contracts | New component, schemas, semantic checks and documentation | Valid payload fixtures, rejected counterexamples, cross-language digest agreement, review acceptance |
| 1: entrypoint observation | Runtime identity and incarnation at the cosh-ng entrypoint | Exit, restart, external attachment and identifier reuse cannot mix old state or grant control |
| 2: read-only capability | One native provider through Host and Core negotiation | Correlated input/output/receipt; truthful failure, timeout and no-result behavior |
| 3: projection adoption | One adapter with replacement and observation powers | Preserve other blocks, recover source, observe final boundary; no false savings after no gain or later override |
| 4: execution safety | Ordered plans, final executor and independent OS protection | Denial cannot be overridden; input/policy changes invalidate old checks; hook-bypassing access remains constrained |
| 5: state operations | Separately reviewed payloads, approval and recovery | Idempotency, crash recovery, uncertainty, expired approval, changed generation and query reconciliation |
| 6: compatibility | Each adapter publishes its descriptor and tests | Awaitable, synchronous and observation-only classes; no authority transfer by analogy |

Native integration must inject process crashes and ledger failures. Contract tests cannot replace those checks. Review must confirm the first Rust API, time budget meanings, resource limits, recovery expiry and operation state machine before accepting stable semantics.

## Trade-offs

A bounded UTF-8 slot supports compression and inspection without importing every framework's message model into Core. Closed contracts expose drift but require negotiation for extension. Plain JSON and offline schemas enable multiple languages; cross-record invariants need semantic validators and counterexamples in addition to schema validation.

Observation-only hooks remain observation-only. Original process owners retain lifecycle authority. Future control integration must demonstrate authenticated authority and atomicity through native implementations.

## Execution ordering and native events

AW preserves each framework's native events and messages while defining capability plans, decision composition and critical ordering constraints. Core owns ordering within AW; adapters and environment executors enforce those constraints in the native runtime. A boundary event enters Core once; providers do not independently subscribe to native hooks and compete for execution order.

DSH Waterfall composes returning continuations, with the tool runtime explicitly awaiting the decision. Final guards and immutable result observation use separate extension points. This layering informs an adapter without requiring other frameworks to install the same event bus. [DSH extension guide](https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/cookbook/extension-cookbook.md)

`capability-plan/v1` contains ordered steps, preserving Core ownership of capabilities, provider selection and failure policy. It pins event_id, scope, revision, original source digest and resolved provider identities before calls start. Plugin load order cannot mutate an in-flight plan. Every step in this profile reads the same boundary_source: inspecting original text must not silently become inspecting a candidate. Projection occurs at most once, as the final step after source inspection. Arbitrary DAGs, candidate re-transformation, retries and dynamic step insertion are outside this profile.

Steps settle serially within one plan; different event IDs may run independently. All_distinct_providers invokes each selected implementation once; exactly_one selects at most one. An empty route is recorded as a gap without pretending a provider ran. Required means obtaining a usable result, not automatically assigning denial authority to any content finding. Content/code inspection supplies facts; command inspection gates dispatch in this profile.

```text
Native boundary event → Core pins plan
  → Ordered capabilities with linked invocations and receipts
  → Whole-plan terminal decision
  → Environment final guard → Dispatch / observed adoption
```

Not calling next does not necessarily deny: an allow return can skip later plugins. Mandatory command steps therefore cannot be optional or ignore failures. Any command warn/deny prevents direct dispatch; this profile has no implicit approval override and requires a separate explicit decision flow. Once denial, cancellation or source preservation is terminal, remaining steps are skipped. A later allow cannot overwrite it.

Observation gaps continue only when explicitly optional with record_gap_and_continue. Reject_plan prevents pre-tool dispatch or preserves the post-tool source. Missing projection output cannot become adoption. Proceed only admits the next native stage; it does not prove adoption, execution or OS permission for an actual system call.

`plan-execution/v1` accounts for every step, including gap, skipped and cancelled. Only actual calls reference provider receipts. Started/settled sequences come from one Core journal and require each step to settle before the next starts; cross-machine wall clocks do not establish order. Writers authenticate and persist journal ordering and allow the actual native dispatch claim for each event_id only once. Repeated successful pure validation does not authorize repeated execution.

## Independent OS system protection

Safety combines AW capability decisions, the environment's final input guard and ongoing OS access restrictions. Establish OS protection before untrusted execution, covering the policy's actual execution subjects and child scope. Protection does not depend on the agent entering an AW hook. AW allow cannot relax installed OS policy, and OS confinement cannot authorize a tool already denied by AW.

```text
                         Ongoing OS access restrictions
Native arguments → AW checks → Final intent check → Tool execution → Resource access
                                  ↑                       ↑
                         Check protection binding   Independent system enforcement
```

`capability-plan.os_requirement` fixes a policy digest and required versioned control IDs such as filesystem.access/v1. Execution-intent.protection_policy_digest binds that same policy to dispatch. A trusted system manager supplies os-protection-binding/v1 with target/incarnation, authority, state, control coverage, mechanism, freshness and independent evidence. Controllers authenticate its source against a preconfigured authority, never a provider's self-issued enforced assertion.

Validate_dispatch succeeds only when the whole plan permits, final intent is unchanged, binding is current, target/policy match and every required control is enforced. Missing protection, declared-only support, unsupported mandatory controls, stale generations and expired evidence reject admission. This execution profile does not silently degrade to no OS protection. Different assurance levels require separate policy review and explicit negotiation.

The contract names controls and policy identity without mandating Landlock, seccomp or another mechanism. Landlock restricts process access; seccomp filters system calls and is not a complete sandbox. Implementations verify actual kernel-supported coverage; a mechanism name cannot prove universal protection. [Landlock](https://docs.kernel.org/userspace-api/landlock.html), [seccomp](https://docs.kernel.org/userspace-api/seccomp_filter.html)

A trusted policy catalogue defines each control's exact access scope under policy_digest. Implementations examine threads/children, inherited privileges and handles, alternative execution routes and privilege separation between protection authority and untrusted plugins. A hook-owned boolean is not an OS backstop. Actual OS denial is an environment fact, not a compression provider verdict or a conclusion inferred solely from a tool error code.

This change provides interfaces, pure validators and synthetic contract tests only. It installs no kernel policy, starts no controller and demonstrates no real syscall denial. Subsequent live acceptance must cover forbidden access even after bypassing/short-circuiting hooks, child/alternate routes, missing controls or privileges not becoming active, changed policies invalidating old intents and target-correlated denial evidence. OS controls restrict configured system behaviors; they do not understand every business risk or replace AW semantic checks.
