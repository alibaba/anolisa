# Backend Skill - Quick Reference

## When to Use

Use the `backend-skill` when you need to add a new security capability to agent-sec-cli.

## Backend Type Selection

| Choose | When | Example Use Cases |
|--------|------|-------------------|
| **python** | Rapid development, existing libraries, glue code | Code scanning, config validation, log analysis |
| **rust** | Performance-critical, CPU-intensive, memory safety | Cryptographic operations, large file processing, pattern matching |

## Quick Start

### Python Backend (Recommended for most cases)

**Prompt**:
```
Use backend-skill in folder dev-tools to create a new python backend called my_scanner with module_path agent_sec_cli.my_scanner.analyzer
```

**What gets created**:
1. Backend wrapper: `security_middleware/backends/my_scanner.py`
2. Router entry in `router.py`
3. Lifecycle category in `lifecycle.py`
4. CLI subcommand in `src/agent_sec_cli/cli.py`

**You need to create**:
```
src/agent_sec_cli/my_scanner/
├── __init__.py
├── analyzer.py        # Your core logic
└── tests/
    └── test_analyzer.py
```

** analyzer.py template**:
```python
def analyze(**kwargs) -> dict:
    """Your analysis logic."""
    return {
        "success": True,
        "output": "Analysis complete",
        "data": {"checked": 10, "passed": 10},
    }
```

### Rust Backend (For performance-critical tasks)

**Prompt**:
```
Use backend-skill in folder dev-tools to create a new rust backend called crypto_verify
```

**What gets created**:
1. Backend wrapper: `security_middleware/backends/crypto_verify.py` (with Python fallback)
2. Rust function stub in `rust_backends/src/lib.rs`
3. Router entry in `router.py`
4. Lifecycle category in `lifecycle.py`
5. CLI subcommand in `src/agent_sec_cli/cli.py`

**You need to implement**:
- Rust logic in `rust_backends/src/lib.rs` (see SKILL.md Section 4.2)

**Build**:
```bash
make build-rust-backends
```

## Testing

### Python Backend
```bash
# Unit tests
pytest src/agent_sec_cli/my_scanner/tests/

# E2E through CLI
agent-sec-cli my_scanner --param1 value
```

### Rust Backend
```bash
# Rust unit tests
cd src/agent_sec_cli/rust_backends
cargo test

# E2E through CLI
agent-sec-cli crypto_verify --param1 value

# Test Python fallback
mv src/agent_sec_cli/rust_ext/rust_backends.so src/agent_sec_cli/rust_ext/rust_backends.so.bak
agent-sec-cli crypto_verify --param1 value
mv src/agent_sec_cli/rust_ext/rust_backends.so.bak src/agent_sec_cli/rust_ext/rust_backends.so
```

## Key Files

| File | Purpose |
|------|---------|
| `dev-tools/backend-skill/SKILL.md` | Complete step-by-step guide |
| `dev-tools/backend-skill/templates/python_backend.py` | Python backend template |
| `dev-tools/backend-skill/templates/rust_backend.py` | Rust backend template |
| `src/agent_sec_cli/asset_verify/` | Example Python backend (production) |
| `src/agent_sec_cli/security_middleware/backends/` | All backend wrappers |

## Common Patterns

### Pattern 1: Python Module Delegation
```python
# In backend wrapper
def _run(module, **kwargs) -> ActionResult:
    result = module.analyze(**kwargs)
    return ActionResult(
        success=result["success"],
        stdout=result["output"],
        data=result["data"],
    )
```

### Pattern 2: Rust with Fallback
```python
# In backend wrapper
def execute(self, ctx, **kwargs) -> ActionResult:
    if RUST_AVAILABLE:
        return self._execute_rust(**kwargs)
    return self._execute_python(**kwargs)  # Fallback
```

### Pattern 3: CLI Subcommand
```python
# In src/agent_sec_cli/cli.py
@app.command()
def my_scanner(
    param1: str = typer.Option("", "--param1", help="Parameter 1"),
    verbose: bool = typer.Option(False, "--verbose", help="Verbose output"),
):
    """My scanner description."""
    result = invoke("my_scanner", param1=param1, verbose=verbose)
    ...
```

## Troubleshooting

### Issue: Module not found
```
ModuleNotFoundError: No module named 'agent_sec_cli.my_scanner'
```
**Solution**: Ensure module is in `src/agent_sec_cli/` and has `__init__.py`

### Issue: Rust extension not loading
```
ImportError: cannot import name 'rust_backends'
```
**Solution**: Run `make build-rust-backends` and copy `.so` to `rust_ext/`

### Issue: No CLI output
**Solution**: Ensure `ActionResult` has `stdout` or `error` field populated

## Examples

See `src/agent_sec_cli/asset_verify/` for a complete Python backend example with:
- Core logic implementation (`verifier.py`)
- Configuration management (`config.conf`)
- Trusted key management (`trusted-keys/`)
- Proper error handling

## Next Steps

1. Read `SKILL.md` for detailed instructions
2. Choose backend type (python or rust)
3. Run the skill with appropriate parameters
4. Implement your core logic
5. Add tests
6. Run E2E verification
