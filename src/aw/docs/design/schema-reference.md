# AW schema reference and rationale

[中文](schema-reference_zh.md) · [Freeze ADR](interface-freeze.md) · [All schemas](../../schemas/)

This reference explains the responsibilities, field groups, trade-offs and validation boundaries of all 21 resources. Linked JSON Schemas define exact types, required fields and enums. [contracts.json](../../tests/fixtures/contracts.json) contains synthetic conformance examples, not provider execution evidence.

“Freeze” means acceptance after review. JSON Schema checks structure; [validation.rs](../../src/validation.rs) and [orchestration.rs](../../src/orchestration.rs) check relationships between records; native integration establishes authentic facts. None replaces the others.

## 1. common/v1: shared value domains

File: [common-v1.schema.json](../../schemas/common-v1.schema.json). This resource only contains `$defs`; it is not a business message or another universal envelope.

| Group | Meaning and rationale |
| --- | --- |
| `name`, `rule` | Bounded printable ASCII identifiers, with a narrower rule alphabet. Names resolve through trusted registries, not as paths, commands or download URLs |
| `uint`, `digest` | Safe integers and lowercase SHA-256 hex. Digests bind bytes; they are neither signatures nor authentication |
| `plan_ref.plan_id/revision/digest/step_id` | Bind one Core plan and step across invocation and receipt; providers do not choose execution order |
| `schema_ref.id/digest` | Bind both protocol identity and exact schema resource bytes, detecting different contracts under one name |
| `scope.environment_id/execution_context_id/actor_id` | Separate environment, execution context and actor. A session ID cannot stand in for all three |
| `scope.runtime_id/runtime_generation/binding_revision` | Bind runtime incarnation and observation binding revision; old records cannot control a new instance |
| `scope.session_id/turn_id/tool_use_id/parent_context_id` | Map available native identities. Tool boundaries require turn/tool IDs; subagents retain parent context links |
| `artifact.id/digest/content/media_type/origin/tool_name` | One UTF-8 text slot. Hash original text bytes; origin describes provenance, not trust, and tool_name provides context |
| `finding.rule_id/category/severity/confidence/count` | Separate rule, category, severity, confidence and occurrence count. Count is not scanned bytes |
| `coverage.input_digest/input_bytes/scanned_bytes/complete/ruleset_ids/languages` | Bind inspected text and declared coverage, including rule versions and languages; input length alone cannot establish complete scanning |
| `evidence.source_id/record_id/digest` | Reference an independent evidence source and record without embedding its body in a receipt. Readers authenticate the source and verify record bytes |
| `meter.meter_id/unit/measurement_kind/method/value` | Separate metric identity, unit, estimate/observed/billed nature, versioned method and value. Fewer bytes do not prove lower token billing |

`boundary` locates invocation; `proof_boundary` locates observation. `post_tool` is not synonymous with `final_tool_result`. `local_history` does not imply `model_request` or remote consumption.

AW JSON documents are limited to 4 MiB and depth 32. Object keys are ASCII and numbers are integers with absolute value at most 2^53−1. Raw text and serialized native arguments remain strings, preserving floats, non-ASCII native keys and original formatting. These limits define this inline text baseline; larger/binary resources need a new artifact profile, not silent truncation.

## 2. context.projection.prepare input v2

File: [context-projection-prepare-input-v2.schema.json](../../schemas/context-projection-prepare-input-v2.schema.json). Produced by adapter/Core and consumed by Host/provider.

`artifact` fixes the text slot and original digest; `boundary` locates the call. `constraints.allow_text_reencoding` explicitly permits conversion between supported text media types, without authorizing changes to surrounding structure. `constraints.accepted_reversibility` lists accepted `lossless`, `retrievable` and `unrecoverable` modes and must be a subset of adapter support.

Avoid normalizing an entire native result into a string: preserve error status, stderr, images and other blocks. Core supplies policy explicitly; a provider cannot weaken recovery requirements for a better compression ratio. An unrecoverable summary requires explicit acceptance.

