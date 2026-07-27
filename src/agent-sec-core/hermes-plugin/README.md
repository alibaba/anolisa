# Hermes Plugin — Agent-Sec-Core

Hermes Agent 安全插件，基于 `agent-sec-cli` 提供 OS 级安全防护能力。

## 架构概述

```
src/                          # 运行时文件（部署到 ~/.hermes/plugins/）
├── plugin.yaml               # Hermes 插件 manifest
├── __init__.py               # register(ctx) 入口
├── config.toml               # 能力开关与参数
├── registry.py               # 能力注册器 + safe-wrap
├── cli_runner.py             # agent-sec-cli subprocess 封装
├── observability/            # Observability 记录转换
│   ├── helpers.py            # 通用转换 helper
│   └── record.py             # Hermes hook -> agent-sec-cli schema
└── capabilities/
    ├── __init__.py           # 能力清单
    ├── base.py               # AgentSecCoreCapability 抽象基类
    ├── code_scan.py          # Code Scanner 实现
    ├── observability.py      # Observability 实现
    ├── pii_scan.py           # PII Checker 实现
    ├── prompt_scan.py        # Prompt Scanner 实现
    └── skill_ledger.py       # Skill Ledger 实现
```

采用 **capability 分层模式**：每个安全能力继承 `AgentSecCoreCapability` 抽象基类，
通过 `config.toml` 控制开关，`registry.py` 统一注册。

## 如何新增一个 Capability

### 1. 创建能力文件

在 `src/capabilities/` 下新建 `my_capability.py`：

```python
"""My new security capability."""

import logging

from ..cli_runner import call_agent_sec_cli
from .base import AgentSecCoreCapability

logger = logging.getLogger("agent-sec-core")


class MyCapability(AgentSecCoreCapability):
    id = "my-capability"
    name = "My Capability"

    def _on_register(self, config: dict) -> None:
        """Read capability-specific config."""
        self._my_option = config.get("my_option", "default")

    def get_hooks_define(self) -> dict:
        return {"pre_tool_call": self._on_pre_tool_call}

    def _on_pre_tool_call(self, tool_name, args, **kwargs):
        # 实现逻辑...
        return None  # None = 放行
```

### 2. 导出能力

在 `src/capabilities/__init__.py` 中添加：

```python
from .my_capability import MyCapability

ALL_CAPABILITIES = [
    CodeScanCapability(),
    MyCapability(),  # 新增
]
```

### 3. 添加配置

在 `src/config.toml` 中添加（所有字段必须显式配置）：

```toml
[capabilities.my-capability]
enabled = true
timeout = 10
```

Observability 配置：

```toml
[capabilities.observability]
enabled = true
timeout = 5
```

`timeout` 控制 `agent-sec-cli observability record` 子进程。CLI 失败、超时、invalid record
或缺少必需 metadata 都是 fail-open。

## 可用 Hook 列表

Hermes 支持的 hook 及其回调签名：

| Hook | 签名 | 返回值 |
|------|------|--------|
| `pre_tool_call` | `(tool_name, args, **kwargs)` | `None` 放行 / `{"action": "block", "message": str}` 阻断 |
| `post_tool_call` | `(tool_name, params, result)` | 观测用，返回值忽略 |
| `pre_llm_call` | `(messages, **kwargs)` | `{"context": str}` 注入上下文 / `None` |
| `post_llm_call` | `(messages, response, **kwargs)` | 观测用 |
| `pre_api_request` | `(**kwargs)` | 观测用 |
| `post_api_request` | `(**kwargs)` | 观测用 |
| `on_session_start` | `(**kwargs)` | 观测用 |
| `on_session_end` | `(**kwargs)` | 观测用 |
| `transform_tool_result` | `(tool_name, result, **kwargs)` | 修改后的 result / `None` |
| `transform_llm_output` | `(response_text, session_id, **kwargs)` | 修改后的 response text / `None` |

