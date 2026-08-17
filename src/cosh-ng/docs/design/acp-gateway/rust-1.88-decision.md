# Rust 1.88 Toolchain Decision

[中文版](rust-1.88-decision_zh.md)

Related architecture: [COSH Gateway and ACP Architecture](README.md)

## Context

The official `agent-client-protocol` 2.0.0 package declares MSRV 1.88.0.
cosh-ng needs one reproducible compiler baseline for local development, CI,
RPM builds, and ACP Runtime integration.

## Decision

The cosh-ng minimum Rust version and pinned toolchain are **1.88.0**.

This changes the compiler baseline only. It does not require moving existing
crates from Rust edition 2021 to edition 2024. The toolchain change remains
independent from SDK adoption, protocol code, and Gateway features so each can
be reviewed and rolled back separately.

## Rationale

- Satisfy the explicit MSRV of the official ACP Rust SDK 2.0.0.
- Remove drift between developer machines, CI, RPM builders, and release jobs.
- Keep protocol failures separate from toolchain failures.
- Use the dependency's minimum required version instead of an arbitrary newer
  stable compiler.

## Workspace requirements

| Location | Requirement |
| --- | --- |
| `src/cosh-ng/Cargo.toml` | `workspace.package.rust-version = "1.88"` |
| `src/cosh-ng/rust-toolchain.toml` | Pin `channel = "1.88.0"` with rustfmt and Clippy |
| cosh-ng CI | Install and use 1.88.0 rather than runner default stable |
| RPM/build image | Fail clearly when Rust 1.88 is unavailable |
| Developer docs | State MSRV, installation, and troubleshooting impact |

These locations change atomically. A future compiler upgrade follows the same
rule.

## Validation gate

Toolchain or dependency changes run on Linux:

```bash
cd src/cosh-ng
rustc --version
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
```

`rustc --version` must report 1.88.x. Public API or rustdoc changes also run:

```bash
cargo doc --workspace --no-deps --locked
```

The actual RPM build environment is validated independently; success on a
GitHub runner does not prove the release image can provide the toolchain.

## Compatibility impact

- Source builders using an older Rust compiler must upgrade first.
- Existing binaries and runtime protocols do not change merely because the
  compiler changed.
- Edition remains 2021 to avoid unrelated semantic changes.
- Rust 1.88 does not authorize unrelated dependency upgrades.

## Upgrade and rollback

Compiler, CI, and RPM baselines move together. If a supported build environment
cannot provide the pinned version, revert the toolchain change as a unit. ACP
must not enter main through a forked SDK, copied generated types, or bypassed
MSRV checks; such an alternative requires a new architecture decision.

## Reference

- [agent-client-protocol 2.0.0 manifest](https://docs.rs/crate/agent-client-protocol/2.0.0/source/Cargo.toml)