Semantic checks bind exact source bytes, the full input document digest and the invocation input budget. The adapter must separately prove that this slot belongs to the native tool call identified by scope.

## 3. context.projection.prepare output v2

File: [context-projection-prepare-output-v2.schema.json](../../schemas/context-projection-prepare-output-v2.schema.json). The output contains `candidate`, not `adopted` or an aggregatable savings claim.

| Field | Meaning |
| --- | --- |
| `source_artifact_id/source_digest` | Bind the exact source and prevent cross-call replacement |
| `content/media_type` | Prepared representation within the explicitly permitted conversion domain |
| `transform_chain` | Ordered, versioned transformation identifiers, not arbitrary executable paths |
| `reversibility` | Recovery class satisfying the caller's requirements |
| `recovery.mode=self_contained/decoder_id` | Independently decodable candidate using a negotiated decoder version |
| `recovery.mode=external/resolver_id/reference/source_digest/expires_at_ms` | Resolver and opaque recovery reference, original digest and expiry |
| `recovery.mode=none` | Only valid with explicitly accepted `unrecoverable` |

A provider's lossless declaration is not proof. Before adoption, a trusted caller independently decodes or resolves the candidate and verifies exact original bytes. This library accepts that recovered text; it does not run decoders or resolvers. External references must be valid at adoption; subsequent retention belongs to the resolver contract, with no promise of perpetual recovery here.

An empty candidate may be represented as provider output but cannot be marked adopted, preventing accidental clearing of a result slot. The environment can preserve a source when there is no byte gain. Actually adopting a longer candidate cannot produce negative or falsely positive savings.

## 4. security.content.inspect input v2

File: [security-content-inspect-input-v2.schema.json](../../schemas/security-content-inspect-input-v2.schema.json). Fields are `artifact`, `boundary` and `constraints.include_low_confidence`.

This capability detects content risks; it neither guesses an execution language nor automatically blocks execution. `include_low_confidence` requests a reporting policy, not permission to skip digest or completeness checks. Pre/post-tool boundaries require support from both adapter and provider.

Source text stays immutable. Redaction, notification and denial are subsequent policy decisions rather than implied effects of inspection. The reference validator does not rerun the algorithm or decide which rules ought to match.

## 5. security.content.inspect output v2

File: [security-content-inspect-output-v2.schema.json](../../schemas/security-content-inspect-output-v2.schema.json). `inspection` contains `verdict`, `findings` and `coverage`.

`clean` requires no findings and declared complete coverage; `suspicious` and `sensitive` require at least one finding. Input digest and bytes match the source. Scanned bytes cannot exceed input bytes, and complete is true exactly when the lengths agree. Content inspection reports an empty languages array and nonempty ruleset_ids.

This detects the contradiction of partial scanning with a clean claim. It does not prove that every byte was actually examined. Partial scanning can report discovered risks but cannot report clean. Rule correctness and authentic coverage still need component tests.

## 6. security.code.inspect input v2

File: [security-code-inspect-input-v2.schema.json](../../schemas/security-code-inspect-input-v2.schema.json). Fields are `artifact`, `boundary` and `constraints.language`.

Supported values are `bash`, `python` and `auto`. Here auto explicitly enables both bash and python scanning; it does not guess one language. Unknown language detection must not silently skip scanning and yield clean. Adding languages requires a new revision or capability profile; this version does not default unknown languages to support.

Code inspection alone does not bind the final native execution arguments. Dispatch control requires command inspection plus an execution intent.

## 7. security.code.inspect output v2

File: [security-code-inspect-output-v2.schema.json](../../schemas/security-code-inspect-output-v2.schema.json). The inspection verdict, findings and coverage share the content-inspection shape.

`coverage.languages` must exactly match the requested language, or both languages for auto. Order is irrelevant and duplicates are forbidden. An ambiguous single “detected language” field is insufficient. Scanned bytes measure text coverage, not the sum of visits by multiple scanners; two scanners do not double input size.

