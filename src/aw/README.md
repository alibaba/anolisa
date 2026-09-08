# AW

[中文](README_zh.md)

AW defines versioned capability contracts between native agent adapters, a coordinating core and component providers. This portable Rust library bundles JSON Schemas and offline semantic validators. It separates provider output from environment adoption, runtime observation from control authority, and effect preparation from completion. Existing components remain independently usable.

**Status: proposed freeze baseline for review, not a released compatibility promise.** This component does not yet implement an AW Core service, Provider Host, native adapter, ledger backend, process controller or state provider. Its tests validate contract consistency with synthetic fixtures; they do not certify a framework integration.

## Start from source

Prepare a Rust toolchain with rustfmt and Clippy. The cross-language digest test also needs Python 3 and Node.js; the Rust library itself does not depend on either runtime. Run these commands from the repository root:

```bash
cd src/aw
cargo test --workspace --locked
python3 tests/check_canonical.py
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
```

These checks run as a regular user without starting an Agent or signing into a service. Cargo downloads dependencies that are not cached; schema validation reads bundled resources and never fetches a schema URI.

This validation used Linux ARM64 with Rust 1.97.1, Python 3.12.3 and Node.js 24.15.0. These versions reproduce the checked environment. Minimum supported versions have not been established, and other operating systems have not been validated.

## Read and integrate

- [PoC baseline and interface delta](docs/design/poc-schema-delta.md)
- [Freeze decision and incremental acceptance gates](docs/design/interface-freeze.md)
- [Every schema: fields, reasoning and review boundaries](docs/design/schema-reference.md)
- [Schema resources](schemas/) and [complete synthetic examples](tests/fixtures/contracts.json)
- [Validation API](src/validation.rs) and [contract tests](tests/contracts.rs)

Callers parse untrusted wire bytes with `canonical::parse` and reuse a `Registry`. Core checks a pinned ordered plan with `validate_plan`, admits each invocation with both `validate_plan_invocation` and `validate_invocation`, and checks all settled steps and receipts with `validate_plan_execution`. Final adoption uses `validate_plan_adoption`; final dispatch uses `validate_dispatch` to check the whole plan, current execution intent and independent OS protection binding.

`validate_result`, `validate_adoption` and `validate_execution_gate` are local consistency checks, not complete plan admission. Shape-only `Registry::validate` is also insufficient for authorization. The library neither schedules providers nor installs OS rules. Native executors authenticate evidence, serialize final checks with actions and claim each event_id once to prevent duplicate dispatch.

## Contract boundaries

The Registry bundles 21 schema resources (plus eight unregistered PoC v1 review resources kept separately): one shared definitions resource, eight capability input/output resources, and twelve orchestration/evidence resources. Capability v2 IDs preserve room for explicit migration from experimental v1 contracts. Exact schema ID and resource digest must match; consumers must not silently reinterpret another revision.

Text projection covers **one UTF-8 text slot**, preserving the surrounding native tool result and other blocks. A receipt records what a provider produced. Only an independently captured observation may establish adoption at `final_tool_result`, `local_history` or `model_request`; none of these proves remote model consumption. Ledger acknowledgement authenticity and durability remain storage responsibilities.

Effect operation records provide a generic approval/idempotency state machine. They do not define or execute checkpoint, restore or arbitrary state capabilities. Those payload profiles and recovery implementations require separate reviewed additions.

OS protection independently enforces ongoing system restrictions; it is not a scanner activated after plugin failure. `os-protection-binding` binds target, policy digest, required controls and coverage evidence. `validate_dispatch` rejects missing, expired or merely declared mandatory protection. Actual kernel restrictions, hook-bypassing access denial and child-process coverage require later native implementation and live validation.
