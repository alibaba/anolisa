# Response 压缩功能说明

## 一、功能概述

第一阶段的 PostTool 压缩由 Runtime 内部的 `PostToolPipeline` 编排，并把 JSON 内容静态派发给 `JsonCompressor`（`crates/tokenless-compressors/src/json.rs`）。压缩器解析 JSON、递归应用以下规则，并在宿主允许文本替换时选择更小的 Compact JSON 或 TOON 表示。非 JSON 内容当前统一透传；Terminal Cleanup 与 Build Log 引擎仍保留，但尚未接入 Pipeline。

## 二、7 条压缩规则

| # | 规则 | 判断条件 | 处理方式 | 默认阈值 |
|---|------|---------|---------|---------|
| R1 | **字符串截断** | Unicode 字符数 > 4096 | 在 Unicode 字符边界截断，追加截断标记 | 4096 字符 |
| R2 | **数组截断** | 数组元素 > 32 | 保留前 32 个 + 末尾 8 个（`array_tail_preserve`），head 与 tail 之间插入 `<... N more items truncated, not stashed>`；head+tail 覆盖全部元素时不截断 | 32 + 8 个 |
| R3 | **字段删除** | key 匹配黑名单 | 整个字段移除（不递归进入） | 7 个字段 |
| R4 | **null 移除** | 值为 `null` | 从对象/数组中删除 | 启用 |
| R5 | **空值移除** | 值为 `""` / `[]` / `{}` | 从对象/数组中删除 | 启用 |
| R6 | **深度截断** | 嵌套深度 > 8 | 替换为 `<{type} truncated at depth {N}>` | 8 层 |
| R7 | **原始类型保留** | bool / number | 直接保留，不做处理 | — |

**R3 默认黑名单字段**：`debug`, `trace`, `traces`, `stack`, `stacktrace`, `logs`, `logging`

## 三、递归处理顺序

```
compress_value(value, depth)
 ├─ 1. 检查深度限制 → 超限则返回截断标记（R6）
 ├─ 2. 按类型分支：
 │   ├─ null / bool / number → 直接返回（R7）
 │   ├─ string → compress_string()（R1）
 │   ├─ array  → compress_array()
 │   │   ├─ 截取前 N 个元素（R2）
 │   │   ├─ 逐项递归 compress_value(item, depth+1)
 │   │   ├─ 过滤 null（R4）和空值（R5）
 │   │   └─ 追加截断标记
 │   └─ object → compress_object()
 │       ├─ 跳过黑名单字段（R3）
 │       ├─ 逐值递归 compress_value(val, depth+1)
 │       └─ 过滤 null（R4）和空值（R5）
```

## 四、集成路径

### 路径 1：OpenClaw 插件（`tool_result_persist` hook）

```
工具执行完成
   ↓
OpenClaw 触发 tool_result_persist 事件
   ↓
插件检查：RTK 启用且 toolName === "exec" → 跳过（避免双重压缩）
   ↓
tryCompressResponse(event.message)
   ↓
execFileSync("tokenless", ["compress-response"], { input: JSON, timeout: 3s })
   ↓
返回 { message: compressed } 替换原始结果
```

**RTK 跳过逻辑**：当 RTK 启用且可用时，`exec` 工具的结果已经过 RTK 优化，不再二次压缩。

### 路径 2：共享 PostTool Hook（Protocol v2）

```
工具执行完成
   ↓
Adapter 触发 PostTool 事件，stdin 传入框架 Envelope
   ↓
Hook 转换为 `operation: "post_tool"` 的 v2 Request
   ↓
调用一次 `tokenless compress`
   ↓
Runtime 执行门禁、内容检测和静态派发
   ↓
JSON → `JsonCompressor`；非 JSON → Passthrough
   ↓
Runtime 执行一次字符/Token 仲裁和一次 Stash Commit/Rollback
   ↓
Hook 校验版本与 Operation，并按宿主 Capability 应用 v2 Result
```

**流水线说明**：`PostToolPipeline` 位于 Runtime 内部。第一阶段只接入
`ContentType::Json -> JsonCompressor`；清理、截断、Structured Slot 恢复、Compact JSON 与
可选 TOON 都在同一次 JSON 领域调用内完成。其他 ContentType 当前不调用保留的文本引擎。
Common Hook 不声明受信 Retrieve 能力，因此只接受无损 JSON 候选；需要截断的候选以
`recoverability_unavailable` 透传。实际 Pipeline、Stash 或 RTK 操作错误由 CLI 以退出码 1
返回，Hook 在进程边界上 fail-open。Common PreTool Hook 仍直接调用 RTK，本阶段尚未把逐调用
`output_optimization: "rtk"` 状态传给 PostTool。