完整列表参见 [Hermes 官方文档](https://hermes-agent.nousresearch.com/docs/zh-Hans/user-guide/features/plugins)。

## 内置 Capability

### code-scan

`code-scan` 挂在 `pre_tool_call`，扫描 `terminal.command` 和 `execute_code.code`。
默认 observe，仅在 `enable_block = true` 时对 `warn` / `deny` 阻断。

### Skill Ledger

当前 Hermes 场景暂不支持 Skill Ledger 安全检查，请自行关注 skill 安全性。Hermes
`skill-ledger` capability 只保留 fail-open 兼容行为：检测到不兼容的 Hermes skill root
时跳过检查、不调用 CLI、不阻断，并在 `policy = "ask"` / `policy = "warn"` 下提示
`暂不支持Hermes场景，请自行关注skill安全性。`

- `enabled = false`：完全不注册 Hermes hook。
- `policy = "ask"`：默认策略；未触发暂不支持兜底时，当 summary `message` 非空会缓存为本轮告警，并通过
  `transform_llm_output` 追加到最终回复开头，确保用户可见。
- `policy = "debug"`：静默兼容模式；summary `message` 非空、CLI 失败或 JSON 解析失败都 fail-open，只写 debug。
- `policy = "warn"`：warning-only 兼容模式；未触发暂不支持兜底时，summary `message` 非空会缓存为本轮告警，并通过
  `transform_llm_output` 追加到最终回复开头，确保用户可见。
- `policy = "block"`：summary `message` 非空时直接返回 Hermes block 结果。
- 检测到暂不支持的 Hermes skill root 时，所有 `policy` 都 fail-open；`ask` / `warn`
  会显示 `暂不支持Hermes场景，请自行关注skill安全性。`，`debug` 只写日志，`block`
  不阻断。
- `latestStatus = "unmanaged"` 是 Skill Ledger 诊断状态，summary `message` 为 `null`，包括 `block` 在内的所有 policy 都静默放行。
- 未配置 `policy` 的旧配置仍兼容：`enable_block = true` 映射为 `block`，`enable_block = false` 映射为 `warn`。
- 当前版本仅覆盖 Hermes 默认本地技能目录 `~/.hermes/skills`，按 Hermes `skill_view`
  的本地目录规则解析 `category/skill` 或裸 skill 名称；`skills.external_dirs` 和
  plugin-provided skills 暂不覆盖，hook 会 fail-open 跳过。
- `file_path` / `path` 仅表示 skill 内 supporting file，不参与 skill 目录定位。
- `block_statuses` 是 legacy 兼容配置；当前 `policy = "block"` 不再按状态过滤。
- `max_warnings_per_turn = 0` 影响 `policy = "ask"` / `policy = "warn"` 的用户可见 warning 注入。
- Skill Ledger 全局 `activationPolicy` 属于 SkillFS/daemon activation；这里的 hook `policy`
  只控制宿主 hook 的可见行为和日志等级。

配置示例：

```toml
[capabilities.skill-ledger]
enabled = true
timeout = 5
policy = "ask"
enable_block = false
block_statuses = ["none", "drifted", "deny", "tampered"]
max_warnings_per_turn = 5
max_warning_contexts = 128
```

Hermes 场景请不要依赖该 capability 作为 Skill Ledger 安全拦截；如需严格 Skill
安全检查，请在非 Hermes 场景或独立流程中完成。

### observability

`observability` capability 会把每个 Hermes hook input 独立转换成一条
`agent-sec-cli` observability record：

```bash
agent-sec-cli observability record --format json --stdin
```

Hermes plugin 只负责信息转换，不维护 tracing state。它不会缓存 `task_id`、不会生成本地
counter、不会记住上一个 hook，也不会计算聚合指标。每条 record 只来自当前 hook 参数。

Hermes 当前没有原生 run id，因此插件使用固定的 schema-compatible 值：

```text
runId = 00000000-0000-0000-0000-000000000000
```

如果当前 hook input 没有真实 `session_id`，record 会被跳过。tool record 还要求当前
hook input 带有 `tool_call_id`；如果没有，插件也会跳过，因为 `agent-sec-cli` schema
要求 tool hook 必须有 `metadata.toolCallId`，而 Hermes plugin 不合成 tool id。

CLI 调用方式和 `openclaw-plugin` 保持一致：helper 将一条 JSON payload 通过 stdin 发送给
`agent-sec-cli observability record --format json --stdin`。CLI 失败只记录 debug 日志，
不会影响 Hermes hook 行为。

| Hermes hook | agent-sec-cli hook | Metadata 行为 | Metrics 行为 |
|-------------|--------------------|---------------|--------------|
| `pre_llm_call` | `before_agent_run` | 需要当前 `session_id`，固定全零 `runId` | 映射 `user_message`、`model`、`platform` |
| `pre_api_request` | `before_llm_call` | 需要当前 `session_id`，固定全零 `runId`，可从当前 `api_call_count` 生成 `callId` | 映射 `model`、`provider`、`api_mode`、`base_url`、`message_count` |
| `post_api_request` | `after_llm_call` | 需要当前 `session_id`，固定全零 `runId`，可从当前 `api_call_count` 生成 `callId` | 映射 `api_duration`、`finish_reason`、`assistant_tool_call_count` |
| `pre_tool_call` | `before_tool_call` | 需要当前 `session_id` 和当前 `tool_call_id` | 映射 `tool_name`、`args` |
| `post_tool_call` | `after_tool_call` | 需要当前 `session_id` 和当前 `tool_call_id` | 映射 `result`、`duration_ms`、result 中的直接 `exit_code` |
| `post_llm_call` | `after_agent_run` | 需要当前 `session_id`，固定全零 `runId` | 映射 `assistant_response`、`model`、`platform` |

初始实现不注册 `transform_tool_result` 和 `transform_llm_output`，因为 `post_tool_call` 和
`post_llm_call` 是语义上更直接的 producer。

### pii-scan-user-input

`pii-scan-user-input` 对齐 Cosh/OpenClaw 多点位 PII checker 语义：

- 挂在 `pre_llm_call`、`pre_tool_call`、`post_tool_call`、`transform_llm_output`、`on_session_end`
- 扫描本轮用户输入、tool 参数、tool 返回结果和最终模型回复；不扫描 history、memory 或 RAG context
- 调用 `agent-sec-cli scan-pii --stdin --format json --redact-output --source <source>`，敏感原文仅通过 stdin 传入子进程
- tool 参数/结果的 `warn` / `deny` 不阻断请求，只缓存脱敏 warning
- `transform_llm_output` 会扫描最终模型回复；命中时使用 `redacted_text` 替换用户可见回复，并 prepend 已缓存 warning
- 当前实现依赖 Hermes 对完整最终回复调用一次 `transform_llm_output`；若未来改成流式分片 transform，需要重新审视 warning pop 语义
- `on_session_end` 清理残留缓存
- 所有异常、超时、非 JSON 输出、未知 verdict 都 fail-open
- warning 只使用 `evidence_redacted`，不展示 raw evidence 或原始文本

### prompt-scan-user-input

基于`agent-sec-cli scan-prompt` 的多层检测（L1 规则引擎 + L2 ML 分类器）能力识别 prompt injection / jailbreak 攻击。

- 挂在 `pre_llm_call`、`transform_llm_output`、`on_session_end` 三个钩子
- `warn` / `deny` 不阻断请求，缓存脱敏 warning，通过 `transform_llm_output` prepend 到回复前
- 所有异常情况 fail-open

```toml
[capabilities.prompt-scan-user-input]
enabled = true
timeout = 15
warning_ttl_seconds = 300
```

## 开发与调试

### 本地测试

```bash
# 运行单元测试
cd agent-sec-core
uv run --project agent-sec-cli pytest tests/unit-test/hermes-plugin/ -v
```

### 部署到本地 Hermes

```bash
# 从源码目录直接部署
./hermes-plugin/scripts/deploy.sh
```

deploy.sh 会自动推导 `src/` 路径并复制到 `~/.hermes/plugins/agent-sec-core-hermes-plugin/`。

## 注意事项

1. **Fail-open 原则** — 任何异常都不应阻塞 agent 运行。hook 内部捕获所有异常，返回 `None` 放行。
2. **零运行时依赖** — 仅使用 Python 3.11 标准库（tomllib、json、subprocess、logging、dataclasses）。RPM 分发不携带额外 pip 包。
3. **性能要求** — `pre_tool_call` 在热路径上执行。阻断型能力通过 config.toml 配置严格超时；observability 采用 fire-and-forget 调用，不等待 CLI 结果影响 hook 行为。
4. **日志** — 使用 `logging.getLogger("agent-sec-core")`，Hermes 会自动捕获到 `~/.hermes/logs/agent.log`。
5. **导入方式** — Hermes 以包形式加载插件，因此模块间使用**相对导入**：

   ```python
   # 正确：相对导入
   from .registry import load_config              # 同级模块
   from .capabilities import ALL_CAPABILITIES     # 同级子包
   from ..cli_runner import call_agent_sec_cli    # 上级模块（在子包中）

   # 错误：裸名导入（插件目录不在 sys.path）
   # from registry import load_config
   ```

   依赖分层（无循环依赖）：
   - 底层：`cli_runner.py`（纯 stdlib，无内部依赖）
   - 中间层：`registry.py`（纯 stdlib）
   - Helper 层：`observability/*.py`（纯转换逻辑，依赖 cli_runner 以外的 stdlib）
   - 基类层：`capabilities/base.py`（依赖 registry）
   - 实现层：`capabilities/*.py`（继承 base，依赖 cli_runner 和 helper）
   - 顶层：`__init__.py`（依赖 capabilities、registry）
