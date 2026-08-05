# Tokenless CLI 参考

[English](../../../en/token-saving/tokenless/cli-reference.md)

`tokenless` CLI 可独立压缩 Schema 和响应、进行 TOON 编解码、取回 Stash 内容、检查工具环境并查询统计。Agent Adapter 在内部调用同一组能力。

## 命令总览

| 命令 | 用途 |
|------|------|
| `tokenless compress-schema` | 压缩 Function Calling 工具 Schema |
| `tokenless compress-response` | 压缩 JSON/API/工具响应 |
| `tokenless compress-toon` | 将 JSON 编码为 TOON |
| `tokenless decompress-toon` | 将 TOON 解码为 JSON |
| `tokenless retrieve` | 取回被截断并写入 Stash 的 Payload |
| `tokenless env-check` | 检查工具依赖与环境 |
| `tokenless stats` | 查询和控制本地统计 |
| `tokenless mcp serve` | 启动提供取回工具的 MCP stdio 服务 |

使用当前安装版本的帮助查看最终参数定义：

```bash
tokenless --help
tokenless <command> --help
```

## 通用输入规则

压缩和编解码命令支持两种输入方式：

```bash
tokenless compress-response --file response.json

cat response.json | tokenless compress-response
```

- `-f` 是 `--file` 的缩写。
- 不传 `--file` 时必须通过 stdin 提供输入。
- 单次输入上限为 64 MiB。
- JSON 相关命令要求输入为合法 JSON。
- 压缩后没有 Token 收益时，CLI 向 stderr 说明原因，并输出原文。

## `compress-schema`

压缩单个 OpenAI Function Calling Schema：

```bash
tokenless compress-schema -f tool.json
```

压缩 JSON 数组：

```bash
cat tools.json | tokenless compress-schema --batch
```

输入本身是数组时会自动使用 batch 处理。常用参数：

| 参数 | 说明 |
|------|------|
| `-f, --file <path>` | 输入文件；省略时读 stdin |
| `--batch` | 把输入作为 Schema 数组处理 |
| `--agent-id <id>` | 统计中的 Agent 标识 |
| `--session-id <id>` | 统计中的 Session 标识 |
| `--tool-use-id <id>` | 统计中的工具调用标识 |
| `--no-stash` | 不保存被截断的描述；截断将不可逆 |
| `--stash-db <path>` | 覆盖 Stash 数据库；无效路径会被拒绝为覆盖值，并回退到环境变量或默认路径 |

默认处理规则：

| 项目 | 默认值 |
|------|--------|
| 函数描述最大长度 | 256 字符 |
| 参数描述最大长度 | 160 字符 |
| 删除 `examples` | 是 |
| 删除 `title` | 是 |
| 移除描述中的围栏代码和行内代码，再合并空白 | 是 |
| 最大递归深度 | 32 |

示例：

```bash
tokenless compress-schema -f tools.json --batch \
  --agent-id copilot-shell --session-id session-001
```

## `compress-response`

压缩 JSON 响应：

```bash
tokenless compress-response -f response.json
```

默认会移除名称完全匹配且区分大小写的黑名单字段、`null`、空字符串/数组/对象，包括数组中的空项；随后截断长字符串、长数组和超过配置嵌套深度的值。常用参数：

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `-f, --file <path>` | stdin | 输入文件 |
| `--truncate-strings-at <n>` | `4096` | 字符串截断阈值 |
| `--truncate-arrays-at <n>` | `32` | 数组元素保留上限 |
| `--max-depth <n>` | `8` | 最大嵌套深度 |
| `--agent-id <id>` | `cli` | 统计中的 Agent 标识 |
| `--session-id <id>` | — | 统计中的 Session 标识 |
| `--tool-use-id <id>` | — | 统计中的工具调用标识 |
| `--no-stash` | 关闭 | 禁用可逆 Stash |
| `--stash-db <path>` | `~/.tokenless/stash.db` | 覆盖 Stash 数据库；无效路径会被拒绝为覆盖值，CLI 随后回退到环境变量或默认路径 |

覆盖阈值：

```bash
tokenless compress-response -f response.json \
  --truncate-strings-at 2048 \
  --truncate-arrays-at 16 \
  --max-depth 6
```

默认删除的字段名为：

```text
debug, trace, traces, stack, stacktrace, logs, logging
```

字段匹配和截断会改变模型看到的响应表示。处理关键 Payload 前，应先保存样例并对比压缩结果。

Stash 只作用于字符串、数组尾部和深层子树截断。黑名单字段、`null` 和空值会直接移除，不会生成取回标记。