Finding counts do not establish byte coverage. Independent findings may be reported by both scanners, but their number cannot imply complete. Provider compatibility tests must demonstrate that the algorithms actually run.

## 8. security.command.inspect input v2

File: [security-command-inspect-input-v2.schema.json](../../schemas/security-command-inspect-input-v2.schema.json). Its boundary is fixed to `pre_tool`.

`command.content/digest/language/tool_name` describe command text extracted from native arguments. `execution_intent_digest` binds the entire execution context. The two digests cover different objects: scanned text versus complete arguments and target.

Binding only a command string leaves cwd, environment, executor and target changes unguarded. Binding an intent still cannot prove the extractor inspected the correct argument field. The adapter must accurately extract the command from those arguments and capture the current intent at final dispatch. That native mapping requires separate tests.

## 9. security.command.inspect output v2

File: [security-command-inspect-output-v2.schema.json](../../schemas/security-command-inspect-output-v2.schema.json). `decision` contains verdict, findings, coverage, reasons and the echoed execution_intent_digest.

`allow` requires complete coverage, no findings and no reasons. `warn`/`deny` require nonempty findings and reasons; all decisions in this profile require complete coverage. Incomplete scanning produces a failure receipt, never a manufactured allow. Actual language coverage matches the request.

This decision is an inspection result, not a transferable execution permit. Validate_execution_gate checks one result; complete dispatch uses validate_dispatch to check all plan steps and independent OS protection. `validate_execution_gate` only accepts allow, an unchanged unexpired intent and a scope matching the invocation. Warn requires a separate explicit policy/approval path; it is not automatically allowed. The executor must compare and dispatch within one controlled boundary, preventing later mutation.

## 10. boundary-descriptor/v1: native powers

File: [boundary-descriptor-v1.schema.json](../../schemas/boundary-descriptor-v1.schema.json). Published by the adapter and consumed through trusted Core configuration.

`adapter_id/adapter_version/boundary_id/revision` bind implementation and boundary version. `boundary` locates the event; `invocation_mode` distinguishes awaitable, synchronous and observe_only. `can_replace_text`, `can_deny_dispatch` and `has_final_input_guard` independently declare replacement, denial and final input guarding.

`proof_boundaries` lists genuinely observable locations. `ledger_policy` is required_before_delivery or best_effort; the former requires an observable delivery boundary and cannot be promised by an observer. `media_types` and `reversibility` declare supported text handling.

`composition.input_finality` distinguishes immutable_until_dispatch, revalidate_at_dispatch and uncontrolled; a final guard cannot bind uncontrolled input. Composition.gate is required_final_guard or none and must agree with has_final_input_guard. Composition.result_finality distinguishes final from subject_to_later_change; final_tool_result proof requires final. Native ordering tests must substantiate these declarations.

Validation rejects observers claiming mutation, denial or delivery ordering, and denial without a final guard. A framework name does not prove a descriptor truthful. Each adapter needs native ordering, later-override, cancellation and exception-path tests.

## 11. runtime-binding/v1: runtime observation

File: [runtime-binding-v1.schema.json](../../schemas/runtime-binding-v1.schema.json). Published by the original launcher or a trusted attachment manager.

`runtime_id/generation` identify an incarnation; `binding_revision` identifies mapping changes; `sequence` orders observations. `environment_id` identifies its environment. `process_ref` is resolved by a native manager, avoiding a raw PID in the portable contract. `observation_source` distinguishes owned_child and external_attachment; attachment does not confer ownership.

`owner_id` identifies original control authority. State is running, suspended, exited or detached. Optional session_id restricts the binding to one session and must match invocation scope when present. Without it, a native manager may associate several contexts with one runtime. Optional workspace_id and workspace_generation must occur together.

Admission requires a current running binding. Monotonic observations, native incarnation identity, PID reuse protection and durable event capture remain manager duties; the library does not inspect or watch processes. Resume cannot resurrect an exited incarnation; restarting needs a new generation.