**TOON 效果**：对结构化/表格数据可额外节省 30-60%，整体压缩效果 = 响应压缩节省 + TOON 语法消除。例如：原始 JSON 4480 字节，经响应压缩至 625 字节（~86%），再经 TOON 编码进一步缩减。实测表格数据（`[{"id":...}]`）可达到 44% 的 TOON 单独节省。

### 路径 3：Hermes Agent 插件（`transform_tool_result` hook）

```
工具执行完成
   ↓
Hermes 触发 transform_tool_result 事件
   ↓
检查：是否为内容检索工具（Read/Glob/...）→ 跳过
   ↓
检查：响应长度 < 200 字符 → 跳过
   ↓
Step 1：tokenless compress-response（零截断语义清理）
   ↓
Step 2：tokenless compress-toon（无损 TOON 编码）
   ↓
两步均采用 fail-open 策略
   ↓
返回压缩后的结果字符串
```

### 路径 4：Qoder CLI 插件（`PostToolUse` hook）

Qoder 通过原生插件目录 `hooks/hooks.json` 加载 hook，并在运行时展开 `${QODER_PLUGIN_ROOT}`。插件内的 `hooks/run-hook.sh` 再从 ANOLISA adapter 目录定位共享的 `compress_response_hook.py`，无需改写 `~/.qoder/settings.json` 或将机器相关绝对路径写入插件缓存。

Qoder CLI 支持对任意工具使用 `hookSpecificOutput.updatedToolOutput`，因此压缩结果会**替换**原始工具输出，`additionalContext` 只携带环境错误归因等追加信息。结构化响应沿用 Claude Code 的 schema 保留逻辑；字符串响应可使用更小的 TOON 文本。其他不支持输出替换的 agent 才使用 `additionalContext` 回退。

### 路径 5：Claude Code 插件（`PostToolUse` hook）

通过 `run-hook.sh` 调度器定位共享 hook 脚本，调用 `compress_response_hook.py`。Claude Code 复制插件到版本化缓存目录，因此 `run-hook.sh` 通过 FHS 路径查找共享 hook。

与其他 agent 不同，Claude Code 的 `additionalContext` 是**追加式**的（模型会同时看到原始工具结果和注入内容），因此压缩结果通过 `hookSpecificOutput.updatedToolOutput`（Claude Code >= 2.1.121）**替换**模型可见的工具结果，`additionalContext` 仅保留真正追加式的诊断信息（环境错误归因）。替换时会回填被压缩剥离的空 schema 字段（如 Bash 的 `stderr`/`interrupted`/`isImage`），保持内置工具输出结构不变；结构化响应不做 TOON 编码（TOON 为文本格式，会破坏 schema）。旧版本 Claude Code（< 2.1.121）或版本无法探测时 fail-open：直接透传原始结果，不注入重复内容。版本探测结果缓存于 `~/.tokenless/.claude-version`（0600 权限、拒绝符号链接，与其他 hook 状态文件一致），缓存键为 claude 二进制的路径+mtime+大小，升级 Claude Code 后自动失效重探，避免每次 PostToolUse 都启动 node CLI。

### 路径 6：Codex 插件（`PostToolUse` hook）

Codex 的 PostToolUse 不能替换或抑制原始输出。通过 `additionalContext` 追加压缩内容
会同时保留原文，增加模型首轮可见 Payload，因此 Codex Adapter 不运行响应压缩或
TOON。独立脚本 `response-diagnostics` 只在识别出环境失败时追加修复提示。支持的 Shell
命令由 PreToolUse Hook 通过 RTK 在执行前重写，工具从源头产生更小的输出。

### 路径 7：CLI 直接使用

```bash
# 从文件
tokenless compress-response -f response.json

# 从 stdin
cat response.json | tokenless compress-response

# 管道组合
curl -s https://api.example.com/data | tokenless compress-response
```

## 五、压缩前后示例

### 示例 1 — 字段删除 + null 移除 + 空值移除（R3 + R4 + R5）

输入：
```json
{
  "status": "success",
  "data": { "name": "test", "count": 42 },
  "debug": { "request_id": "abc123", "timing": 0.05 },
  "trace": "GET /api/data 200 OK",
  "metadata": null,
  "tags": [],
  "extra": ""
}
```

