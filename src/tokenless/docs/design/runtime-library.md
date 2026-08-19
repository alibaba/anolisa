# Tokenless Runtime Library

[中文版](runtime-library_zh.md)

## Purpose

`anolisa-tokenless` is the framework-neutral, in-process Tokenless SDK. A platform wheel contains
the PyO3 runtime and the pinned RTK executable, so Python applications do not require `tokenless`
or `rtk` on `PATH`.

The public `TokenlessSdk` maps four host lifecycle boundaries to Tokenless behavior:

| Lifecycle | Behavior |
|---|---|
| `before_model` | Reversible Function Calling schema compression and conditional retrieve-tool publication |
| `before_tool_call` | RTK rewrite for an adapter-declared command field |
| `after_tool_call` | Response compression, TOON selection, and environment-error guidance |
| `retrieve` | Marker-authorized, byte-exact Stash retrieval |

Tool Ready is product-wide hard-disabled and is not part of this API.

## Contracts and state

Adapters translate framework objects into immutable `ModelRequest`, `ToolCall`, `ToolResult`, and
`RetrieveRequest` values. `Attribution` requires agent and session identifiers; tool lifecycles also
require a tool-use identifier. OpenAI Function Calling JSON is the normalized schema representation,
but the lifecycle envelope is a Tokenless protocol rather than an OpenAI request.

`tokenless-runtime` owns one SQLite Stash and statistics recorder. Schema and response compression
share that Stash and roll back keys whenever a candidate is discarded. TOON is linked as a Rust
library and never starts a process. RTK is used only when an adapter supplies `command_field`; every
rewritten wrapper is anchored to the packaged executable and carries per-execution attribution.

The SDK never stores a process-global current session. `before_model` returns the exact visible
marker set, the adapter retains it in framework session state, and `retrieve` accepts only a hash in
that set. Host applications retain raw tool values for UI and business logic and pass only a copied,
model-visible text value through `after_tool_call`.

Invalid inputs, missing packaged RTK, attachment failures, and tool-name collisions fail fast.
Compression and per-call rewrite failures preserve the original value and emit a warning because
they are optional optimizations. A candidate is applied only when it is strictly smaller; schema and
response truncation must also remain retrievable.

## Packaging and validation

`make python-wheel` builds the pinned RTK version, stages it as
`anolisa_tokenless/_bin/rtk`, and creates a CPython 3.11 stable-ABI platform wheel. Cross-platform
builders may set `PYTHON_RTK_BINARY` to the RTK executable built for the same wheel target.
`make test-python-runtime` installs the wheel in a fresh environment and exercises all four
lifecycles without relying on a system RTK binary.

`anolisa-tokenless-agentscope` supports AgentScope 1.0.11 through 1.0.x and 2.0.x. The 1.x adapter
uses a Tokenless Toolkit, a model proxy, and public instance hooks. The 2.x adapter uses
`on_model_call` and `on_acting`; 2.0.0 keeps marker state in the paired Middleware/Tool, while later
versions also persist it in `AgentState.middle_context`. Both expose the complete SDK; 2.0.0 supports
direct Agent construction, while App integration starts at 2.0.1.
