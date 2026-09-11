# AW

[中文版](README_zh.md)

AW provides versioned JSON Schemas and offline validators for capability calls and their records. The Rust library checks payload shapes and relationships between invocations, results and observations. It has no service process and does not execute providers or control agents.

The interfaces are experimental. Tests use synthetic records and do not certify runtime integration.

## Run the checks

Prepare a Rust toolchain with rustfmt and Clippy. The cross-language digest test also needs Python 3 and Node.js; the Rust library does not depend on either runtime. Run from the repository root:

```bash
cd src/aw
cargo test --workspace --locked
python3 tests/check_canonical.py
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
```

These checks run as a regular user without an Agent or service login. Cargo downloads uncached dependencies; schema validation reads only bundled resources.

The checked environment is Linux ARM64 with Rust 1.97.1, Python 3.12.3 and Node.js 24.15.0. Minimum supported versions and other operating systems have not been verified.

## Source reference

- [Registered schemas](schemas/) and [synthetic payload examples](tests/fixtures/contracts.json)
- [Public API](src/lib.rs), [record validation](src/validation.rs) and [plan validation](src/orchestration.rs)
- [Encoding tests](tests/canonical.rs), [schema tests](tests/schemas.rs),
  [record tests](tests/contracts.rs) and [plan tests](tests/orchestration.rs)

The Registry includes 21 schema resources. The eight v1 resources in `crates/aw-contracts/schemas/` are reference copies and are not registered. Callers must use matching schema IDs and digests; no automatic version conversion is provided.

Parse incoming bytes with `canonical::parse` before schema validation. Shape checks alone do not validate record relationships or grant authorization. Follow the public API documentation for plan-level checks; callers remain responsible for authenticating evidence and enforcing actions.
