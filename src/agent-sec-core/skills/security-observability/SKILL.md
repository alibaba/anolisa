---
name: security-observability
description: 使用 agent-sec-cli 查询本地安全事件并生成会话级安全复盘。用户要求查看安全告警、审计安全检测结果、按时间/类别/trace/session/run 筛选事件、统计安全事件，或查询最近一次 Agent 会话的工具与安全判定时使用。
---

# Security Observability

通过 `agent-sec-cli` 查询本地 SQLite 中的安全事件，并将事件与 Agent 会话关联起来。此 Skill 只执行只读查询，不负责写入 observability 数据。

## 查询流程

1. 先用 `events --summary` 或 `events --count-by` 获取概览。
2. 根据 `event_type`、`category`、关联 ID 和时间范围缩小查询。
3. 需要程序解析时使用 `--output json` 或 `--output jsonl`，不要解析 table 或 summary 文本。
4. 已知 `session_id` 或需要查看最近会话时，使用 `observability report --format json` 汇总 LLM、工具和安全事件。
5. 向用户报告必要结论即可。`details` 可能包含命令、扫描证据或后端诊断信息，不要无必要地完整回显。

## 安全事件查询

### 概览

```bash
# 最近 24 小时的人类可读安全态势摘要
agent-sec-cli events --summary

# 最近 24 小时按类别统计
agent-sec-cli events --last-hours 24 --count-by category

# 最近 8 小时 code_scan 事件数量
agent-sec-cli events --last-hours 8 --category code_scan --count
```

`--summary` 在未指定时间范围时默认查询最近 24 小时。它输出人类可读文本，只适合展示，不适合作为稳定的数据接口。

### 筛选并获取结构化数据

```bash
# 查询最近一小时的代码扫描事件
agent-sec-cli events \
  --last-hours 1 \
  --category code_scan \
  --output json

# 按 session 和 run 精确关联，并以 JSONL 输出
agent-sec-cli events \
  --session-id '<session_id>' \
  --run-id '<run_id>' \
  --output jsonl

# 查询 ISO-8601 时间区间；since 包含边界，until 不包含边界
agent-sec-cli events \
  --since '2026-08-05T00:00:00Z' \
  --until '2026-08-06T00:00:00Z' \
  --limit 100 \
  --offset 0 \
  --output json
```

### 参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--event-type` | 无 | 按事件类型筛选 |
| `--category` | 无 | 按安全能力类别筛选 |
| `--trace-id` | 无 | 按一次 CLI/调用链 trace 筛选 |
| `--session-id` | 无 | 按 Agent 会话筛选 |
| `--run-id` | 无 | 按 Agent run/turn 筛选 |
| `--since` | 无 | ISO-8601 起始时间，包含该边界 |
| `--until` | 无 | ISO-8601 结束时间，不包含该边界 |
| `--last-hours` | 无 | 查询最近 N 小时，可使用小数 |
| `--limit` | `100` | 最多返回的事件数 |
| `--offset` | `0` | 跳过的事件数 |
| `--count` | `false` | 只输出匹配事件数量 |
| `--count-by` | 无 | 分组计数，仅支持 `category`、`event_type`、`trace_id` |
| `--output`, `-o` | `table` | 列表输出格式：`table`、`json`、`jsonl` |
| `--summary` | `false` | 输出人类可读安全态势摘要 |

### 参数约束

- `--last-hours` 与 `--since` / `--until` 互斥。
- `--count` 与 `--count-by` 互斥。
- `--summary` 与 `--count`、`--count-by`、任何显式 `--output` 互斥。
- `--summary` 会读取最多 10000 条匹配事件；普通列表使用 `--limit` 和 `--offset`。
- 未知 `event_type` 或 `category` 会产生 warning，但查询仍会执行，以兼容未来新增类型。

## 事件类型与类别

| `event_type` | `category` | 含义 |
|--------------|------------|------|
| `sandbox_prehook` | `sandbox` | 沙箱执行前决策 |
| `harden` | `hardening` | Security Baseline 检查或加固 |
| `verify` | `asset_verify` | 资产完整性验证 |
| `summary` | `summary` | 安全摘要动作 |
| `code_scan` | `code_scan` | 代码安全扫描 |
| `prompt_scan` | `prompt_scan` | Prompt 安全扫描 |
| `pii_scan` | `pii_scan` | PII/凭据检测 |
| `skill_ledger` | `skill_ledger` | Skill Ledger 检查 |

