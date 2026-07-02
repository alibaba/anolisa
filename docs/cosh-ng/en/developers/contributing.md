# Contributing Guide

## Development Environment

| Requirement | Version |
|-------------|---------|
| Rust toolchain | stable (managed by `rust-toolchain.toml`) |
| Minimum Rust version | 1.74 |
| Components | rustfmt + clippy |

```bash
cd src/cosh-ng
rustup show   # Confirm toolchain is ready
```

## Build

```bash
# Full build (all 5 crates)
cargo build --workspace

# Release build
cargo build --workspace --release

# Build a specific binary
cargo build --bin cosh-cli
cargo build --bin cosh-core
cargo build --bin cosh-shell
```

## Code Quality Checks

All of the following checks must pass before committing:

```bash
# Format check
cargo fmt --all -- --check

# Clippy (warnings treated as errors)
cargo clippy --all-targets --locked -- -D warnings

# Tests
cargo test --locked

# Documentation build (when modifying pub API)
cargo doc --workspace --no-deps
```

## Workspace Structure

```
cosh-ng/
├── Cargo.toml              # workspace configuration
├── rust-toolchain.toml     # stable + rustfmt + clippy
└── crates/
    ├── cosh-types/         # Pure types, zero side effects
    ├── cosh-platform/      # Platform abstraction (distro detection, backend routing)
    ├── cosh-cli/           # CLI entry
    ├── cosh-core/          # Agent core
    └── cosh-shell/         # Interactive terminal
```

## Dependency Management

- All dependency versions are declared in `[workspace.dependencies]`
- Sub-crates reference via `dep = { workspace = true }`
- Check for existing equivalent crates before adding new dependencies
- Major version upgrades are not allowed without discussion

## Code Standards

### Module Organization

Use Rust 2018+ recommended file layout, **do not use `mod.rs`**:

```
# Correct
src/extension.rs        # Parent module
src/extension/          # Child module directory
    config.rs
    manager.rs

# Wrong — do not use
src/extension/mod.rs
```

### Error Handling

| Scenario | Approach |
|----------|----------|
| Library crate | `thiserror` enum |
| Binary | `anyhow::Result` |
| Unreachable path | `unreachable!()` + comment |
| Prohibited | `unwrap()` / `expect()` / `panic!()` |

### Comments

- `///` for all pub items
- `//` only explains *why*, does not repeat type signatures
- First line is a standalone summary, imperative or noun phrase
- No `TODO` without owner, no commented-out old code

### Clippy

- Default deny all warnings
- When genuinely needed, use narrowest scope `#[allow(clippy::xxx)]` + comment explaining why

## Commit Standards

Format: `type(scope): imperative description`

- Types: feat / fix / refactor / docs / test / ci / chore
- Scope: `cosh-ng` maps to `cosh` (when changes span multiple crates)
- Within 50 characters, English, imperative mood, lowercase first letter, no period
- Requires `Signed-off-by` trailer

```bash
git commit \
  --trailer "Assisted-by: Qoder:1.7.0" \
  --trailer "Signed-off-by: $(git config user.name) <$(git config user.email)>" \
  -m 'feat(cosh): add registry list action for hooks'
```

## PR Process

1. Branch from latest main
2. Follow branch naming: `feature/cosh/<short-desc>`
3. Ensure all checks pass before pushing
4. PR title follows commit message format
5. Fill in PR template (Description / Testing / Related Issue)