大多数 Adapter 会覆盖这些独立 CLI 默认值。共享 Shell 策略使用 `65536`、`128`、`8`；其他结构化工具策略使用 `1048576`、`65536`、`32`。内容读取类工具会被跳过。详见[框架集成 · Adapter 处理规则](framework-integration.md#adapter-处理规则)。

## `compress-toon` 与 `decompress-toon`

JSON 转 TOON：

```bash
echo '{"name":"Alice","age":30}' | tokenless compress-toon
```

TOON 转 JSON：

```bash
printf 'name: Alice\nage: 30\n' | tokenless decompress-toon
```

往返验证：

```bash
echo '{"name":"test","value":42}' \
  | tokenless compress-toon \
  | tokenless decompress-toon
```

`compress-toon` 支持 `--agent-id`、`--session-id` 和 `--tool-use-id`。编码后无收益时会输出原 JSON，且不记录该次统计。

## `retrieve`

压缩输出中出现以下标记时，说明被截断的内容已写入 Stash：

```text
<<tokenless:0123456789abcdef01234567>>
```

使用裸 Hash 取回：

```bash
tokenless retrieve 0123456789abcdef01234567
```

也可以粘贴包含标记的整行文本：

```bash
tokenless retrieve \
  '<... 12 items truncated, retrieve with <<tokenless:0123456789abcdef01234567>>'
```

覆盖数据库：

```bash
tokenless retrieve 0123456789abcdef01234567 \
  --stash-db ~/.tokenless/stash.db
```

Hash 必须是 24 个十六进制字符，且不区分大小写。SQLite Stash 默认 TTL 为一小时，最多保留 10,000 个有效条目。超过 TTL、被容量策略淘汰、使用 `--no-stash`、处于 dry-run、写入失败或数据库路径不一致时都无法取回。

## `mcp serve`

启动 stdio MCP 服务：

```bash
tokenless mcp serve
```

服务暴露 `tokenless_retrieve` 工具，让支持 MCP 的 Agent 无需执行 Shell 命令即可取回 Stash 内容。MCP 服务必须使用与压缩流程相同的用户和 Stash 数据库。

## `env-check`

检查单个工具：

```bash
tokenless env-check --tool Shell
```

检查全部已声明工具：

```bash
tokenless env-check --all
tokenless env-check --all --json
tokenless env-check --all --checklist
```

状态含义：

| 状态 | 含义 |
|------|------|
| `READY` | 必需依赖、推荐依赖、配置和权限均满足 |
| `PARTIAL` | 必需依赖和权限已满足，但缺少推荐依赖、配置项或网络检查 |
| `NOT_READY` | 缺少必需依赖或权限，不应重复调用该工具 |
| `UNKNOWN` | 依赖规范中没有该工具 |

自动修复：

```bash
tokenless env-check --tool Shell --fix
```

> `--fix` 只尝试修复缺失的必需依赖，不会安装推荐依赖。它可能调用系统包管理器、安装依赖或创建链接。应先阅读普通检查输出，并在明确接受环境变更后使用；需要管理员权限时，按输出提示操作。

## `stats`

```bash
tokenless stats summary
tokenless stats summary --json
tokenless stats list --limit 20
tokenless stats show <record-id>
tokenless stats diff <record-id>
tokenless stats diff --session <session-id>
tokenless stats status
tokenless stats enable
tokenless stats disable
tokenless stats clear --yes
```

双跑对比：

```bash
tokenless stats summary --compare <baseline-session> <active-session>
```

查看单条记录，或一次工具调用中可确认衔接的阶段：

```bash
tokenless stats diff <record-id> -U 5
tokenless stats diff --session <session-id> \
  --tool-use-id <tool-use-id>
```

`stats show` 会输出存储的完整 before/after 文本；`stats diff` 用于解释估算节省并显示变化行。主要选项如下：

| 选项 | 适用范围 | 行为 |
|------|----------|------|
| `<record-id>` | 单条记录 | 与 `--session` 冲突 |
| `--session <id>` | Session | 显示仅含指标的总览 |
| `--tool-use-id <id>` | Session | 展开一次工具调用；必须配合 `--session` |
| `-l, --limit <n>` | Session 总览 | 最多显示的链路数，默认 `20` |
| `--sort saved\|time` | Session 总览 | 默认按节省量降序，也可按时间从新到旧 |
| `-U, --context <n>` | 内容差异 | 每处变化周围的未变化行数，默认 `3` |
| `--no-color` | 文本输出 | 关闭 ANSI 颜色 |
| `--json` | 所有范围 | 输出 schema `1.0` JSON 和结构化 diff hunks |

任一端内容不可用或超过 1 MiB 时不生成内容差异，渲染的 hunk 最多 500 行。单记录和 tool-use 差异可能包含存储的源文本，使用共享终端或收集输出时注意敏感信息。完整说明见[效果度量](measuring-savings.md)和[配置与数据隐私](configuration-and-privacy.md)。

`stats status` 只报告本地统计和 SLS 开关及其来源，因为当前状态读取路径没有读取 compression 开关，所以不显示 `compression_enabled`。该设置应检查 `TOKENLESS_COMPRESSION_ENABLED` 和 `~/.tokenless/config.json`。

## 错误与降级

- CLI 错误写入 stderr，并以非零状态退出。
- Hook/Plugin 通常捕获错误并透传原始响应。
- 无压缩收益不是错误；CLI 会输出原文。
- Stash 写入失败时压缩可能继续，但相关截断内容无法取回。

遇到输入、数据库或 Adapter 错误时，参阅[故障排查](troubleshooting.md)。
