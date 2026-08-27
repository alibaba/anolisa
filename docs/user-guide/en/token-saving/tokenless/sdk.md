# Tokenless Python SDK

[中文版](../../../zh/token-saving/tokenless/sdk.md)

Tokenless provides two Python SDK layers:

| Layer | Package | Purpose |
|-------|---------|---------|
| Framework-neutral SDK | `anolisa-tokenless` | Integrate any Agent lifecycle, invoke individual Tokenless operations, or query statistics |
| AgentScope integration | `anolisa-tokenless-agentscope` | Attach the framework-neutral SDK to the supported AgentScope 1.x and 2.x lifecycle APIs |

The AgentScope layer depends on the exact same version of the framework-neutral SDK and delegates
Tokenless operations to it; it is not a separate compression implementation. This page introduces
both layers. Detailed AgentScope usage lives in the
[AgentScope SDK integration](sdk/agentscope.md) child document, while product plugins remain in
[Agent integration](framework-integration.md).

## Layer 1: Framework-neutral SDK

The `anolisa-tokenless` wheel lets Python applications run Tokenless in process. Use
`TokenlessSdk` when integrating Tokenless into an Agent lifecycle. Use `TokenlessRuntime` when you
do not need lifecycle integration and only want to invoke a specific operation, such as compressing
one response or retrieving one Stash entry. Use `TokenlessStats` only for statistics queries.

### Install from GitHub Release

