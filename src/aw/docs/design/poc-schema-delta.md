# PoC schema baseline and interface delta

[中文版](poc-schema-delta_zh.md)

This branch separates review into two layers: the eight corrected PoC v1 capability schemas, followed by the proposed contract freeze baseline. Reviewers can inspect the added interfaces separately from the PoC runtime implementation.

## Provenance and scope

The first layer extracts selected paths from the [pinned PoC revision](https://github.com/kongche-jbw/anolisa/tree/5ebfc0b3905fa2f5f74aff2da4aec2b3be639647/src/aw/crates/aw-contracts/schemas). Paths, bytes and schema IDs are unchanged. The four schema changes below are replayed in order as four separate commits, each attributed with a `Source-commit` trailer. These are path-filtered replays, not full cherry-picks of the PoC commits. It excludes Core, Host, Ledger, Provider native protocols, manifests and deployment scripts.

Original schema history:

- [Projection contracts](https://github.com/kongche-jbw/anolisa/commit/b299cdfe): introduces two projection schemas.
- [Inspection contracts](https://github.com/kongche-jbw/anolisa/commit/1556e9d6): introduces six security schemas.
- [Contract corrections](https://github.com/kongche-jbw/anolisa/commit/4d47593b): tightens recovery and security outcomes.
- [Input/output binding corrections](https://github.com/kongche-jbw/anolisa/commit/8ecb1412): clarifies media types and coverage.

The second layer cherry-picks the [interface commit](https://github.com/kongche-jbw/anolisa/commit/6fe1d30b0235b8039ac423d1649f2bdcb96d3b63), then adds this comparison. Root READMEs and `.github/` remain at the base revision.

## Layout and compatibility

| Location | Purpose | Registered by the current library |
| --- | --- | --- |
| `crates/aw-contracts/schemas/` | Eight unchanged PoC v1 review resources; no second Rust crate exists here | No |
| `schemas/` | 21 proposed contracts: eight capability v2 resources, one common resource and 12 coordination/evidence schemas | Yes |
| `src/`, `tests/` | Offline validation APIs, cross-record checks and synthetic fixtures | Current implementation |

Retaining v1 files does not add v1 support to Registry or migrate PoC callers. The old `/schemas/capabilities/.../v1` and new `/schemas/aw/.../v2` identities differ; changing a version number or URI is insufficient. Callers must negotiate schema identities and digests and update adapter/Provider mappings explicitly. No automatic conversion is implemented.

## Eight capability schema changes

The following file prefixes use `-v1.schema.json` in the old directory and `-v2.schema.json` in the new directory.

| File prefix | Existing PoC contract | Change and rationale |
| --- | --- | --- |
| `context-projection-prepare-input` | Artifact, boundary and media type reencoding permission | Adds `accepted_reversibility` so callers explicitly constrain recovery guarantees |
| `context-projection-prepare-output` | Source ID/digest, candidate, transform chain and reversibility | Adds matching `recovery` metadata and independent recovered-byte checks; a declared class is not proof |
| `security-content-inspect-input` | Artifact, boundary and low-confidence reporting policy | Shares artifact definitions for text slots, IDs, media types and numeric constraints |
| `security-content-inspect-output` | Verdict, findings, scanned bytes and truncated flag | Uses `coverage` for input digest, input/scanned bytes, completeness, rulesets and languages; retains complete and empty-findings requirements for clean |
| `security-code-inspect-input` | Artifact, boundary and bash/python/auto | Shares artifact definitions; cross-record checks require auto to cover both languages |
| `security-code-inspect-output` | Inspection and `language_detected` | Uses `coverage.languages` for actual declared scan scope and checks it against the request |
| `security-command-inspect-input` | Command text/digest/language and pre_tool | Adds `execution_intent_digest` binding full arguments, target, cwd, environment and protection policy |
| `security-command-inspect-output` | Allow/warn/deny, reasons, findings and scanned bytes | Echoes intent digest and provides coverage; final admission rechecks the current execution intent |

Shared definitions also change accepted IDs, media types, boundaries and numeric ranges. This table is not an exhaustive field diff. See the [schema reference](schema-reference.md) for field-level constraints and both JSON directories for exact wire shapes.

## Added coordination and evidence contracts

The PoC already contains Provider, Receipt, Ledger, adoption and ordered Core plan concepts, including Rust implementations. The following are new or strengthened public schema expressions, not claims that those concepts originated here.

| Schema | Boundary made explicit |
| --- | --- |
| `common` | Shared scope, artifact, coverage, meter and evidence references |
| `boundary-descriptor` | Actual adapter observation/denial boundaries, input finality and final guard |
| `runtime-binding` | Runtime identity, generations and observation source; observation is not control authority |
| `control-grant` | Holder, actions, target generation and expiry |
| `execution-intent` | Complete intent that must remain consistent between inspection and dispatch |
| `provider-descriptor` | Provider identity, capability versions, guarantees and resource bindings |
| `capability-invocation` | Scope, budgets, deadline, input and pinned plan step |
| `provider-receipt` | Result binding to invocation, input and plan; does not prove adoption |
| `context-adoption` | Independently observed adoption boundary, effective text and Ledger acknowledgement |
| `operation-record` | Approval, idempotency and uncertainty for generic effects; no checkpoint payload yet |
| `capability-plan` | Pinned ordered steps, Provider selection, required gates and failure rules |
| `plan-execution` | Ordered evidence for all steps and Receipts; omissions or later allows cannot bypass denial |
| `os-protection-binding` | Independent OS protection target, policy, coverage, validity and evidence |

Ordering covers a Core-owned plan, not all framework plugins globally. Native execution layers must continuously enforce OS protection. This library checks binding consistency from a trusted caller; JSON claims cannot establish kernel isolation.

## Reviewing and validating

The branch has five commits above the latest checked main baseline: four ordered PoC schema changes followed by the interface changes. The fifth commit shows the added interface layer and leaves all eight accumulated first-layer schemas unchanged. Run from the repository root:

```bash
git log --oneline HEAD~5..HEAD
git diff --stat HEAD~1 HEAD
git diff HEAD~1 HEAD -- src/aw
git diff --exit-code HEAD~1 HEAD -- src/aw/crates/aw-contracts/schemas
```

The first layer only checks JSON parsing and exact source-byte equality; it has no compilable runtime. The final layer runs the component README's Rust checks, 28 tests and Python/JavaScript digest vectors. These validate offline contracts, not actual Agent, Provider or OS integration. Freeze acceptance and runtime migration still require review and staged acceptance.
