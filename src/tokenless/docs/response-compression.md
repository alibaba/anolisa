# Response 压缩功能说明

## 一、功能概述

Response 压缩由核心 Rust 库 `ResponseCompressor`（`crates/tokenless-schema/src/response_compressor.rs`）实现，通过递归遍历 JSON 值，应用 **7 条压缩规则** 来缩减 LLM 工具调用结果的 token 消耗。实测节省率因内容而异：`web_fetch` 类内容可达 **~78%**，结构化 API 返回约 **~26%**。

## 二、7 条压缩规则

| # | 规则 | 判断条件 | 处理方式 | 默认阈值 |
|---|------|---------|---------|---------|
| R1 | **字符串截断** | 字符串字节长度 > 512 | 在 UTF-8 安全边界截断，追加 `… (truncated)` | 512 字节 |
| R2 | **数组截断** | 数组元素 > 16 | 保留前 16 个，末尾追加 `<... N more items truncated>` | 16 个 |
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

### 路径 2：copilot-shell hook（`PostToolUse` 事件）

```
工具执行完成
   ↓
copilot-shell 触发 PostToolUse 事件，stdin 传入 JSON
   ↓
提取 tool_response 字段
   ↓
检查：长度 < 200 字节 → 跳过（太短不值得压缩）
   ↓
echo "$TOOL_RESPONSE" | tokenless compress-response
   ↓
返回 { suppressOutput: true, hookSpecificOutput: { additionalContext: compressed } }
```

### 路径 3：CLI 直接使用

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

默认阈值 512 字节。多字节 UTF-8 字符（如中文）会回退到安全边界，不会截断在字符中间。

### 示例 3 — 数组截断（R2）

输入（`truncate_arrays_at = 3` 为例）：
```json
[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
```

输出：
```json
[1, 2, 3, "<... 7 more items truncated>"]
```

默认阈值 16 个元素。

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
| `truncate_strings_at` | 512 | `with_truncate_strings_at(len)` |
| `truncate_arrays_at` | 16 | `with_truncate_arrays_at(len)` |
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
| 核心压缩算法 | `crates/tokenless-schema/src/response_compressor.rs` |
| 公开 API | `crates/tokenless-schema/src/lib.rs` |
| CLI 子命令 | `crates/tokenless-cli/src/main.rs` |
| OpenClaw 插件 | `openclaw/index.ts`（第 161-186 行） |
| copilot-shell hook | `hooks/copilot-shell/tokenless-compress-response.sh` |
| 集成测试 | `crates/tokenless-schema/tests/integration_test.rs` |
