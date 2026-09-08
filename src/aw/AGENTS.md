# AW contract development

Read the root AGENTS.md and repository documentation standard before changing this component.

## Responsibilities

This is a portable, side-effect-free contract library. Do not add process control, network calls, provider execution, native agent hooks, authorization credentials or ledger storage here. A schema URI is an identity, not permission to fetch a resource.

Schema resources are authoritative wire shapes. Rust checks cross-record invariants; native callers authenticate observations and enforce atomicity. Keep these layers explicit in code and documentation. Never equate receipt production with adoption, process observation with ownership, or declared coverage with independently verified scanner behavior.

The current schema bundle is a proposed freeze baseline. After acceptance, incompatible shape, enum, hashing or semantic changes require new schema IDs and negotiation; do not broaden an existing revision silently. Native provider protocols stay owned by their components. Unsupported profiles fail explicitly.

Core owns pinned serial plans, selected providers and failure rules. Required command gates cannot be skipped or overridden by later allow results. OS protection is independently enforced by a trusted runtime authority; JSON coverage claims alone are not authentication. Keep complete-plan admission distinct from single-invocation consistency checks.

## Validation

Run from this directory:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python3 tests/check_canonical.py
cargo doc --workspace --no-deps --locked
```

The vector check also requires Python 3 and Node.js. Tests and fixtures belong in `tests/`; Cargo outputs belong in `target/`. Fixtures are synthetic and must never contain credentials, real user text, private endpoints or deployment identifiers. Each payload schema needs a valid fixture and meaningful invalid cases. Update English and Chinese design explanations together.
