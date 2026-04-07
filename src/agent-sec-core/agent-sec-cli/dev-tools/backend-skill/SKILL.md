---
name: add-backend
description: Guide for adding a backend (Rust or Python) to the agent-sec-core security middleware. Use when creating new backends, integrating Rust or Python code into the security middleware, or extending with new backend actions.
arguments:
  - name: backend_name
    description: "Name of the new backend (e.g. 'code_verify'). Spaces are converted to underscores for code identifiers."
    required: true
  - name: backend_type
    description: "Backend implementation type: 'rust' or 'python'"
    required: true
  - name: module_path
    description: "For python type: module path (e.g. 'agent_sec_cli.code_verify.verifier'). Required when backend_type=python."
    required: false
---

# Adding a Backend to Security Middleware

This skill walks through the **complete, end-to-end** process of adding a backend
(Rust or Python) to the security middleware, wiring it into the router, and
exposing it through the CLI.

> **Unified interface**: Both Rust and Python backends implement the same
> `execute(ctx, **kwargs) → ActionResult` contract. The middleware doesn't care
> about the implementation language.

## Backend Type Selection

| Type | Use Case | Pros | Cons |
|------|----------|------|------|
| **rust** | Performance-critical, CPU-intensive tasks | High performance, memory safety | Requires Rust toolchain, compilation |
| **python** | Rapid development, glue code, existing libraries | Fast iteration, rich ecosystem | Slower execution, GIL limitations |

## Naming Convention

Derive all identifiers from the `backend_name` argument:

| Concept | Rule | Example (`backend_name` = "code verify") |
|---------|------|------------------------------------------|
| action_name | lowercase, underscores | `code_verify` |
| Backend class | PascalCase + `Backend` | `CodeVerifyBackend` |
| Python module | `{action_name}.py` | `code_verify.py` |
| lifecycle category | same as action_name | `code_verify` |

**Rust-specific** (only when `backend_type=rust`):

| Concept | Rule | Example |
|---------|------|----------|
| Rust function | same as action_name | `code_verify` |
| Request struct | PascalCase + `Request` | `CodeVerifyRequest` |
| Response struct | PascalCase + `Response` | `CodeVerifyResponse` |

---

## 1. Architecture Overview

Both backend types follow the same execution flow:

```
agent-sec-cli  ──→  security_middleware.invoke("{action_name}", **kwargs)
                            │
                            ├─ router.get_backend("{action_name}")
                            │      └─ _REGISTRY["{action_name}"] → "security_middleware.backends.{action_name}"
                            │      └─ lazy import → {ActionName}Backend()
                            │
                            ├─ backend.execute(ctx, **kwargs) → ActionResult
                            │      │
                            │      ├─ [Rust]  import rust_backends ← PyO3 .so
                            │      │          rust_backends.{action_name}(json_in) → json_out
                            │      │
                            │      └─ [Python] import {module_path}
                            │                  module.function(**kwargs) → result
                            │
                            └─ lifecycle.post_action() → SecurityEvent → JSONL
```

**Key contract**: Every backend is a Python class with an `execute(ctx, **kwargs) → ActionResult`
method. The implementation language (Rust/Python) is an **implementation detail** — the middleware
never calls Rust or module functions directly.

---

## 2. Create the Python Backend Wrapper

The Python backend wrapper is the **unified interface** that the middleware calls. It delegates
to either Rust or Python implementation based on `backend_type`.

### 2.1 Choose Template

- **For `backend_type=rust`**: Use `templates/rust_backend.py`
- **For `backend_type=python`**: Use `templates/python_backend.py`

### 2.2 Create Backend File

Create `agent-sec-cli/src/agent_sec_cli/security_middleware/backends/{action_name}.py`

Copy the appropriate template and replace placeholders:
- `{backend_name}` → actual backend name (e.g., "code_verify")
- `{BackendName}` → PascalCase class name (e.g., "CodeVerify")
- `{action_name}` → action name for Rust calls (e.g., "code_verify")
- `{module_path}` → Python module path (only for python type, e.g., "agent_sec_cli.code_verify.verifier")

**Convention**: Class name = PascalCase of module name + `Backend`.

> **IMPORTANT — `stdout` / `error` contract**: The CLI (`agent-sec-cli`) only
> prints `result.stdout` and `result.error`. If a backend returns an `ActionResult`
> with both `stdout` and `error` empty, the CLI produces **no output at all**.
> Every `ActionResult` **must** populate at least one of:
>
> | Field | When to set |
> |-------|-------------|
> | `stdout` | Always on success — human-readable text for the terminal |
> | `error` | Always on failure — written to stderr by the CLI |
>
> A helper like `_format_stdout()` keeps formatting in one place and makes it
> easy to test independently.