## 12. control-grant/v1: control authorization reference

File: [control-grant-v1.schema.json](../../schemas/control-grant-v1.schema.json). The original owner grants limited actions to an explicit holder.

`grant_id/issuer_id/holder_id` separate credential identity, issuer and recipient. `runtime_id/runtime_generation/binding_revision` prevent reusing old authority on a new binding. `actions` contains stop/resume only, and `expires_at_ms` limits validity. Stop applies to running; resume to suspended, never exited or detached instances.

This is an authorization statement contract, not a credential encoding. The caller authenticates the issuer as the current owner; the validator then compares holder, instance, generation, action and time. Copying JSON or discovering a process does not confer authority. Suspend, restart and migration require a new revision or separate capability.

## 13. execution-intent/v1: final execution intent

File: [execution-intent-v1.schema.json](../../schemas/execution-intent-v1.schema.json). Constructed by the native executor after final arguments are determined.

| Group | Reason for binding |
| --- | --- |
| `intent_id/scope` | Prevent reusing a result from another intent, actor or session |
| `tool_name/tool_definition_digest/executor_revision` | A matching tool name cannot conceal changed implementation or argument interpretation |
| `arguments.content/media_type/digest` | Preserve exact native serialized argument bytes without AW reformatting or numeric precision loss |
| `target_id/target_generation` | Bind the actual target incarnation; generation is an opaque native version string |
| `working_directory_ref/environment_revision` | Bind cwd and the actual environment snapshot, resolved consistently by the executor |
| `expires_at_ms` | Bound the dispatch validity window |
| `protection_policy_digest` | Bind the required OS policy; a policy change invalidates prior inspection |

The intent digest covers the whole AW document; arguments.digest covers only the argument string bytes. Any final intent change requires reinspection. Environment revision cannot be an arbitrary label: the executor guarantees that a revision resolves to the same controlled environment. The library checks consistency but does not implement native TOCTOU prevention.

## 14. provider-descriptor/v1: provider capability catalogue

File: [provider-descriptor-v1.schema.json](../../schemas/provider-descriptor-v1.schema.json). Constructed by a trusted manifest loader/Host.

`provider_id/provider_version/manifest_digest` fix the implementation and manifest. `driver/lifecycle` are versioned trusted names; the contract mandates neither one transport nor long-lived processes. `guarantee` distinguishes declared and enforced, but the library does not independently certify that declaration.

Each capabilities item contains capability, authority, input_schema, output_schema and boundaries. Authority is observe, advise, mediate or enforce; provider declarations cannot elevate adapter powers. Capability names are unique within a descriptor. Routing uses a full capability version and both schema references.

Native provider request/response and launch contracts stay component-owned. AW does not duplicate their schemas or define a plugin marketplace. Implementing a new driver belongs to Host; advertising its name cannot claim that implementation exists.

## 15. capability-invocation/v1: invocation admission

File: [capability-invocation-v1.schema.json](../../schemas/capability-invocation-v1.schema.json). Constructed by Core for one capability invocation.

| Group | Meaning and check |
| --- | --- |
| `plan_ref` | Bind the pinned plan ID, revision, full document digest and step_id; calls must use a provider selected by that step |
| `invocation_id/idempotency_key` | Separate call identity from retry semantics. Receivers persist deduplication; a key is not permission for blind retry |
| `provider_id/provider_version/manifest_digest/capability` | Exact match against a trusted descriptor |
| `scope/boundary_id/boundary_revision` | Correlate current runtime and native boundary; tool locations require turn/tool-use identities |
| `policy_revision` | Record resolved policy version. Core stores and verifies its body; this library does not interpret a policy DSL |
| `deadline_at_ms` | Latest completion time for a successful result, in Unix epoch milliseconds |
| `budget.input_bytes/output_bytes/wall_time_ms` | Positive canonical document byte limits and per-call elapsed-time limit |
| `input_schema/output_schema` | Bind actual input and expected output contracts, rejecting unknown resource content |
| `input_digest/input` | Bind the complete input document and separately validate artifact or command text bytes |

