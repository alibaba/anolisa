# Tokenless Runtime Library

[中文版](runtime-library_zh.md)

## Purpose

Framework integrations need response compression and Stash retrieval without starting a
`tokenless` subprocess for every tool result. The runtime library provides that in-process API
while keeping the CLI and language bindings on the same compression implementation.

This design introduces two public layers:

- `tokenless-runtime`, a reusable Rust crate that owns state, policy, and attribution.
- `anolisa-tokenless`, a native Python package that exposes the runtime through PyO3.

The first Python surface covers JSON response compression and Stash retrieval. It also exposes a
framework-neutral `TokenlessConfig` and `ToolResponseCompressor` so framework packages share policy,
type preservation, savings checks, and marker-scoped retrieval without sharing lifecycle code.
Schema compression, TOON encoding, RTK command rewriting, MCP, and framework-specific middleware
remain outside the library. The separate `tokenless_agentscope` package consumes this API and owns
the AgentScope lifecycle contract.

## Architecture

```text
tokenless-schema ─┐
tokenless-ccr ────┼──> tokenless-runtime ──> tokenless CLI
tokenless-stats ──┘              └─────────> PyO3 ──> anolisa_tokenless
                                                       └──> tokenless_agentscope
```

`tokenless-runtime` is the high-level application API. It opens the Stash and statistics databases
once, applies one policy decision per call, and attaches the supplied agent, session, and tool-call
identifiers to statistics. The CLI delegates response compression and retrieval to the same
functions, so the Python package does not fork the compression algorithm.

The Python extension is built for the CPython 3.11 stable ABI. A wheel is still specific to its
operating system and CPU architecture, but one wheel can be used by CPython 3.11 and later on the
same platform. The extension releases the Python GIL during compression and retrieval. The shared
SQLite state uses the existing synchronized Stash and statistics implementations, so one runtime
instance can serve concurrent tool calls without global attribution variables.

## Runtime contract

Construction accepts an explicit data directory. When it is absent, the runtime uses
`TOKENLESS_DATA_DIR` and then the passwd-backed home directory, following the same path policy as
the CLI. Each user or tenant should receive a separate directory because Stash markers grant access
to data stored in that directory.

`compress_response` accepts a JSON string and returns a structured result containing:

- caller-visible output and the calculated compressed candidate;
- the disposition: `applied`, `dry-run`, `no-savings`, or
  `reversibility-unavailable`;
- estimated token counts, Stash write metrics, and the number of truncations
  without a retrievable marker.

The Python binding defaults `require_reversible` to `True`. If Stash is requested but cannot be
opened or written, or a configured limit cannot fit a retrieval marker, the runtime returns the
original response with the `reversibility-unavailable` disposition. This fail-open behavior
prevents an embedding framework from silently accepting an unrecoverable truncation. The CLI
retains its existing behavior and may emit the lossy candidate after a Stash failure, together with
its existing warning.

Invalid JSON and invalid state paths are explicit errors. No savings and reversible-storage
failures are policy outcomes rather than exceptions. Retrieval accepts a bare 24-character
hexadecimal hash or a string containing a Tokenless marker and returns the stored UTF-8 payload
unchanged.

## Packaging and validation

`make python-wheel` builds `anolisa-tokenless` into `target/wheels/` with Maturin. The custom
`python-release` Cargo profile uses unwind panic semantics because an embedded interpreter must not
be aborted by a Rust panic. `make test-python-runtime` installs the wheel in a fresh virtual
environment and validates compression, byte-exact Unicode retrieval, error mapping, concurrent
calls, and per-call statistics attribution.

`python/agentscope/` is an independent pure-Python distribution supporting AgentScope
1.0.11 through 1.0.x and AgentScope 2.0.x. Its stable `TokenlessAgentScope` entry point selects one
of two lifecycle backends: AgentScope 1.x chains Toolkit postprocessors and binds retrieval to Agent
memory, while AgentScope 2.x supplies a middleware and explicit retrieval Tool during Agent
construction. AgentScope 2.0.0 supports direct Agents; App integration starts at 2.0.1 because
2.0.0 has no App-level Agent middleware or Tool injection.

`make agentscope-wheel` builds it into the same `target/wheels/` output directory, and
`make test-agentscope-integration` validates compression and byte-exact retrieval against 1.0.11,
the latest 1.0.x, 2.0.0, the 2.0.1 App boundary, the 2.0.3 Tool ABI boundary, and the latest 2.0.x
with the same-version native runtime wheel.

This repository builds and tests both Python distributions but does not publish them to PyPI.
Publication requires the release pipeline to build each supported platform wheel, sign or attest
the artifacts according to release policy, and upload them with release credentials.

## Compatibility and evolution

The Rust API and Python package begin as an alpha surface. New in-process framework integrations
should depend on the Python API instead of invoking the CLI when they run in a compatible Python
process. Existing CLI and hook integrations remain supported and do not need to migrate.

The AgentScope package owns framework details such as streaming block preservation, lifecycle
attachment, and extraction of model-visible state. The Python runtime package owns shared
compression modes, tool policy, type/savings checks, and marker authorization. This boundary keeps
patch-version differences inside the framework package and makes the common policy reusable by
future integrations.
