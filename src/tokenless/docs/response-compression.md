# Response 压缩功能说明

## 一、功能概述

Response 压缩由核心 Rust 库 `ResponseCompressor`（`crates/tokenless-schema/src/response_compressor.rs`）实现，通过递归遍历 JSON 值，应用 **7 条压缩规则** 来缩减 LLM 工具调用结果的 token 消耗。实测节省率因内容而异：`web_fetch` 类内容可达 **~78%**，结构化 API 返回约 **~26%**。

## 二、7 条压缩规则

| # | 规则 | 判断条件 | 处理方式 | 默认阈值 |
|---|------|---------|---------|---------|
| R1 | **字符串截断** | 字符串字节长度 > 4096 | 在 UTF-8 安全边界截断，追加 `… (truncated)` | 4096 字节 |
| R2 | **数组截断** | 数组元素 > 32 | 保留前 32 个，末尾追加 `<... N more items truncated>` | 32 个 |
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

### 路径 2：copilot-shell hook（`PostToolUse` 事件）— 含 TOON 流水线

```
工具执行完成
   ↓
copilot-shell 触发 PostToolUse 事件，stdin 传入 JSON
   ↓
提取 tool_response 字段
   ↓
检查：长度 < 200 字节 → 跳过（太短不值得压缩）
   ↓
检查：是否为内容检索工具（Read/Glob/list_directory 等）→ 跳过
   ↓
检查：是否为 skill 文件（YAML 头标记）→ 跳过
   ↓
Step 1：echo "$TOOL_RESPONSE" | tokenless compress-response（零截断语义清理）
   ↓
Step 2：echo "$COMPRESSED" | tokenless compress-toon（无损 TOON 编码）
   ↓
两步均采用 fail-open 策略，任何一步失败都透传上一步结果
   ↓
返回 { suppressOutput: true, hookSpecificOutput: { additionalContext: compressed } }
```

**流水线说明**：copilot-shell 的 PostToolUse hook 中实现了一个**两阶段链式压缩流水线**：

1. **第一阶段 — 响应压缩（3-layer 分流）**：
   - Layer 1 — 内容检索工具（Read/Glob/Grep）：跳过全部压缩，保留完整性
   - Layer 2 — Shell/exec 工具（Bash/Shell）：适度截断阈值（64K 字符/128 数组/8 深度），95% 真实 shell 输出完整保留，仅对极端输出截断
   - Layer 3 — API/结构化工具（其他所有）：零截断阈值（1M 字符/64K 数组/max_depth=32），仅做语义清理（R3/R4/R5），从不截断有意义的内容
2. **第二阶段 — TOON 编码（无损）**：将第一阶段输出的 JSON 通过 `toon_format::encode_default()` 编码为紧凑的二进制 TOON 格式，消除 JSON 语法开销（引号、逗号、冒号、花括号）。

两个阶段各自独立，任一步骤失败都不影响原始结果的透传（fail-open）。

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

使用共享的 `compress_response_hook.py`（与 copilot-shell 共用），通过 `hooks.json` 中的 `${QODER_TOKENLESS_HOOKS}` 变量引用共享 hook 路径。

### 路径 5：Claude Code 插件（`PostToolUse` hook）

通过 `run-hook.sh` 调度器定位共享 hook 脚本，调用 `compress_response_hook.py`。Claude Code 复制插件到版本化缓存目录，因此 `run-hook.sh` 通过 FHS 路径查找共享 hook。

### 路径 6：Codex 插件（`PostToolUse` hook）

独立的 Python hook 脚本 `compress-response`，实现完整的压缩+TOON+环境错误检测流水线。与 copilot-shell 的 hook 不同，Codex 的 PostToolUse **不能抑制原始输出**（`suppressOutput` 被拒绝），因此注入压缩摘要作为 `additionalContext`。

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

输入（`truncate_arrays_at = 3` 为例）：
```json
[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
```

输出：
```json
[1, 2, 3, "<... 7 more items truncated>"]
```

默认阈值 32 个元素。

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