Use a small envelope and independent payload profiles rather than one growing union of all capabilities. The validator registers four v2 capabilities and explicitly rejects unknown ones. Shared idempotency storage, scheduling, enforced termination and authentication are not implemented here.

## 16. provider-receipt/v1: provider completion evidence

File: [provider-receipt-v1.schema.json](../../schemas/provider-receipt-v1.schema.json). Host produces it at completion or uncertainty; a presentation layer must not assemble it from unrelated data.

Plan_ref exactly matches the invocation, linking this receipt to a Core plan step without claiming that the entire plan completed. Identity repeats invocation_id, provider/version/manifest, capability, scope, input_schema and input_digest so separately stored records remain correlated. Disposition is produced, bypassed, denied, failed, uncertain or effect_applied. Produced requires output. Failure/denial/uncertainty require error_code and forbid output. Bypassed has neither. Effect_applied requires independent evidence, but semantic validation rejects it for the four bundled read-only/candidate capabilities.

`output.schema/digest/bytes` reference the separately delivered complete output document, omitting candidate text from the receipt. `meters` retain units, methods and measurement nature; IDs cannot repeat. The validator does not recompute arbitrary component metrics, and unknown metrics must not automatically become savings. `evidence` references independent records. Started/completed timestamps are ordered; produced results satisfy deadline and wall-time budget, while late failures remain retainable facts.

Content-free means no raw input/candidate body, not that provider-chosen error codes and identifiers are inherently nonsensitive. Host supplies controlled codes and opaque IDs. A receipt is not a signature, adoption proof, persistence acknowledgement, effect execution proof or model-consumption proof.

## 17. context-adoption/v1: environment observation

File: [context-adoption-v1.schema.json](../../schemas/context-adoption-v1.schema.json). Produced by an environment observer independently from the provider.

| Group | Role |
| --- | --- |
| `invocation_id/scope/boundary_id/boundary_revision` | Bind one invocation and native boundary revision |
| `receipt_digest/source_digest/candidate_digest` | Bind receipt document, original text bytes and complete candidate output document, respectively. Omit candidate_digest when no candidate exists |
| `decision/reason` | Distinguish adopted, preserved, overridden and unverified from their causes. Environment rejection must not become fabricated adoption |
| `effective_digest/effective_bytes` | Exact observed text bytes and length, available only for verified decisions |
| `proof.boundary/representation_id/revision` | Identify the observation layer and native representation/version rather than guessing from a tool ID |
| `proof.evidence/observed_at_ms` | Independent native evidence and observation time, after receipt completion and no later than checking time |
| `ledger_status/ledger_evidence` | Committed requires a separate acknowledgement reference; unavailable forbids one. Caller verifies authentic durable acknowledgement |

Adopted text equals the candidate and satisfies recovery. Preserved text equals the source, allowing truthful reasons such as environment_rejected and recovery_unavailable. Overridden records later transformation without attributable savings. Unverified cannot invent effective text or proof. No-savings reasons must agree with byte lengths in this baseline.

Ledger evidence references an independent append acknowledgement, not the final document containing that acknowledgement; this avoids circular digests. Writers implement their own atomic append/acknowledgement protocol and supply authenticated confirmation. A JSON field is not a database transaction. Required-before-delivery cannot adopt a candidate with an unavailable ledger. Best-effort can truthfully report observed adoption but excludes it from ledger-backed savings.

Validate_plan_adoption first verifies the whole plan and that this invocation, receipt and output occur in its evidence. A cancelled or fallback plan cannot mark a candidate adopted. The lower-level `validate_adoption` derives bytes only: verified, committed adoption yields `max(source bytes − effective bytes, 0)`; verified preservation or override yields zero; unverified or unavailable-ledger observations yield unknown. Aggregation deduplicates invocation, proof boundary and representation revision, displays layers separately, and must not add local-history and model-request savings as two independent gains.