---

## 3. Register Backend in Router and Lifecycle

### 3.1 Register in Router

Edit `agent-sec-cli/src/agent_sec_cli/security_middleware/router.py` — add to `_REGISTRY`:

```python
_REGISTRY: Dict[str, str] = {
    # ... existing entries ...
    "{action_name}":   "agent_sec_cli.security_middleware.backends.{action_name}",
}
```

### 3.2 Add Lifecycle Category Mapping

Edit `agent-sec-cli/src/agent_sec_cli/security_middleware/lifecycle.py` — add to `_ACTION_CATEGORY`:

```python
_ACTION_CATEGORY: Dict[str, str] = {
    # ... existing entries ...
    "{action_name}":   "{action_name}",
}
```

### 3.3 Add CLI Entry Point

Edit `src/agent_sec_cli/cli.py` — add a new `@app.command()` function:

```python
# {action_name} subcommand
@app.command()
def {action_name}(
    param1: str = typer.Option("", "--param1", help="Parameter 1"),
    # Add more arguments as needed for the backend
):
    """{ActionName} description."""
    result = invoke("{action_name}", param1=param1)
    if result.stdout:
        typer.echo(result.stdout)
    if result.error:
        typer.echo(result.error, err=True)
    raise typer.Exit(code=result.exit_code)
```

Now callable as:

```bash
agent-sec-cli {action_name} --param1 value
```

---

## 4. Rust-Specific Steps (backend_type=rust)

> Skip this section if `backend_type=python`.

### 4.1 Create the Rust Project (if not exists)

Check whether `agent-sec-cli/src/agent_sec_cli/rust_backends/` exists. If not, create the
entire project scaffold.

#### 4.1.1 Create `Cargo.toml`

```toml
[package]
name = "rust_backends"
version = "0.1.0"
edition = "2024"
license = "Apache-2.0"

[lib]
name = "rust_backends"
crate-type = ["cdylib"]       # Produces .so for Python import

[lints.clippy]
uninlined_format_args = "deny"
redundant_closure_for_method_calls = "deny"

[dependencies]
pyo3 = { version = "0.22", features = ["extension-module"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

#### 4.1.2 Create `rust-toolchain.toml`

```toml
[toolchain]
channel = "1.93.0"
components = ["clippy", "rustfmt", "rust-src"]
```

#### 4.1.3 Create `src/lib.rs` (empty scaffold)

```rust
//! Rust backends for the agent-sec-core security middleware.
//!
//! Each exported `#[pyfunction]` follows the JSON-in / JSON-out pattern and
//! releases the GIL during computation so Python threads are not blocked.

use pyo3::prelude::*;
use serde::{Deserialize, Serialize};

// --- backend functions are added below by the add-backend skill ---

#[pymodule]
fn rust_backends(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Functions are registered here as they are added.
    Ok(())
}
```

### 4.2 Add the Rust Function

#### 4.2.1 Rust Coding Rules

| Rule | Reason |
|------|--------|
| Use `py: Python<'_>` param, not `Python::with_gil()` | GIL is already held in `#[pyfunction]` |
| Use `py.allow_threads(\|\| { ... })` | Releases GIL for concurrency |
| No Python API calls inside `allow_threads` | GIL not held — would segfault |
| Use `&Bound<'_, PyModule>` in `#[pymodule]` | PyO3 0.22+ API |
| Return `PyResult<String>` (JSON) | Clean boundary, no Python objects in Rust |

#### 4.2.2 Add Request/Response Structs and Logic to `lib.rs`

Insert **above** the `#[pymodule]` block. Follow this pattern exactly, replacing
placeholders with the actual backend name:

```rust
// ---------------------------------------------------------------------------
// {action_name}
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct {ActionName}Request {
    // Add domain-specific fields here
}

#[derive(Serialize)]
struct {ActionName}Response {
    // Add domain-specific output fields here
}

/// Pure Rust logic — no Python API calls.
fn do_{action_name}(req: &{ActionName}Request) -> Result<{ActionName}Response, String> {
    // Implement domain logic here
    todo!("implement {action_name} logic")
}

#[pyfunction]
fn {action_name}(py: Python<'_>, request_json: &str) -> PyResult<String> {
    let req: {ActionName}Request = serde_json::from_str(request_json)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
            format!("Invalid JSON: {e}")
        ))?;

    py.allow_threads(|| {
        let resp = do_{action_name}(&req)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e))?;
        serde_json::to_string(&resp)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                format!("Serialization failed: {e}")
            ))
    })
}
```