输入（`truncate_arrays_at = 2` 为例）：
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
  "<... 2 more items truncated>"
]
```

第一个对象的 `debug`（R3）和 `value: null`（R4）被移除，数组在第 2 个元素后截断（R2）。

## 六、默认配置汇总

| 参数 | 默认值 | Builder 方法 |
|------|-------|-------------|
| `truncate_strings_at` | 4096 | `with_truncate_strings_at(len)` |
| `truncate_arrays_at` | 32 | `with_truncate_arrays_at(len)` |
| `drop_nulls` | true | `with_drop_nulls(bool)` |
| `drop_empty_fields` | true | `with_drop_empty_fields(bool)` |
| `max_depth` | 8 | `with_max_depth(depth)` |
| `add_truncation_marker` | true | `with_add_truncation_marker(bool)` |
| `drop_fields` | 7 个（见上文） | `add_drop_field(field)` |

## 七、Fail-Open 设计

所有集成路径均采用 fail-open 策略：

- **OpenClaw 插件**：`tryCompressResponse` 的 try-catch 返回 null，hook 不返回值 → 原始结果透传
- **copilot-shell hook**：任何失败点（依赖缺失、压缩失败、输出为空）均 `exit 0` 且不输出 stdout → 原始结果透传
- **CLI**：错误输出到 stderr，调用方可检查退出码决定是否回退

## 八、关键文件路径

| 用途 | 文件路径 |
|------|--------|
| 核心压缩算法（ResponseCompressor） | `crates/tokenless-schema/src/response_compressor.rs` |
| Schema 压缩器（SchemaCompressor） | `crates/tokenless-schema/src/schema_compressor.rs` |
| 公开 API | `crates/tokenless-schema/src/lib.rs` |
| CLI 子命令 | `crates/tokenless-cli/src/main.rs` |
| 环境检查 | `crates/tokenless-cli/src/env_check.rs` |
| 统计记录器（SQLite WAL） | `crates/tokenless-stats/src/recorder.rs` |
| 统计记录类型及操作枚举 | `crates/tokenless-stats/src/record.rs` |
| OpenClaw 插件 | `adapters/tokenless/openclaw/dist/index.js` |
| OpenClaw 插件配置 | `adapters/tokenless/openclaw/openclaw.plugin.json` |
| copilot-shell hook（响应+TOON 流水线） | `adapters/tokenless/common/hooks/compress_response_hook.py` |
| Hermes 插件 | `adapters/tokenless/hermes/__init__.py` |
| Qoder 插件配置 | `adapters/tokenless/qoder/hooks.json` |
| Claude Code 插件 | `adapters/tokenless/claude-code/hooks/run-hook.sh` |
| Codex 压缩 hook | `adapters/tokenless/codex/scripts/compress-response` |
| TOON 编解码器（crates.io toon-format） | `toon-format` crate v0.4.6 |
| 集成测试 | `crates/tokenless-schema/tests/integration_test.rs` |
| TOON E2E 测试 | `tests/test-toon-full.sh` |
| 全量测试套件 | `tests/run-all-tests.sh` |

## 九、TOON 压缩与统计验证

### 9.1 TOON 压缩 CLI

```bash
# TOON 编码（JSON → 紧凑二进制文本格式）
echo '{"users":[{"id":1,"name":"Alice"}]}' | tokenless compress-toon

# TOON 解码（往返验证）
echo '{"name":"test","value":42}' | tokenless compress-toon | tokenless decompress-toon

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

> **SLS 日志记录（JSONL）**：除 SQLite 统计外，tokenless 默认还会将每次压缩以 SLS JSONL 记录写入 `/var/log/anolisa/sls/ops/tokenless.jsonl`（默认开启）。该文件由 **anolisa SLS 组件统一管理**，tokenless 不创建/删除，仅在文件存在时追加，不存在则跳过。开关字段 `~/.tokenless/config.json` 的 `sls_enabled`（默认 `true`），环境变量 `TOKENLESS_SLS_ENABLED` 优先；输出路径可用 `TOKENLESS_SLS_PATH` 覆盖（须位于 `/var/log/` 或 `/tmp/` 下）。仅记录度量，不含原文/敏感数据。详见用户手册 [SLS 日志记录](tokenless-user-manual-zh.md#sls-日志记录)。

### 9.3 压缩效果说明

| 数据类型 | 响应压缩 | 响应压缩+TOON | 说明 |
|---------|---------|--------------|------|
| 含 debug/trace 的 API 响应 | ~78% | ~82-85% | 响应压缩移除冗余字段后，TOON 消除剩余 JSON 语法 |
| 表格数据 `[{...}]` | ~5-10% | ~40-60% | 响应压缩对表格效果有限，TOON 效果显著（实测 44%） |
| 简单扁平对象 | ~0-10% | ~15-25% | JSON 语法开销占比有限 |
| 嵌套 Schema 定义 | ~57% | ~60-65% | Schema 压缩由专门的 SchemaCompressor 处理 |