输出：
```json
{
  "status": "success",
  "data": { "name": "test", "count": 42 }
}
```

被删除的内容：`debug`（R3 黑名单）、`trace`（R3 黑名单）、`metadata`（R4 null）、`tags`（R5 空数组）、`extra`（R5 空字符串）。

### 示例 2 — 字符串截断（R1）

输入（`truncate_strings_at = 20` 为例）：
```json
"This is a very long string that should be truncated"
```

输出：
```json
"This is a very long … (truncated)"
```

默认阈值 4096 字节。多字节 UTF-8 字符（如中文）会回退到安全边界，不会截断在字符中间。

### 示例 3 — 数组截断（R2）

输入（`truncate_arrays_at = 3`、`array_tail_preserve = 0` 为例）：
```json
[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
```

输出：
```json
[1, 2, 3, "<... 7 more items truncated, not stashed>"]
```

默认阈值 32 个元素。默认同时保留末尾 8 个元素（`array_tail_preserve = 8`）：
截断时保留 head + tail，两者之间插入截断标记，中间被丢弃；若 head+tail
能覆盖整个数组，则不截断、不加标记。上例在默认配置下（3 + 8 ≥ 10）会
原样保留全部 10 个元素。

### 示例 4 — 深度截断（R6）

输入（`max_depth = 2` 为例）：
```json
{
  "level1": {
    "level2": {
      "level3": {
        "level4": "deep value"
      }
    }
  }
}
```

输出：
```json
{
  "level1": {
    "level2": {
      "level3": "<object truncated at depth 3>"
    }
  }
}
```

默认阈值 8 层。

### 示例 5 — 递归组合压缩（R1 + R3 + R4 同时生效）

输入（`truncate_strings_at = 10` 为例）：
```json
{
  "outer": {
    "inner": {
      "long_text": "This is a very long text that should be truncated",
      "null_field": null,
      "number": 42
    }
  }
}
```

输出：
```json
{
  "outer": {
    "inner": {
      "long_text": "This is a … (truncated)",
      "number": 42
    }
  }
}
```

### 示例 6 — 数组内对象的复合压缩（R2 + R3 + R4）

输入（`truncate_arrays_at = 2`、`array_tail_preserve = 0` 为例）：
```json
[
  {"id": 1, "debug": "remove me", "value": null},
  {"id": 2},
  {"id": 3},
  {"id": 4}
]
```

输出：
```json
[
  {"id": 1},
  {"id": 2},
  "<... 2 more items truncated, not stashed>"
]
```

第一个对象的 `debug`（R3）和 `value: null`（R4）被移除，数组在第 2 个元素后截断（R2）。

## 六、默认配置汇总

| `JsonCompressionConfig` 字段 | 默认值 | 含义 |
|------|-------|-------------|
| `truncate_strings_at` | 4096 | 最大 Unicode 字符数 |
| `truncate_arrays_at` | 32 | 保留的 head 元素数 |
| `array_tail_preserve` | 8 | 保留的 tail 元素数；0 表示仅保留 head |
| `drop_nulls` | true | 移除 null 值 |
| `drop_empty_fields` | true | 移除空字符串、数组和对象 |
| `max_depth` | 8 | 最大递归深度 |
| `add_truncation_marker` | true | 为截断内容生成有界标记 |

## 七、Fail-Open 设计

所有集成路径均采用 fail-open 策略：

- **OpenClaw 插件**：`tryCompressResponse` 的 try-catch 返回 null，hook 不返回值 → 原始结果透传
- **copilot-shell hook**：任何失败点（依赖缺失、压缩失败、输出为空）均 `exit 0` 且不输出 stdout → 原始结果透传
- **CLI**：错误输出到 stderr，调用方可检查退出码决定是否回退

## 八、关键文件路径

