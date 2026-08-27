# Tokenless Python SDK

[English](../../../en/token-saving/tokenless/sdk.md)

Tokenless 提供两层 Python SDK：

| 层级 | 包 | 用途 |
|------|----|------|
| 通用 SDK | `anolisa-tokenless` | 接入任意 Agent 生命周期、执行单项 Tokenless 操作或查询统计 |
| AgentScope 集成 | `anolisa-tokenless-agentscope` | 把通用 SDK 挂载到受支持的 AgentScope 1.x 和 2.x 生命周期 API |

AgentScope 层依赖完全相同版本的通用 SDK，并把 Tokenless 操作交给通用 SDK 执行；
它不是另一套压缩实现。本页介绍两层的关系。AgentScope 详细用法放在
[AgentScope SDK 集成](sdk/agentscope.md) 子文档，产品 Plugin 仍放在
[Agent 集成](framework-integration.md)。

## 第一层：通用 SDK

`anolisa-tokenless` Wheel 让 Python 应用可以在进程内运行 Tokenless。把 Tokenless 接入
Agent 生命周期时使用 `TokenlessSdk`。不需要接入生命周期、只想执行某一项具体操作时
使用 `TokenlessRuntime`，例如单独压缩一个响应或恢复一条 Stash 内容。只查询统计时
使用 `TokenlessStats`。

### 从 GitHub Release 安装