Official SDK wheels are attached to Tokenless GitHub Releases starting with
[v0.7.14](https://github.com/alibaba/anolisa/releases/tag/tokenless/v0.7.14). They require
CPython 3.11 or later. Select the native `anolisa-tokenless` wheel for the target system:

| System | Release asset |
|--------|---------------|
| Linux x86_64 | `anolisa_tokenless-<version>-cp311-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl` |
| Linux aarch64 | `anolisa_tokenless-<version>-cp311-abi3-manylinux_2_17_aarch64.manylinux2014_aarch64.whl` |
| macOS Apple silicon | `anolisa_tokenless-<version>-cp311-abi3-macosx_11_0_arm64.whl` |

For example, install v0.7.14 on Linux x86_64 into a virtual environment:

```bash
python3 -m venv .venv
. .venv/bin/activate
python -m pip install \
  "https://github.com/alibaba/anolisa/releases/download/tokenless/v0.7.14/anolisa_tokenless-0.7.14-cp311-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl"
```

The Linux assets target glibc-based distributions compatible with `manylinux_2_17`; they do not
support Alpine Linux or other musl-based distributions. The Release also includes
`SHA256SUMS-python-wheels.txt` for download verification.

The wheel contains the native Tokenless runtime and the matching RTK executable. It does not need
the `tokenless` CLI, a system RTK binary, or a separate TOON executable.

### Build from source

Source builds in this repository are Linux-only. Build the SDK from the Tokenless component
directory with a discoverable CPython 3.11 or later development environment:

```bash
make python-wheel
python3 -m venv /tmp/tokenless-sdk
/tmp/tokenless-sdk/bin/pip install target/wheels/anolisa_tokenless-*.whl
```

`make python-wheel` uses `uvx` to provide Maturin by default. Install
[`uv`](https://docs.astral.sh/uv/) first, or use a compatible Maturin already on `PATH`:

```bash
make python-wheel MATURIN=maturin
```

Pip installs the wheel in unpacked form, which gives the packaged RTK executable the stable path
required by command rewriting.

### Choose an API

| API | Role | Use it for |
|-----|------|------------|
| `TokenlessSdk` | Lifecycle integration | Connect Tokenless to the model-call and tool-call stages of an Agent framework |
| `TokenlessRuntime` | Individual operations | Directly compress one schema, response, or TOON payload, or retrieve one Stash entry |
| `TokenlessStats` | Statistics queries | Read status, summaries, recent records, record details, diffs, and session comparisons |

`TokenlessSdk` is the recommended integration surface for a new agent framework. It owns one
`TokenlessRuntime`, exposes the same state directory through `sdk.runtime.data_dir`, and creates
`sdk.stats` lazily when statistics are queried.

### Complete lifecycle example

This example compresses a model-visible tool schema, compresses a successful tool result, and
recovers one marker-authorized Stash payload. It disables RTK and TOON so the example focuses on the
schema/response lifecycle and has no command-execution dependency.

```python
import asyncio
import json
import tempfile
from pathlib import Path

from anolisa_tokenless import (
    Attribution,
    ModelRequest,
    RetrieveRequest,
    TokenlessConfig,
    TokenlessSdk,
    ToolCall,
    ToolResult,
    ToolStatus,
)


async def main() -> None:
    with tempfile.TemporaryDirectory(prefix="tokenless-sdk-") as data_dir:
        sdk = TokenlessSdk(
            TokenlessConfig(
                data_dir=Path(data_dir),
                mode="aggressive",
                min_chars=0,
                rtk_enabled=False,
                toon_enabled=False,
            )
        )
        model_attribution = Attribution("my-agent", "session-42")
        tool = {
            "type": "function",
            "function": {
                "name": "lookup",
                "description": "Detailed lookup instructions. " * 100,
                "parameters": {"type": "object", "properties": {}},
            },
        }

        model_request = await sdk.before_model(
            ModelRequest((tool,), "", model_attribution)
        )
        print([item.get("function", {}).get("name") for item in model_request.tools])

        call = ToolCall(
            "api",
            {},
            Attribution("my-agent", "session-42", "tool-7"),
        )
        original = json.dumps(
            {"items": [{"name": "same", "value": index} for index in range(300)]}
        )
        result = await sdk.after_tool_call(
            ToolResult(call, original, ToolStatus.SUCCESS)
        )
        print(result.transformed, len(original), len(result.content))

        visible_markers = sdk.extract_markers(result.content)
        if visible_markers:
            marker_hash = next(iter(visible_markers))
            recovered = await sdk.retrieve(
                RetrieveRequest(marker_hash, visible_markers, model_attribution)
            )
            print(f"recovered {len(recovered)} characters")


asyncio.run(main())
```

`TemporaryDirectory` keeps the example self-contained and deletes its state on exit. In production,
use a stable, writable absolute `data_dir`, with a different directory for every tenant or security
boundary.

The SDK treats lifecycle values as immutable contracts: it copies tool schemas and argument maps
instead of modifying caller-owned objects. Keep the returned `ModelRequest`, `ToolCall`, and
`ToolResult`; the original values do not reflect transformations.

### The four lifecycle seams

#### Before a model call

```python
request = await sdk.before_model(
    ModelRequest(tuple(model_tools), visible_context, attribution)
)
```

`before_model()` compresses OpenAI Function Calling tools, scans the transformed tools and visible
context for `<<tokenless:HASH>>` markers, and publishes the configured retrieval schema only when at
least one marker is visible. The name defaults to `tokenless_retrieve` and is reserved; set a unique
`retrieve_tool_name` when the application already owns that name.

#### Before a tool call

```python
call = await sdk.before_tool_call(
    ToolCall(
        "shell",
        {"command": "grep needle large.log"},
        Attribution("my-agent", "session-42", "tool-8"),
        command_field="command",
    )
)
```

The SDK considers only the explicitly named `command_field`. If RTK has a rewrite, the returned
arguments contain the packaged RTK path and per-call attribution environment. Execute the returned
arguments, and preserve `call.rewritten` when constructing the final `ToolResult`; rewritten calls
skip result compression because RTK already reduced their output at the source.

#### After a tool call

```python
result = await sdk.after_tool_call(
    ToolResult(call, model_visible_text, ToolStatus.SUCCESS)
)
```

Successful, non-rewritten final text can pass through response compression and then TOON. The SDK
keeps the original unless the UTF-8 result is strictly smaller. `ERROR` results are not compressed;
recognized dependency, permission, path, network, and package failures may instead receive
`additional_context` guidance. `INTERRUPTED` and `DENIED` results pass through.

#### Marker-scoped retrieval

```python
markers = sdk.extract_markers(model_visible_context)
payload = await sdk.retrieve(
    RetrieveRequest(marker_hash, markers, attribution)
)
```

Retrieval accepts exactly 24 hexadecimal characters and only a hash present in the supplied visible
marker set. Treat that set as model-call state: recompute it from the actual model-visible context
instead of accumulating every marker ever seen in a session. Retrieval also requires the same
Tokenless data directory and an unexpired Stash entry.

### Configuration

```python
config = TokenlessConfig(
    mode="balanced",
    data_dir="/absolute/path/to/tenant-tokenless-data",
    min_chars=200,
    excluded_tools={"read_database"},
    retrieve_tool_name="tokenless_retrieve",
    schema_compression_enabled=True,
    response_compression_enabled=True,
    toon_enabled=True,
    rtk_enabled=True,
)
```

| Mode | Content-retrieval tools | Other tools |
|------|-------------------------|-------------|
| `conservative` | Compress | 1 MiB strings, 65,536 array items, depth 32 |
| `balanced` (default) | Skip | Shell: 65,536 / 128 / depth 8; others use conservative limits |
| `aggressive` | Skip | 4,096-character strings, 32 array items, depth 8 |

`data_dir` must be absolute and writable. Use a different directory for every tenant or security
boundary; `TOKENLESS_DATA_DIR` is only a process-wide fallback. `excluded_tools` is added to
Tokenless's built-in exclusions, and the retrieval tool is always excluded from response
optimization.

The SDK lifecycle currently uses the schema/response/TOON Runtime operations. The CLI and shared
Agent hooks additionally expose the content-aware Pipeline, including build/log compression; that
Pipeline is not a `TokenlessSdk` method yet.

### Direct Runtime examples

Use `TokenlessRuntime` when the caller does not need `TokenlessSdk` to coordinate an Agent lifecycle
and wants to invoke Tokenless operations directly. Create one Runtime for the data directory:

```python
import json
import re
from anolisa_tokenless import TokenlessRuntime

runtime = TokenlessRuntime("/absolute/path/to/tokenless-data")
```

#### Compress a response

```python
original_response = json.dumps(
    {"items": [f"record-{index:04d}" for index in range(200)]}
)
response_result = runtime.compress_response(
    original_response,
    truncate_arrays_at=32,
    agent_id="my-agent",
    session_id="session-42",
    tool_use_id="tool-7",
    require_reversible=True,
)
model_visible_response = response_result.output
print(response_result.disposition, response_result.before_tokens, response_result.after_tokens)
```

#### Compress a tool schema

```python
tool_schema = {
    "type": "function",
    "function": {
        "name": "lookup",
        "description": "Detailed lookup instructions. " * 100,
        "parameters": {"type": "object", "properties": {}},
    },
}
schema_result = runtime.compress_schema(
    json.dumps(tool_schema),
    agent_id="my-agent",
    session_id="session-42",
)
model_visible_schema = json.loads(schema_result.output)
```

#### Encode JSON as TOON

```python
records = {
    "items": [
        {"name": f"item-{index:04d}", "status": "ready"}
        for index in range(100)
    ]
}
toon_result = runtime.compress_toon(
    json.dumps(records),
    agent_id="my-agent",
    session_id="session-42",
    tool_use_id="tool-8",
)
model_visible_text = toon_result.output
```

`compress_toon()` keeps the original JSON when TOON would not reduce the estimated token count.

#### Retrieve stashed content

Response or schema compression may place omitted content in Stash and leave a marker in the output:

```python
marker = re.search(r"<<tokenless:([0-9a-f]{24})>>", response_result.output)
if marker is not None:
    recovered_content = runtime.retrieve(marker.group(0))
    print(recovered_content)
```

`retrieve()` accepts either the complete marker or its 24-character hash. Direct Runtime callers
must decide which markers are authorized for retrieval; `TokenlessSdk.retrieve()` applies the SDK's
model-visible marker check.

Runtime inputs and outputs are strings. Use each `CompressionResult.output` as the exact downstream
value, then inspect its `disposition`, token counts, and Stash fields when the caller needs to
understand whether and how the input changed.

### Query statistics

```python
from anolisa_tokenless import TokenlessStats

stats = TokenlessStats("/absolute/path/to/tokenless-data")
status = stats.status
summary = stats.summary()
recent = stats.list(limit=20)

print(status.database_path, summary.total.tokens_saved)
if recent:
    record = stats.show(recent[0].id)
    change = stats.diff(record_id=record.id)
```

Use `stats.diff(session_id="...")` for a session overview,
`stats.diff(session_id="...", tool_use_id="...")` for one tool lifecycle, and
`stats.compare("baseline-session", "tokenless-session")` for a dry-run versus active comparison.

Token counts are estimates, and only operations with positive savings are recorded. `list()`,
`summary()`, and `compare()` do not return stored content; `show()` and detailed `diff()` results may
contain sensitive tool input or output. The public query API does not clear data or change settings,
but opening it may create or migrate `stats.db`, so the selected data directory must be writable.

## Layer 2: AgentScope integration

`anolisa-tokenless-agentscope` maps the framework-neutral lifecycle to AgentScope. Application code
uses `TokenlessAgentScope` instead of calling `before_model()`, `before_tool_call()`,
`after_tool_call()`, and `retrieve()` itself. The integration also carries AgentScope session and
tool-call attribution into the generic SDK.

See [AgentScope SDK integration](sdk/agentscope.md) for supported versions, build and installation,
complete 1.x/2.x/App examples, configuration, retrieval boundaries, and validation. Product adapters
such as Claude Code and OpenCode are separate from both Python SDK layers.

## Validate both SDK layers

Build the framework-neutral wheel and run its installed-wheel tests:

```bash
make python-wheel
make test-python-runtime
```

Validate the AgentScope layer with the commands in its
[child document](sdk/agentscope.md#validate-the-integration).

## Related documents

- [Agent integration](framework-integration.md)
- [AgentScope SDK integration](sdk/agentscope.md)
- [CLI reference](cli-reference.md)
- [Measuring savings](measuring-savings.md)
- [Configuration and data privacy](configuration-and-privacy.md)
- [Runtime design](../../../../../src/tokenless/docs/design/runtime-library.md)