| 用途 | 文件路径 |
|------|--------|
| JSON 领域压缩器（JsonCompressor） | `crates/tokenless-compressors/src/json.rs` |
| PostTool Pipeline 与最终仲裁 | `crates/tokenless-runtime/src/post_tool/` |
| Schema 压缩器（SchemaCompressor） | `crates/tokenless-schema/src/schema_compressor.rs` |
| 内容压缩公开 API | `crates/tokenless-compressors/src/lib.rs` |
| CLI 子命令 | `crates/tokenless-cli/src/main.rs` |
| 环境检查 | `crates/tokenless-cli/src/env_check.rs` |
| 统计记录器（SQLite WAL） | `crates/tokenless-stats/src/recorder.rs` |
| 统计记录类型及操作枚举 | `crates/tokenless-stats/src/record.rs` |
| OpenClaw 插件 | `adapters/tokenless/openclaw/dist/index.js` |
| OpenClaw 插件配置 | `adapters/tokenless/openclaw/openclaw.plugin.json` |
| copilot-shell hook（响应+TOON 流水线） | `adapters/tokenless/common/hooks/compress_response_hook.py` |
| Hermes 插件 | `adapters/tokenless/hermes/__init__.py` |
| Qoder 插件配置 | `adapters/tokenless/qoder/hooks/hooks.json` |
| Claude Code 插件 | `adapters/tokenless/claude-code/hooks/run-hook.sh` |
| Codex 响应诊断 Hook | `adapters/tokenless/codex/scripts/response-diagnostics` |
| TOON 编解码器（crates.io toon-format） | `toon-format` crate v0.4.6 |
| JSON 压缩测试 | `crates/tokenless-compressors/src/tests/json_tests.rs` |
| PostTool 集成测试 | `crates/tokenless-runtime/src/entry.rs` |
| TOON E2E 测试 | `tests/test-toon-full.sh` |
| 全量测试套件 | `tests/run-all-tests.sh` |

## 九、TOON 压缩与统计验证

### 9.1 TOON 压缩 CLI

短于 500 字符的负载默认原样透传（与 Hook 层阈值一致）；示例中使用
`--min-toon-chars 0` 对短负载强制编码。透传和无收益场景会在 stdout 上
逐字节原样复现输入（不添加、不去除末尾换行符），脚本可直接比较
stdout 与输入来判断是否发生了编码。

```bash
# TOON 编码（JSON → 紧凑二进制文本格式）
echo '{"users":[{"id":1,"name":"Alice"}]}' | tokenless compress-toon --min-toon-chars 0

# TOON 解码（往返验证）
echo '{"name":"test","value":42}' | tokenless compress-toon --min-toon-chars 0 | tokenless decompress-toon

# 附带统计追踪（自动记录到 SQLite 数据库）
tokenless compress-toon -f data.json --agent-id my-agent --session-id sess-001
```

### 9.2 通过统计数据库验证压缩效果

Tokenless 自动将每次压缩操作记录到 `~/.tokenless/stats.db`（SQLite WAL 模式）。四种操作类型均被追踪：`compress-schema`、`compress-response`、`rewrite-command`、`compress-toon`。

```bash
# 查看统计状态
tokenless stats status

# 列出最近 20 条记录
tokenless stats list

# 查看某条记录的压缩前后文本对比
tokenless stats show <id>

# 查看汇总统计（按操作类型分组）
tokenless stats summary
```

统计启用条件：`TOKENLESS_STATS_ENABLED` 环境变量未设为 `0`/`false`，或通过 `tokenless stats enable` 启用。

> **SLS 日志记录（JSONL）**：除 SQLite 统计外，tokenless 默认还会将每次压缩以 SLS JSONL 记录写入 `/var/log/anolisa/sls/ops/tokenless.jsonl`（默认开启）。该文件由 **anolisa SLS 组件统一管理**，tokenless 不创建/删除，仅在文件存在时追加，不存在则跳过。开关字段 `~/.tokenless/config.json` 的 `sls_enabled`（默认 `true`），环境变量 `TOKENLESS_SLS_ENABLED` 优先；输出路径可用 `TOKENLESS_SLS_PATH` 覆盖（须位于 `/var/log/` 或 `/tmp/` 下）。仅记录度量，不含原文/敏感数据。详见 [Tokenless 效果度量 · SLS JSONL](../../../docs/user-guide/zh/token-saving/tokenless/measuring-savings.md#sls-jsonl)。

### 9.3 压缩效果说明

| 数据类型 | 响应压缩 | 响应压缩+TOON | 说明 |
|---------|---------|--------------|------|
| 含 debug/trace 的 API 响应 | ~78% | ~82-85% | 响应压缩移除冗余字段后，TOON 消除剩余 JSON 语法 |
| 表格数据 `[{...}]` | ~5-10% | ~40-60% | 响应压缩对表格效果有限，TOON 效果显著（实测 44%） |
| 简单扁平对象 | ~0-10% | ~15-25% | JSON 语法开销占比有限 |

Schema 压缩不经过本表的响应压缩或 TOON 流程。Tokenless 0.7.11 在仓库参考
fixture 上的独立 Schema 压缩结果为 47.3%；该数字不是生产范围或任意 Schema
的保证值，实际结果取决于输入结构、description 长度和可移除字段。