从 [v0.7.14](https://github.com/alibaba/anolisa/releases/tag/tokenless/v0.7.14) 开始，
Tokenless GitHub Release 会附带官方 SDK Wheel。Wheel 需要 CPython 3.11 或更高版本，
请根据目标系统选择原生 `anolisa-tokenless` Wheel：

| 系统 | Release 产物 |
|------|--------------|
| Linux x86_64 | `anolisa_tokenless-<version>-cp311-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl` |
| Linux aarch64 | `anolisa_tokenless-<version>-cp311-abi3-manylinux_2_17_aarch64.manylinux2014_aarch64.whl` |
| macOS Apple 芯片 | `anolisa_tokenless-<version>-cp311-abi3-macosx_11_0_arm64.whl` |

例如，在 Linux x86_64 上把 v0.7.14 安装到虚拟环境：

```bash
python3 -m venv .venv
. .venv/bin/activate
python -m pip install \
  "https://github.com/alibaba/anolisa/releases/download/tokenless/v0.7.14/anolisa_tokenless-0.7.14-cp311-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl"
```

Linux 产物面向兼容 `manylinux_2_17` 的 glibc 发行版，不支持 Alpine Linux 等 musl 发行版。
Release 同时提供 `SHA256SUMS-python-wheels.txt`，可用于校验下载内容。

Wheel 包含原生 Tokenless Runtime 和匹配的 RTK 可执行文件，不需要 `tokenless` CLI、系统
RTK 或独立 TOON 可执行文件。

### 从源码构建

本仓库仅支持在 Linux 上从源码构建。请在 Tokenless 组件目录中构建，并确保系统可发现
CPython 3.11 或更高版本的开发环境：

```bash
make python-wheel
python3 -m venv /tmp/tokenless-sdk
/tmp/tokenless-sdk/bin/pip install target/wheels/anolisa_tokenless-*.whl
```

`make python-wheel` 默认通过 `uvx` 提供 Maturin。请先安装
[`uv`](https://docs.astral.sh/uv/)，也可以直接使用 `PATH` 中已有的兼容 Maturin：

```bash
make python-wheel MATURIN=maturin
```

Pip 会以展开形式安装 Wheel，从而为命令改写提供 Wheel 内置 RTK 所需的稳定可执行路径。

### 选择 API

| API | 职责 | 适用场景 |
|-----|------|----------|
| `TokenlessSdk` | 生命周期集成 | 把 Tokenless 接入 Agent 框架的 Model 调用和工具调用阶段 |
| `TokenlessRuntime` | 单项操作 | 直接压缩一个 Schema、响应或 TOON Payload，或恢复一条 Stash 内容 |
| `TokenlessStats` | 统计查询 | 读取状态、汇总、最近记录、记录详情、Diff 和 Session 对比 |

新接入 Agent 框架时建议使用 `TokenlessSdk`。它持有一个 `TokenlessRuntime`，通过
`sdk.runtime.data_dir` 暴露相同状态目录，并在查询统计时延迟创建 `sdk.stats`。

### 完整生命周期示例

下面的示例会压缩模型可见工具 Schema、压缩一次成功的工具结果，并恢复一个通过 marker
授权的 Stash Payload。示例关闭 RTK 与 TOON，以便只展示 Schema/响应生命周期且不依赖
命令执行。

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

`TemporaryDirectory` 让示例可以独立运行，并会在退出时删除状态。生产环境应使用稳定、可写
的绝对 `data_dir`，并为每个租户或安全边界使用不同目录。

SDK 把生命周期值视为不可变契约：它会复制工具 Schema 和参数 Map，而不会修改调用方持有
的对象。请使用返回的 `ModelRequest`、`ToolCall` 和 `ToolResult`；原始值不会反映转换结果。

### 四个生命周期接缝

#### Model 调用前

```python
request = await sdk.before_model(
    ModelRequest(tuple(model_tools), visible_context, attribution)
)
```

`before_model()` 压缩 OpenAI Function Calling 工具，在转换后的工具和可见 Context 中扫描
`<<tokenless:HASH>>` marker，并且只在至少一个 marker 可见时发布配置的恢复 Schema。默认
名称为 `tokenless_retrieve`，且由 Tokenless 保留；应用已经使用该名称时，应设置唯一的
`retrieve_tool_name`。

#### 工具调用前

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

SDK 只处理显式指定的 `command_field`。如果 RTK 产生改写，返回参数会包含 Wheel 内置 RTK
路径和逐调用归属环境变量。应执行返回的参数，并在构造最终 `ToolResult` 时保留
`call.rewritten`；已改写调用会跳过结果压缩，因为 RTK 已经从源头缩减输出。

#### 工具调用后

```python
result = await sdk.after_tool_call(
    ToolResult(call, model_visible_text, ToolStatus.SUCCESS)
)
```

成功且未改写的最终文本可以依次经过响应压缩和 TOON；只有 UTF-8 结果严格更小时才会替换
原文。`ERROR` 结果不会压缩；已识别的依赖、权限、路径、网络和包错误可能改为获得
`additional_context` 指引。`INTERRUPTED` 和 `DENIED` 结果原样透传。

#### 受 marker 约束的恢复

```python
markers = sdk.extract_markers(model_visible_context)
payload = await sdk.retrieve(
    RetrieveRequest(marker_hash, markers, attribution)
)
```

恢复只接受精确的 24 位十六进制字符，并要求 Hash 存在于传入的可见 marker 集合中。该集合
应当视为单次 Model 调用状态：从真实模型可见 Context 重新计算，不要持续累积 Session 中
见过的所有 marker。恢复还要求使用相同 Tokenless 数据目录，并且 Stash 条目尚未过期。

### 配置

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

| 模式 | 内容读取类工具 | 其他工具 |
|------|----------------|----------|
| `conservative` | 压缩 | 字符串 1 MiB、数组 65,536 项、深度 32 |
| `balanced`（默认） | 跳过 | Shell：65,536 / 128 / 深度 8；其他使用 conservative 限制 |
| `aggressive` | 跳过 | 字符串 4,096 字符、数组 32 项、深度 8 |

`data_dir` 必须是可写的绝对路径。每个租户或安全边界应使用不同目录；
`TOKENLESS_DATA_DIR` 只是进程级回退。`excluded_tools` 会与 Tokenless 内置排除集合合并，恢复
工具始终排除在响应优化之外。

SDK 生命周期当前使用 Schema/响应/TOON Runtime 操作。CLI 和共享 Agent Hook 还开放了
content-aware Pipeline，包括 build/log 压缩；该 Pipeline 目前还不是 `TokenlessSdk`
方法。

### Runtime 直接调用示例

不需要由 `TokenlessSdk` 协调 Agent 生命周期、希望直接执行 Tokenless 操作时，使用
`TokenlessRuntime`。先为数据目录创建一个 Runtime：

```python
import json
import re
from anolisa_tokenless import TokenlessRuntime

runtime = TokenlessRuntime("/absolute/path/to/tokenless-data")
```

#### 压缩响应

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

#### 压缩工具 Schema

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

#### 编码为 TOON

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

如果 TOON 不能减少预估 Token 数量，`compress_toon()` 会保留原始 JSON。

#### 恢复 Stash 内容

响应或 Schema 压缩可能会把省略内容写入 Stash，并在输出中留下 Marker：

```python
marker = re.search(r"<<tokenless:([0-9a-f]{24})>>", response_result.output)
if marker is not None:
    recovered_content = runtime.retrieve(marker.group(0))
    print(recovered_content)
```

`retrieve()` 既可以接收完整 Marker，也可以接收其中 24 个字符的 Hash。直接调用 Runtime
时，需要由调用方决定允许恢复哪些 Marker；`TokenlessSdk.retrieve()` 会执行 SDK 的模型可见
Marker 检查。

Runtime 的输入输出都是字符串。下游应直接使用各个 `CompressionResult.output`；需要了解
输入是否以及如何变化时，再检查它的 `disposition`、Token 数量和 Stash 字段。

### 查询统计

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

Session 总览使用 `stats.diff(session_id="...")`；单次工具生命周期使用
`stats.diff(session_id="...", tool_use_id="...")`；dry-run 与 active Session 对比使用
`stats.compare("baseline-session", "tokenless-session")`。

Token 数量是估算值，并且只有产生正向节省的操作才会记录。`list()`、`summary()` 和
`compare()` 不返回保存内容；`show()` 和详细 `diff()` 结果可能包含敏感工具输入或输出。
公开查询 API 不会清空数据或修改设置，但打开客户端时可能创建或迁移 `stats.db`，因此选定
的数据目录必须可写。

## 第二层：AgentScope 集成

`anolisa-tokenless-agentscope` 把通用 SDK 生命周期映射到 AgentScope。应用代码使用
`TokenlessAgentScope`，不需要自行调用 `before_model()`、`before_tool_call()`、
`after_tool_call()` 和 `retrieve()`。该集成还会把 AgentScope Session 与 Tool Call 归属传入
通用 SDK。

支持版本、构建安装、1.x/2.x/App 完整示例、配置、恢复边界和验证见
[AgentScope SDK 集成](sdk/agentscope.md)。Claude Code、OpenCode 等产品 Adapter 与这两层
Python SDK 都是不同的接入方式。

## 验证两层 SDK

构建通用 SDK Wheel 并运行 installed-wheel 测试：

```bash
make python-wheel
make test-python-runtime
```

根据 [子文档](sdk/agentscope.md#验证集成) 中的命令单独验证 AgentScope 层。

## 相关文档

- [Agent 集成](framework-integration.md)
- [AgentScope SDK 集成](sdk/agentscope.md)
- [CLI 参考](cli-reference.md)
- [效果度量](measuring-savings.md)
- [配置与数据隐私](configuration-and-privacy.md)
- [Runtime 设计](../../../../../src/tokenless/docs/design/runtime-library_zh.md)