#### 4.2.3 Register in `#[pymodule]`

Add this line inside the `rust_backends` pymodule function:

```rust
m.add_function(wrap_pyfunction!({action_name}, m)?)?;
```

#### 4.2.4 Add Rust Unit Tests

Append a `#[cfg(test)]` module at the bottom of `lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn {action_name}_basic() {
        let req = {ActionName}Request { /* test fields */ };
        let resp = do_{action_name}(&req).unwrap();
        // Add assertions here
    }
}
```

### 4.3 Build and Test

#### 4.3.1 Rust Unit Tests

```bash
cd agent-sec-cli/src/agent_sec_cli/rust_backends
cargo test
```

#### 4.3.2 Development Build (maturin)

```bash
cd agent-sec-cli/src/agent_sec_cli/rust_backends
pip install maturin
maturin develop
python3 python/test_integration.py
```

#### 4.3.3 Where the .so Goes

| Context | Location | How |
|---------|----------|-----|
| **Dev** (`maturin develop`) | Python site-packages | Automatic — importable as `import rust_backends` |
| **Manual** | Next to Python scripts | Copy to `agent-sec-cli/src/agent_sec_cli/rust_ext/rust_backends.so` |
| **RPM** | `%{_datadir}/anolisa/skills/agent-sec-core/scripts/rust_ext/` | Via spec `%install` section |

#### 4.3.4 Deploy the `.so` for Local Testing

After `cargo build --release`, copy the shared library to the location the Python
backend expects:

```bash
# From agent-sec-core/
mkdir -p agent-sec-cli/src/agent_sec_cli/rust_ext
cp agent-sec-cli/src/agent_sec_cli/rust_backends/target/release/librust_backends.so agent-sec-cli/src/agent_sec_cli/rust_ext/rust_backends.so
```

### 4.4 Update Build System

#### 4.4.1 Add Makefile Target

Append to `agent-sec-core/Makefile` (in the BUILD section, after `build-sandbox`):

```makefile
.PHONY: build-rust-backends
build-rust-backends: ## Build Rust PyO3 backends (.so)
	cd rust_backends && cargo build --release
```

#### 4.4.2 Update RPM Spec

In `agent-sec-core/agent-sec-core.spec`:

- Add `BuildRequires: python3-devel` in the build dependencies section
- In `%build`, after `make build-sandbox`, add: `make build-rust-backends`
- In `%install`, add:
  ```
  install -d -m 0755 $RPM_BUILD_ROOT%{_datadir}/anolisa/skills/agent-sec-core/scripts/rust_ext
  install -p -m 0755 rust_backends/target/release/librust_backends.so \
      $RPM_BUILD_ROOT%{_datadir}/anolisa/skills/agent-sec-core/scripts/rust_ext/rust_backends.so
  ```

---

## 5. Python-Specific Steps (backend_type=python)

> Skip this section if `backend_type=rust`.

### 5.1 Create Python Module

Create the Python module at `{module_path}` (e.g., `agent_sec_cli/code_verify/verifier.py`).

**Example Structure:**

```
agent_sec_cli/{backend_name}/
├── __init__.py
├── verifier.py      # Core logic
├── config.py        # Configuration (optional)
└── tests/
    └── test_verifier.py
```

### 5.2 Implement Core Logic

Implement the main function in your module (e.g., `verifier.py`):

```python
"""{backend_name} — core logic."""

def verify(**kwargs):
    """Main verification logic.
    
    Args:
        **kwargs: Parameters passed from CLI/backend.
    
    Returns:
        dict with 'success', 'output', 'data' keys.
    """
    # Implement domain logic here
    return {
        "success": True,
        "output": "Verification passed",
        "data": {"checked": 10, "passed": 10},
    }
```

### 5.3 Update Backend Wrapper

In `security_middleware/backends/{action_name}.py`, update the `_run` method
to call your module's function:

```python
@staticmethod
def _run(module, **kwargs) -> ActionResult:
    """Execute the module logic."""
    result = module.verify(**kwargs)
    return ActionResult(
        success=result["success"],
        stdout=result["output"],
        data=result["data"],
        exit_code=0 if result["success"] else 1,
    )
```

### 5.4 Python Unit Tests

Create tests in `{module_path}/tests/`:

```python
import pytest
from agent_sec_cli.{backend_name}.verifier import verify

def test_verify_basic():
    result = verify(param1="value")
    assert result["success"] is True
    assert result["data"]["checked"] > 0
```

Run tests:

```bash
pytest agent_sec_cli/{backend_name}/tests/
```

---

## 6. Testing (Both Types)

### 6.1 Integration Tests