## 18. operation-record/v1: durable external operation

File: [operation-record-v1.schema.json](../../schemas/operation-record-v1.schema.json). Maintained by an environment executor and durable coordinator for potentially effectful operations.

`operation_id/capability/scope` fix identity. `resource_id/resource_generation/expected_revision` bind target and pre-operation revision; generation and revision are opaque strings. `input_digest/idempotency_key` bind full operation input and deduplication identity. `approval_ref` resolves to trusted approval bound to the same operation, target, input and validity window, not an unrelated approval. Approval payloads and authentication are not defined here.

`state/sequence/evidence` model prepared → approved → started → succeeded/no_effect/uncertain. Prepared or approved operations may resolve to no_effect with evidence. Uncertain operations may update queried evidence or converge to succeeded/no_effect; they cannot return to started and execute again. Terminal records are immutable. Exact record replay is an idempotent read, never authorization to repeat an effect.

The validator rejects identity, target, input and existing approval-reference changes, requiring sequence increments of one. Storage owns compare-and-swap, approval authenticity, durable started-before-effect ordering and crash recovery. No_effect requires evidence; a disconnected connection does not prove absence of change.

This resource does not define snapshot scope, exclusions, restore preflight, file digests or create/restore payloads. Those require separate future state capability profiles. It provides an extension point without claiming a completed State Provider implementation.

## 19. capability-plan/v1: Core-owned ordered plan

File: [capability-plan-v1.schema.json](../../schemas/capability-plan-v1.schema.json). Trusted Core produces plans for schedulers, final executors and auditors. The contract preserves capability selection and failure ownership without defining another agent loop or native event bus.

| Group | Meaning and trade-off |
| --- | --- |
| `plan_id/revision/event_id` | Pin one boundary event; adapters map event identity rather than letting independent plugins mint unrelated events |
| `scope/boundary_id/boundary_revision/boundary` | Bind native environment/location and validate against a trusted descriptor before calls |
| `policy_revision/source_digest` | Pin resolved policy and original text bytes; the whole-plan digest covers routes, order and policy choices |
| `steps[].step_id/capability/input_schema/output_schema` | Ordered steps with exact contracts and unique step IDs |
| `steps[].selection/providers` | Exactly_one or all_distinct_providers with pinned provider ID/version/manifest; reject duplicates and unselected implementations |
| `steps[].required/on_failure` | Mandatory checks versus optional facts; reject_plan, record_gap_and_continue or deny_dispatch are Core choices, not provider downgrades |
| `steps[].input_source` | Fixed boundary_source in this profile; no implicit reuse of another step's output |
| `os_requirement.policy_digest/required_controls` | Required only for pre_tool; pin OS policy and controls that must actually be enforced |

Command checks require pre_tool, required=true and deny_dispatch on failure. A pre-tool plan contains at least one command check and requires a native final denial guard. Projection cannot run at pre_tool; it requires exactly_one, reject_plan and final position. Source inspections therefore precede candidate generation and at most one projection occurs. Unknown capabilities fail explicitly. The [complete pre-tool plan](../../tests/fixtures/pre-tool-plan.json) demonstrates two ordered mandatory checks and independent OS requirements. [Orchestration tests](../../tests/orchestration.rs) cover dispatch counterexamples; these are synthetic contract fixtures.

All_distinct_providers accounts for every selected result. Any non-allow command verdict denies dispatch rather than letting the last response win. Steps settle serially; read-only checks within one step do not depend on provider return order. Empty routing produces an execution gap and terminates required work according to policy. Content/code produce observations; required does not automatically promote findings into denial authority. Arbitrary transformation chains and new risk-interpretation rules need explicit profile review.

## 20. plan-execution/v1: whole-plan execution result

File: [plan-execution-v1.schema.json](../../schemas/plan-execution-v1.schema.json). Produced by a trusted Core journal writer, not one provider asserting global success.

