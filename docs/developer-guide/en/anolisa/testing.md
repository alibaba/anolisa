# Testing

Testing strategy and conventions for the anolisa CLI component.

---

## Test Structure

anolisa tests are organized at the crate level, following Rust conventions:

```
crates/anolisa-cli/tests/      # Integration tests for CLI binary
crates/anolisa-core/src/       # Unit tests inline (#[cfg(test)] modules)
crates/anolisa-env/src/        # Unit tests for env probes
crates/anolisa-platform/src/   # Unit tests for platform abstractions
crates/anolisa-build/src/      # Unit tests for build backends
```

## Running Tests

```bash
# From src/anolisa/

# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p anolisa-cli
cargo test -p anolisa-core

# Run with output
cargo test -- --nocapture

# Run a specific test
cargo test -p anolisa-core -- manifest::tests::test_parse
```

## Code Quality

```bash
# Format check
cargo fmt --check

# Lint
cargo clippy -- -D warnings

# Full CI check
cargo fmt --check && cargo clippy -- -D warnings && cargo test
```

## Test Conventions

### Unit Tests

- Located in `#[cfg(test)]` modules at the bottom of each source file
- Test pure logic without filesystem or network dependencies
- Use table-driven tests for parsers and validators
- Example from manifest parsing:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_manifest() {
        let input = r#"
            [component]
            name = "test-component"
            version = "1.0.0"
        "#;
        let result = parse_manifest(input);
        assert!(result.is_ok());
    }
}
```

### Integration Tests

- Located in `tests/` directories
- Test end-to-end CLI behavior
- Use temporary directories via `tempfile`
- Mock registry responses for download tests
- Verify journal integrity after install/uninstall operations

### Test Isolation

- Each test creates its own state directory
- No shared mutable state between tests
- Temporary directories are cleaned up on test completion
- Use `ANOLISA_STATE_DIR` to isolate state

## Platform-Specific Testing

Some tests require Linux-specific features and are gated:

```rust
#[cfg(target_os = "linux")]
#[test]
fn test_ebpf_probe() {
    // Requires kernel 5.x+
}
```

### Alinux CI Environment

Tests are run on Alinux (Anolis OS) in CI with:
- GCC 13 (via `scl enable gcc-toolset-13`)
- Rust 1.88+
- Kernel 5.10+ with eBPF support

## Writing New Tests

1. **Pure logic**: Add `#[test]` in the source file's test module
2. **File I/O**: Use `tempfile::TempDir` for isolation
3. **CLI integration**: Add to `crates/anolisa-cli/tests/`
4. **Cross-crate**: Test in the highest crate that can exercise the full path
5. **Regression tests**: Reference the issue/PR number in the test name

## Before Submitting a PR

```bash
cd src/anolisa
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

All three must pass with zero warnings.