**For Rust backends**, create `agent-sec-cli/src/agent_sec_cli/rust_backends/python/test_integration.py`:

```python
#!/usr/bin/env python3
"""Standalone integration test for rust_backends.{action_name}.

Run after ``maturin develop``:
    python3 python/test_integration.py
"""
import json
import sys


def main() -> int:
    try:
        import rust_backends
    except ImportError:
        print("SKIP: rust_backends not importable (run `maturin develop` first)")
        return 0

    errors = 0

    # 1. Basic test
    req = json.dumps({/* test input */})
    resp = json.loads(rust_backends.{action_name}(req))
    # Add assertions
    print(f"PASS: basic test (resp={resp})")

    # 2. Invalid JSON
    try:
        rust_backends.{action_name}("{{bad json")
        print("FAIL: expected ValueError for invalid JSON")
        errors += 1
    except ValueError:
        print("PASS: invalid JSON raises ValueError")

    if errors:
        print(f"\n{{errors}} test(s) FAILED")
        return 1
    print("\nAll tests passed!")
    return 0


if __name__ == "__main__":
    sys.exit(main())
```

**For Python backends**, create tests in `{module_path}/tests/` (see Section 5.4).

### 6.2 E2E CLI Tests

This verifies the **full call chain**: CLI → `invoke()` → router → backend → result → CLI output.

#### 6.2.1 CLI Smoke Test

```bash
agent-sec-cli {action_name} --param1 test
```

Expected behaviour:
- Exit code `0` on success.
- Output contains expected result (no errors).

#### 6.2.2 Verify Rust Path (Rust backends only)

```bash
# From agent-sec-core/
agent-sec-cli {action_name} --param1 value
```

Expected behaviour:
- Exit code `0` on success.
- `result.data` contains the Rust backend's JSON response fields.
- No `"python fallback"` note in the output (confirms the Rust path was used).

#### 6.2.3 Verify Python Fallback Path (Rust backends only)

Temporarily remove or rename the `.so` and re-run the same command:

```bash
mv src/agent_sec_cli/rust_ext/rust_backends.so src/agent_sec_cli/rust_ext/rust_backends.so.bak
agent-sec-cli {action_name} --param1 value
mv src/agent_sec_cli/rust_ext/rust_backends.so.bak src/agent_sec_cli/rust_ext/rust_backends.so
```

Expected behaviour:
- Exit code `0` (fallback succeeds).
- Output contains `"python fallback — Rust extension not available"` in the data.

#### 6.2.4 Negative / Error-Path Test

Pass invalid or adversarial input to confirm the backend returns a non-zero exit
code and a meaningful error message:

```bash
# Example: omit required fields or pass unexpected values
agent-sec-cli {action_name}
```

Expected behaviour:
- The CLI exits with a non-zero code **or** returns an error message on stderr
  if the backend rejects the input.

---

## 7. Checklist

### Common (Both Types)

```
- [ ] Python backend wrapper created in security_middleware/backends/{action_name}.py
- [ ] Class name follows PascalCase + Backend convention
- [ ] Action registered in router._REGISTRY
- [ ] Category mapped in lifecycle._ACTION_CATEGORY
- [ ] CLI command added to `src/agent_sec_cli/cli.py`
- [ ] stdout/error contract satisfied (ActionResult always has output)
- [ ] Unit tests created and pass
- [ ] E2E CLI test passes
```

### Rust-Specific (backend_type=rust)

```
- [ ] rust_backends/ project created (or already exists)
- [ ] Cargo.toml: edition = "2024", crate-type = ["cdylib"]
- [ ] rust-toolchain.toml pins 1.93.0
- [ ] Rust function uses JSON-in/JSON-out boundary
- [ ] GIL released with py.allow_threads()
- [ ] No Python API calls inside allow_threads block
- [ ] PyO3 0.22+ API (Bound<'_, PyModule>)
- [ ] Function registered in #[pymodule] via m.add_function()
- [ ] Rust unit tests added and pass (cargo test)
- [ ] Makefile build-rust-backends target exists
- [ ] RPM spec includes python3-devel BuildRequires
- [ ] E2E: .so deployed to agent-sec-cli/src/agent_sec_cli/rust_ext/, CLI returns Rust result
- [ ] E2E: .so removed, CLI returns Python fallback result
```

### Python-Specific (backend_type=python)

```
- [ ] Python module created at {module_path}
- [ ] Core logic implemented (e.g., verify(), scan(), etc.)
- [ ] Backend wrapper _run() method calls module function
- [ ] Python unit tests pass (pytest)
- [ ] Module structure follows asset_verify pattern
```
