# Testing

## Running Tests

```bash
cd src/cosh-ng

# All tests
cargo test --locked

# By crate
cargo test --locked -p cosh-types      # Pure types, no tests
cargo test --locked -p cosh-platform   # 174 unit tests
cargo test --locked -p cosh-cli        # 55 integration tests
cargo test --locked -p cosh-core       # Unit + integration tests
cargo test --locked -p cosh-shell      # Unit + integration (includes PTY tests, slower)

# Run a single test
cargo test --locked -p cosh-platform test_detect_alinux

# Run a specific test file
cargo test --locked -p cosh-cli --test cli_integration
```

## Test Layers

### cosh-types

Pure type definitions, typically no tests. Serialization/deserialization correctness is covered by upper-layer crate tests.

### cosh-platform

Unit tests cover:
- Distribution detection (mocking `/etc/os-release`)
- Package manager command generation
- Audit policy rule matching
- ws-ckpt IPC protocol encoding/decoding

```bash
cargo test --locked -p cosh-platform
# ~174 tests, < 1s
```

### cosh-cli

Integration tests (`crates/cosh-cli/tests/cli_integration.rs`):
- JSON output envelope format validation
- `--dry-run` behavior
- Argument parsing for each subcommand
- Error code mapping

```bash
cargo test --locked -p cosh-cli
# ~55 tests, ~4s
```

### cosh-core

| Test File | Coverage |
|-----------|----------|
| `tests/jsonl_protocol.rs` | JSONL message serialization/deserialization |
| `tests/registry_protocol.rs` | Registry mode request-response |
| `tests/tool_approval.rs` | Tool approval protocol |
| `tests/sls_integration.rs` | SLS logging integration |

Unit tests distributed across modules:
- `extension/` — Configuration parsing, variable substitution, extension loading
- `state.rs` — State file read/write
- `hook.rs` — Hook protocol

```bash
cargo test --locked -p cosh-core
```

### cosh-shell

Most complex test structure, divided into three layers:

**Integration tests** (`crates/cosh-shell/tests/`):

| Directory | Coverage |
|-----------|----------|
| `logic/` | Command classification, failure analysis, agent event handling |
| `protocol/` | Adapter protocol, control messages |
| `raw_cli/` | Raw mode CLI behavior |
| `shell_host/` | PTY sessions, OSC marker parsing |

**Inline unit tests** (`#[cfg(test)]` modules in `src/`):

| Module | Coverage |
|--------|----------|
| `approval/tests.rs` | Approval decision logic |
| `hooks/` | Hook engine, runtime behavior |
| `runtime/` | State machine, evidence requests |
| `shell_host/osc_tests.rs` | OSC escape sequence parsing |
| `tools/` | Tool risk analysis, display |

```bash
cargo test --locked -p cosh-shell
# Slower (PTY tests require fork + terminal interaction)
```

## Test Utilities

| Utility | Purpose |
|---------|---------|
| `tempfile` | Create temporary directories for isolated tests |
| `COSH_STATES_DIR` | Environment variable to override state directory |
| `ExtensionManager::new_isolated()` | Test-specific constructor |

## Writing Tests Guide

1. Integration tests go in `tests/` directory, unit tests inline in modules
2. Test function naming: `test_<behavior_under_test>_<scenario>`
3. Use `tempfile::tempdir()` to isolate filesystem side effects
4. Do not depend on network (LLM API tests use mocks)
5. PTY tests annotated with `#[ignore]` (if they require a real terminal environment)