成功或失败由顶层 `result` 表示，不要假设失败事件的 `event_type` 带 `_error` 后缀。

## `events` 输出结构

### JSON 列表与 JSONL

`--output json` 返回事件对象数组。`--output jsonl` 每行返回一个相同结构的事件对象。事件 envelope 为：

```json
{
  "event_id": "<uuid>",
  "event_type": "code_scan",
  "category": "code_scan",
  "result": "succeeded",
  "timestamp": "<ISO-8601 UTC>",
  "trace_id": "<trace_id>",
  "pid": 1234,
  "uid": 1000,
  "session_id": "<session_id-or-null>",
  "run_id": "<run_id-or-null>",
  "call_id": "<call_id-or-null>",
  "tool_call_id": "<tool_call_id-or-null>",
  "details": {}
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `event_id` | string | 事件 UUID |
| `event_type` | string | 事件类型 |
| `category` | string | 聚合类别 |
| `result` | string | `succeeded` 或 `failed` |
| `timestamp` | string | ISO-8601 UTC 时间 |
| `trace_id` | string | 调用链标识，可能为空字符串 |
| `pid`, `uid` | integer | 记录事件的进程与用户 ID |
| `session_id`, `run_id` | string \| null | Agent 会话和 run/turn 关联标识 |
| `call_id`, `tool_call_id` | string \| null | LLM 与工具调用关联标识 |
| `details` | object | 后端专属结构化数据 |

`details` 没有跨事件类型的固定 schema。只读取当前任务需要且实际存在的字段，不要根据其他类别的事件臆测字段。安全判定可能位于 `details.verdict` 或 `details.result.verdict`；使用前先检查类型和存在性。

### 计数输出

`--count` 输出一个 JSON 整数：

```json
12
```

`--count-by` 输出一个 JSON 对象，键是分组值，值是数量：

```json
{
  "code_scan": 8,
  "prompt_scan": 4
}
```

## 会话级报告

### 调用方式

```bash
# 最近记录的会话
agent-sec-cli observability report --last --format json

# 指定会话
agent-sec-cli observability report \
  --session-id '<session_id>' \
  --format json
```

选择 `--last` 或 `--session-id` 之一。命令没有默认目标；两者都不提供时返回错误。`--format` 支持 `text` 和 `json`，供 Agent 解析时必须使用 `json`。

### JSON 结构

```json
{
  "session_id": "<session_id>",
  "first_seen": "2026-08-05 10:00:00",
  "last_seen": "2026-08-05 10:05:00",
  "duration_seconds": 300.0,
  "turn_count": 3,
  "llm_calls": 4,
  "request_bytes": 1200,
  "response_bytes": 2400,
  "tool_breakdown": {
    "shell": 2
  },
  "security_verdicts": {
    "code_scan": {
      "succeeded": 2
    }
  },
  "security_hint": null
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `session_id` | string | 会话 ID |
| `first_seen`, `last_seen` | string | 会话首末事件的 UTC 时间文本 |
| `duration_seconds` | number | 会话持续秒数，保留一位小数 |
| `turn_count` | integer | 记录的 Agent turn 数 |
| `llm_calls` | integer | `after_llm_call` 事件数 |
| `request_bytes`, `response_bytes` | integer | 模型请求与响应累计字节数 |
| `tool_breakdown` | object<string, integer> | 工具名称到调用次数的映射 |
| `security_verdicts` | object<string, object<string, integer>> | 安全类别到 `result` 计数的映射 |
| `security_hint` | string \| null | 安全事件不可用、未关联或查询失败时的说明 |

`security_verdicts` 当前按安全事件顶层 `result`（如 `succeeded` / `failed`）聚合；不要把它解释为扫描器的 `pass` / `warn` / `deny`。如需具体扫描 verdict，再通过相同 `session_id` 查询 `events --output json` 并检查对应事件的 `details`。

## 关联与报告规则

- 优先使用 `session_id` 关联会话，使用 `run_id` 缩小到某个 run/turn；`trace_id` 用于追踪一次具体调用链。
- 会话报告没有安全事件时，检查 `security_hint`，不要直接断言“没有发生安全检测”。
- table 与 summary 是人类展示格式，不承诺稳定列结构；自动化处理必须选择 JSON/JSONL。
- 报告事件时给出查询范围、筛选条件、匹配数量和必要结论。除非用户明确要求，不完整输出 `details` 中的命令、输入、证据或诊断信息。