Plan_id/revision/plan_digest/event_id/scope/boundary_id/boundary_revision identify the exact plan. Entry count and order equal the plan's steps. Each entry has step_id, outcome, invocations and started/settled sequences for started steps. Non-completed entries require reason. Completed means usable selected-provider results; gap means unavailable results; cancelled records cancellation; skipped means an earlier terminal decision prevented starting this step.

Invocations reference actual invocation_id and receipt_digest only. Uncalled steps cannot manufacture receipts. Reject duplicate invocation IDs, reused provider idempotency keys, omitted selected providers, cross-step receipt reuse and fabricated completion. Call admission still requires validate_invocation against trusted descriptors and current runtime/deadlines; completion validation does not replace admission.

One Core's monotonic journal assigns started < settled and each prior settled < next started. Skipped steps have no start/settle sequence and use previous_step_stopped. No step may run after a terminal decision. Proceed, deny, preserve or cancelled must agree with actual results. Denial/warn cannot be overwritten by later allow. Failed projection yields preserve, not a claim that tool execution failed or was rolled back.

Evidence references durable journal records. Storage/Core/executor duties include authenticated records, immutable terminal results and claiming actual dispatch once per event_id. The validator checks supplied record consistency; it implements no scheduler, persistence or next() wrapper.

## 21. os-protection-binding/v1: independent OS protection

File: [os-protection-binding-v1.schema.json](../../schemas/os-protection-binding-v1.schema.json). Published by an authenticated protection authority to describe established restrictions, not request installation or let a provider grade itself.

| Group | Meaning |
| --- | --- |
| `binding_id/scope/target_id/target_generation` | Bind actual incarnation, context and target generation; old protection cannot cover a new instance |
| `policy_digest/authority_id` | Match plan, intent and the caller's preconfigured trusted authority |
| `state` | Active, unavailable or failed; the latter two cannot satisfy mandatory protection |
| `controls[].control_id/mechanism/coverage` | Separate control identity, mechanism and enforced/declared/unsupported coverage; only matching enforced controls satisfy requirements |
| `observed_at_ms/expires_at_ms` | Trusted-clock validity window; callers still obtain current authority state instead of replaying cached claims |
| `evidence` | Active requires independent activation evidence with authenticated source/digest; the declaration itself is not kernel proof |

The plan specifies mandatory controls such as filesystem.access/v1 and network.egress/v1. A trusted policy under policy_digest defines exact access rules. Reject duplicate control IDs, partial required coverage, inactive state, wrong target/generation and expired binding. OS controls constrain resource access independently of whether hooks call next. AW allow cannot remove those restrictions; the protection authority is isolated from untrusted plugins.

Validate_os_protection checks one binding; final admission uses validate_dispatch for the whole plan, intent and protection. Kernel rule syntax, policy installation protocol and generic OS-denial event formats are not defined here, and real OS enforcement has not been accepted through live testing. Existing native provider protocols and execution paths do not automatically acquire an OS backstop from this schema.

## Encoding, versioning and review order

AW JSON v1 is this project's restricted encoding profile, **not RFC 8785**. Sort object members by ASCII key, preserve array order, emit compact UTF-8 JSON and do not normalize Unicode strings. Reject duplicate keys, float/exponent spellings, negative zero, non-ASCII metadata keys, invalid Unicode and oversized documents. Wire ingress must use a strict parser: shape validation cannot recover duplicate-key ambiguity after an ordinary parser loses it.

Hash raw content as original UTF-8 bytes; hash input/output/receipt/intent documents using canonical bytes; hash schema resources as exact repository file bytes. These digest domains are not interchangeable. Schema URIs identify resources rather than promise hosted endpoints; the registry only uses bundled resources.

Review authority and adoption boundaries first, capability payloads second, encoding and compatibility last. Explicit decisions include the 4 MiB/depth-32 limits, ASCII metadata domain, dual-language auto semantics, recovery expiry versus retention, budget meanings and operation state transitions. This baseline makes those choices concrete for acceptance or revision instead of hiding them as implementation defaults.
