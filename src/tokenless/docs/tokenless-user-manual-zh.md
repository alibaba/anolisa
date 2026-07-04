# Token 优化（tokenless）

tokenless 是 ANOLISA 的 Token 节省工具包，通过 Schema 压缩、响应压缩、TOON 编码、命令重写和工具就绪检查五条互补策略，在工具定义、API 返回和命令输出进入 LLM 上下文窗口之前削减冗余，从而降低 Agent 运行时的 Token 消耗和重试成本。

- **Schema 与响应压缩**：压缩 OpenAI Function Calling 工具定义和 API 返回结果，分别减少约 57% 和 26–78% 的 Token。
- **TOON 编码**：将 JSON 响应编码为 Token 导向的紧凑格式，结构化数据可再省 15–40%。
- **命令重写**：集成 RTK 对 70+ 常见 CLI 命令输出做过滤和重写，消除 60–90% 的无用噪声。
- **工具就绪检查**：执行前检查二进制、配置、权限和网络依赖，自动修复缺失项并标记环境类失败，避免无效重试浪费 Token。
- **统计与可观测**：每次压缩记录字符/Token 节省度量，支持 SQLite 汇总与 SLS JSONL 采集，可双跑对比真实节省。

---

## 能力概览

| 策略 | Token 节省 | 说明 |
|------|----------|------|
| Schema 压缩 | ~57% | 压缩 OpenAI Function Calling 工具定义 |
| 响应压缩 | 26–78% | 压缩 API / 工具返回结果（移除 debug/null/空值，截断长内容） |
| TOON 编码 | 15–40% | JSON 编码为无损紧凑格式，链在响应压缩之后 |
| 命令重写 | 60–90% | 通过 RTK 过滤 70+ 种 CLI 命令输出 |
| 工具就绪检查 | 减少重试浪费 | 执行前检查环境依赖，自动修复，失败归因 |

---

## 安装

### 通过 anolisa CLI（推荐）

```bash
anolisa install tokenless
```

安装产物：`tokenless`、`rtk`、`toon` 三个二进制，以及适配器资源（hooks、插件、tool-ready-spec、env-fix 脚本）。

### RPM 包（Alinux 4）

```bash
sudo dnf install tokenless
```

RPM 安装到系统级 FHS 路径：`/usr/bin/tokenless`、`/usr/libexec/anolisa/tokenless/{rtk,toon}`、`/usr/share/anolisa/adapters/tokenless/`、`/usr/share/anolisa/extensions/tokenless/`。`%post` 会清理旧版残留（pre-FHS 布局、旧 `tokenless-openclaw` 插件 id 等），cosh extension 由 copilot-shell 自动发现，无需手动改 `settings.json`。

### 源码构建（开发者）

```bash
git clone https://github.com/alibaba/anolisa
cd anolisa/src/tokenless

make setup    # 编译 + 安装二进制 + 注册全部适配器
```

构建依赖：Rust ≥ 1.89（edition 2024）、just、Git、nodejs+npm（编译 OpenClaw TS 插件）。运行时依赖：python3、jq、bash（RPM Requires）。详见 [附录 · 安装路径](#安装路径)。

---

## CLI 使用

所有子命令支持 `-f <path>` 从文件读取或从 stdin 读取（管道），输入上限 64 MiB。

### 压缩工具 Schema

```bash
# 从文件压缩单个 schema
tokenless compress-schema -f tool.json

# 从 stdin 批量压缩（JSON 数组）
cat tools.json | tokenless compress-schema --batch

# 附带统计追踪
tokenless compress-schema -f tools.json --batch \
  --agent-id copilot-shell --session-id sess-001
```

| 参数 | 说明 |
|------|------|
| `-f, --file <path>` | 输入文件，省略则读 stdin |
| `--batch` | 压缩 JSON 数组（输入是数组时自动启用） |
| `--agent-id` / `--session-id` / `--tool-use-id` | 统计追踪字段 |

### 压缩 API 响应

```bash
tokenless compress-response -f response.json

# 或管道
curl -s https://api.example.com/data | tokenless compress-response

# 覆盖默认阈值
tokenless compress-response -f resp.json \
  --truncate-strings-at 2048 --truncate-arrays-at 16 --max-depth 6
```

| 参数 | 说明 |
|------|------|
| `-f, --file <path>` | 输入文件，省略则读 stdin |
| `--truncate-strings-at <usize>` | 字符串截断阈值（默认 4096） |
| `--truncate-arrays-at <usize>` | 数组截断阈值（默认 32） |
| `--max-depth <usize>` | 嵌套深度上限（默认 8） |

### TOON 编解码

```bash
# JSON → TOON（紧凑格式）
echo '{"name":"Alice","age":30}' | tokenless compress-toon

# TOON → JSON
echo 'name: Alice
age: 30' | tokenless decompress-toon

# 往返验证
echo '{"name":"test","value":42}' | tokenless compress-toon | tokenless decompress-toon
```

`compress-toon` 另支持 `--agent-id`/`--session-id`/`--tool-use-id` 统计追踪。若编码后无收益，CLI 输出原文并 stderr 提示，不记录统计。

### 环境就绪检查

```bash
# 检查指定工具（支持 alias、大小写不敏感）
tokenless env-check --tool Shell

# 检查全部
tokenless env-check --all

# 输出完整两级清单
tokenless env-check --checklist

# 自动修复缺失依赖
tokenless env-check --tool Shell --fix

# 机器可读 JSON（供 hook/插件消费）
tokenless env-check --all --json
```

| 参数 | 说明 |
|------|------|
| `--tool <name>` | 检查单个工具（精确 key → alias → 大小写不敏感） |
| `--all` | 检查 spec 中所有工具 |
| `--fix` | 自动安装缺失/版本过低的依赖（调 `tokenless-env-fix.sh fix-all`） |
| `--checklist` | 输出两级清单（工具类别 → 依赖项） |
| `--json` | 机器可读 JSON |

**状态值**：`READY`（全部满足）/ `PARTIAL`（推荐项缺失，可用）/ `NOT_READY`（必需项缺失，工具不可用）/ `UNKNOWN`（spec 无此工具）。`NOT_READY` 的 JSON 含 `diagnostic` 字段，格式 `[tokenless:ready] <tool>: NOT_READY — required dependency missing: <bin>. Skip retry.`，供 hook 传给 LLM 作为 skip-retry 指引。

### 统计与效果度量

```bash
# 汇总（按操作类型分组）
tokenless stats summary
tokenless stats summary --json

# 列出最近记录
tokenless stats list                  # 默认 20 条
tokenless stats list -l 50

# 查看某条记录的压缩前后文本
tokenless stats show 42

# 清空统计
tokenless stats clear --yes

# 查看开关状态与来源
tokenless stats status

# 启用/禁用统计记录（写 config.json）
tokenless stats enable
tokenless stats disable
```

#### 双跑对比（dry-run baseline vs active）

通过 `TOKENLESS_COMPRESSION_ENABLED` 控制是否真正压缩：

- `1`（默认）：正常压缩，结果进入 LLM 上下文，记录 `mode=active`
- `0`（dry-run）：计算预测节省并记录（`mode=dryrun`），但**输出原文**，上下文不变

对同一任务跑两次即可对照真实节省：

```bash
# 跑 1：dry-run 基线
TOKENLESS_COMPRESSION_ENABLED=0  <跑同一任务>   # session A
# 跑 2：真实压缩
TOKENLESS_COMPRESSION_ENABLED=1  <跑同一任务>   # session B

# 对照
tokenless stats summary --compare <session-A> <session-B>
tokenless stats summary --compare <session-A> <session-B> --json
```

`--compare` 接受恰好两个 session ID；模式不匹配会 stderr 告警但不阻断。无 token 节省的记录不入库。

> 注：tokenless 仅度量它经手的可压缩内容；模型推理 token / 真实计费 token 不在其内。

#### SLS 日志采集

除 SQLite 外，每次压缩可追加 **SLS JSONL 记录**，供 ilogtail/SLS Logtail 摄取。

- **默认开启**（`sls_enabled=true`）。开关：`~/.tokenless/config.json` 的 `sls_enabled` 或 `TOKENLESS_SLS_ENABLED`。
- **输出路径**：默认 `/var/log/anolisa/sls/ops/tokenless.jsonl`，可用 `TOKENLESS_SLS_PATH` 覆盖（必须位于 `/var/log/` 或 `/tmp/` 前缀下，否则回退默认并告警）。
- **文件归属**：SLS JSONL 文件由 **anolisa SLS 组件统一管理**（创建、轮转、删除）。tokenless **不主动管理**——写日志前先判断文件是否存在，**存在才追加，不存在则静默跳过**。tokenless 永不创建/截断/删除该文件。
- **记录字段**：`component.*`、`tokenless.operation`、`tokenless.session_id`/`tool_use_id`/`source_pid`、`tokenless.compression.{before,after}_{chars,tokens}`、`chars_saved`/`tokens_saved` 及百分比。**仅记录度量，不记录压缩原文**，无敏感数据。

```bash
# 快速验证：先由 anolisa SLS 组件或手动建好文件，tokenless 才会写入
mkdir -p /tmp && touch /tmp/tokenless-sls.jsonl
TOKENLESS_SLS_ENABLED=1 TOKENLESS_SLS_PATH=/tmp/tokenless-sls.jsonl \
  tokenless compress-response -f resp.json
tail -n1 /tmp/tokenless-sls.jsonl | jq .
```

---

## Agent 框架集成

tokenless 安装后可透明集成到多种 Agent 框架，用户无感知：

| 框架 | 集成方式 | 覆盖策略 |
|------|----------|----------|
| **cosh**（copilot-shell） | PreToolUse/PostToolUse 钩子 | 工具就绪 + 命令重写 + 响应压缩 + TOON + Schema 压缩 |
| **OpenClaw** | 插件 | 命令重写 + 响应压缩 + Schema 压缩 |
| **Hermes** | 插件 | 工具就绪 + 命令重写 + 响应压缩 + TOON |
| **Qoder** | 插件 | 工具就绪 + 命令重写 + 响应压缩 |
| **Claude Code** | Marketplace 插件 | 工具就绪 + 命令重写 + 响应压缩 + TOON |
| **Codex** | 插件 | 工具就绪 + 命令重写 + 响应压缩 + TOON |
| **Qwen Code** | Extension 插件 | 工具就绪 + 命令重写 + 响应压缩 + Schema 压缩 |

### 启用适配器

```bash
# 查看可用框架
anolisa adapter scan

# 按需启用
anolisa adapter enable tokenless cosh         # cosh 钩子
anolisa adapter enable tokenless openclaw     # OpenClaw 插件
anolisa adapter enable tokenless hermes       # Hermes 插件
anolisa adapter enable tokenless qoder        # Qoder 插件
anolisa adapter enable tokenless claude-code  # Claude Code 插件
anolisa adapter enable tokenless codex        # Codex 插件
anolisa adapter enable tokenless qwencode     # Qwen Code 插件

# 查看状态
anolisa adapter status tokenless

# 禁用
anolisa adapter disable tokenless openclaw
```

也可在 tokenless 源码目录手动注册（等效于 `anolisa adapter enable`）：

```bash
make cosh-extension-install    # cosh 钩子
make openclaw-install          # OpenClaw 插件
make hermes-install            # Hermes 插件
make qoder-install             # Qoder 插件
make claude-code-install       # Claude Code 插件
make codex-install             # Codex 插件
make qwencode-install          # Qwen Code 插件
```

---

## 工作原理

### Schema 压缩

压缩 OpenAI Function Calling 工具定义，去除冗余描述与 markdown 语法。源码：`crates/tokenless-schema/src/schema_compressor.rs`。

默认阈值：

| 参数 | 默认 | 说明 |
|------|------|------|
| `func_desc_max_len` | 256 | 函数描述最大字符数 |
| `param_desc_max_len` | 160 | 参数描述最大字符数 |
| `drop_examples` | true | 删除 `examples` 字段 |
| `drop_titles` | true | 删除 `title` 字段 |
| `drop_markdown` | true | 去除描述中的 markdown 语法 |
| `max_depth` | 32 | 递归深度上限（schema 容忍更深嵌套） |

### 响应压缩（7 条规则）

递归遍历 JSON 值，应用 7 条规则。源码：`crates/tokenless-schema/src/response_compressor.rs`。

| 规则 | 名称 | 判断条件 | 处理方式 | 默认阈值 |
|------|------|---------|---------|---------|
| R1 | 字符串截断 | 长度 > 4096 字符 | UTF-8 安全边界截断，追加 `… (truncated)` | 4096 字符 |
| R2 | 数组截断 | 元素 > 32 | 保留前 32 个，追加 `<... N more items truncated>` | 32 个 |
| R3 | 字段删除 | key 匹配黑名单 | 整个字段移除 | 7 个字段 |
| R4 | null 移除 | 值为 `null` | 从对象/数组删除 | 启用 |
| R5 | 空值移除 | 值为 `""`/`[]`/`{}` | 从对象/数组删除 | 启用 |
| R6 | 深度截断 | 嵌套深度 > 8 | 替换为 `<{type} truncated at depth {N}>` | 8 层 |
| R7 | 原始类型保留 | bool/number | 直接保留 | — |

**R3 默认黑名单字段**：`debug`, `trace`, `traces`, `stack`, `stacktrace`, `logs`, `logging`

示例（R3 + R4 + R5）：

```json
// 输入
{"status":"success","data":{"name":"test","count":42},
 "debug":{"request_id":"abc123"},"trace":"GET /api 200","metadata":null,"tags":[],"extra":""}

// 输出
{"status":"success","data":{"name":"test","count":42}}
```

### TOON 编码

TOON（Token-Oriented Object Notation）是无损 JSON 编解码器，消除 JSON 语法开销（引号、逗号、冒号、花括号），完整保留数据。CLI 通过 `toon-format` crate（v0.5）作为库直接调用；Python hook 通过独立 `toon` 二进制子进程调用。

| JSON 元素 | JSON 语法 | TOON 编码 | 节省 |
|-----------|-----------|----------|---------|
| 对象键名 | `"name":` | 长度前缀原始字节 | 60-80% |
| 字符串值 | `"value"` | 长度前缀原始字节 | 10-20% |
| 数组分隔符 | `, ` | 隐式边界 | 100% |
| 结构花括号 | `{}`, `[]` | 类型标记隐式 | 100% |

### 命令重写（RTK）

RTK 拦截 Shell 命令，重写为仅输出 Agent 需要的关键信息：

```bash
# 原始命令（浪费大量 Token）
ls -la /usr/lib

# RTK 重写后（仅保留关键信息）
rtk rewrite "ls -la /usr/lib"
```

RTK 源码由 justfile 从 GitHub 克隆（`v0.43.0`）并应用 tokenless 专属补丁后构建，支持 70+ 命令（cargo/npm/go/pytest 等），典型节省 60–90%。

### 工具就绪检查（Tool Ready)

每次工具调用前检查依赖（二进制、配置、权限、网络）。若依赖缺失，返回 `NOT_READY` + "Skip retry" 引导，避免 LLM 无谓重试。依赖声明位于 `tool-ready-spec.json`：

```json
{
  "Shell": {
    "required": [{ "binary": "jq", "package": "jq", "manager": "apt" }],
    "recommended": [
      { "binary": "rtk", "version": ">=0.35", "package": "rtk", "manager": "cargo",
        "fallback": [
          { "method": "symlink", "binary": "rtk", "source": "/usr/libexec/anolisa/tokenless/rtk" }
        ]
      }
    ]
  }
}
```

字符串格式 `"jq"` 也支持（自动转为对象）。`--fix` 调用 `tokenless-env-fix.sh fix-all`，stdin 传入缺失依赖的 JSON 数组，自动安装后复检。

### 链式压缩流水线

在 PostToolUse hook 中，响应压缩和 TOON 编码顺序执行，两阶段最大化节省：

```
工具响应 → ResponseCompressor（有损） → TOON 编码（无损） → 最终输出
```

每步 fail-open（失败透传原文）。

---

## 配置

### 配置文件

`~/.tokenless/config.json`（owner-only 0600），字段：

| 字段 | 默认 | 说明 |
|------|------|------|
| `stats_enabled` | `true` | 是否记录压缩统计到 SQLite |
| `sls_enabled` | `true` | 是否追加 SLS JSONL 记录 |
| `compression_enabled` | `true` | 是否真正应用压缩。`false` = dry-run：计算并记录预测节省，但输出原文 |

也可用 CLI 修改：`tokenless stats enable` / `disable` / `status`。

### 环境变量

优先级：**env > config.json > default**（每个开关独立）。空字符串视为未设置。布尔值：`1`/`true`/`yes`（大小写不敏感）为真。

| 变量 | 用途 | 约束 |
|---------|------|------|
| `TOKENLESS_STATS_ENABLED` | 覆盖 `stats_enabled` | — |
| `TOKENLESS_SLS_ENABLED` | 覆盖 `sls_enabled` | — |
| `TOKENLESS_COMPRESSION_ENABLED` | 覆盖 `compression_enabled`（dry-run 开关） | — |
| `TOKENLESS_STATS_DB` | 自定义统计数据库路径 | 须位于用户 home 下，否则忽略并告警 |
| `TOKENLESS_SLS_PATH` | 自定义 SLS JSONL 路径 | 须位于 `/var/log/` 或 `/tmp/` 下，否则回退默认 |
| `TOKENLESS_TOOL_READY_SPEC` | 自定义 tool-ready-spec 路径 | 须通过信任路径校验 |
| `TOKENLESS_ENV_FIX_SCRIPT` | 自定义 env-fix 脚本路径 | 须通过信任路径校验 |
| `TOKENLESS_PACKAGE_MANAGER` | 覆盖包管理器探测（dnf/yum/apt/apk） | 测试用 |
| `TOKENLESS_AGENT_ID` | hook 注入的 agent 标识 | 由 cosh-extension.json 自动设置 |

### OpenClaw 插件配置（`openclaw.plugin.json`）

| 选项 | 默认 | 说明 |
|------|------|------|
| `rtk_enabled` | `true` | 启用 RTK 命令重写 |
| `response_compression_enabled` | `true` | 启用响应压缩 |
| `schema_compression_enabled` | `true` | 启用 Schema 压缩 |
| `verbose` | `true` | 输出详细日志 |

**响应压缩跳过逻辑**：当 RTK 启用且 `toolName === "exec"` 时跳过压缩（避免双重优化）；自动压缩其他工具，实测 `web_fetch` 节省 ~78%。

### 降级行为

每个钩子/插件独立降级——若对应二进制（`rtk` 或 `tokenless`）未安装，该钩子静默跳过，不影响其他功能：

- **cosh Hook**：任何失败点 `exit 0` 且不输出 → 原始结果透传
- **OpenClaw / Hermes / Qoder / Claude Code / Codex / Qwen Code 插件**：try-catch 返回 null → 原始结果透传
- **CLI**：错误输出到 stderr，调用方检查退出码决定是否回退
- **统计记录**：fail-silent，数据库错误不阻塞压缩输出
- **SLS 写入**：fail-silent，仅 stderr 告警

---

## 故障排查

### 诊断工具

```bash
# 组件级诊断 + 自动修复
anolisa doctor tokenless --fix

# 查看详细安装计划
anolisa install tokenless --verbose
anolisa install tokenless --dry-run

# 适配器状态
anolisa adapter status tokenless

# 工具就绪清单
tokenless env-check --all --checklist
```

### 常见问题

| 问题 | 解决方案 |
|------|---------|
| `No input provided` | stdin 是终端且未传 `-f`；用 `echo '...' \| tokenless <cmd>` 或 `-f <path>` |
| `Input exceeds 64 MiB limit` | 输入超 64 MiB 上限；拆分或截断 |
| `JSON parse error` | 输入非合法 JSON；先用 `jq . < input.json` 校验 |
| 压缩后输出原文 + stderr 提示 | 压缩无收益（`after >= before`），属正常；该次不记录统计 |
| dry-run 模式提示 | `TOKENLESS_COMPRESSION_ENABLED=0` 或 config `compression_enabled=false`；输出原文但记录预测值 |
| `Failed to open database` | `~/.tokenless/` 不可写或 `TOKENLESS_STATS_DB` 路径在 home 之外被拒 |
| SLS JSONL 未生成 | 确认 `TOKENLESS_SLS_ENABLED` 未设为 `0`；`TOKENLESS_SLS_PATH` 须在 `/var/log/` 或 `/tmp/` 下；文件须由 anolisa SLS 组件预建 |
| cosh Hook 不触发 | 确认 `COSH_EXTENSION_DIR` 存在 `cosh-extension.json`；重启 copilot-shell |
| `jq not installed` | `dnf install jq` / `apt install jq` |
| 命令未重写 | 非所有命令都有 RTK 等效；直接 `rtk rewrite "cmd"` 测试 |
| Tool Ready 误报 NOT_READY | 检查 `tool-ready-spec.json`；运行 `tokenless env-check --tool <name> --fix` |
| 适配器 enable 失败 | `anolisa adapter scan` 确认框架已安装；`anolisa adapter status tokenless` 查看详情 |
| 手动 dnf 操作后状态不同步 | `anolisa repair tokenless` 修复；或 `anolisa forget tokenless` 清记录后 `anolisa adopt tokenless` 重新纳管 |

### 状态不一致修复

若通过 `dnf remove` / `rpm -e` 直接操作了 ANOLISA 管理的包：

```bash
anolisa repair tokenless         # 修复状态
anolisa forget tokenless         # 仅清除 ANOLISA 记录（不动包）
anolisa adopt tokenless          # 重新纳管
```

---

## 附录

### 安装路径

`INSTALL_PROFILE` 控制安装前缀：`user`（默认，`~/.local`）或 `system`（`/usr`，RPM 使用）。

| Makefile 变量 | user (默认) | system / RPM |
|------|-------------|-------------|
| `PREFIX` | `~/.local` | `/usr` |
| `BINDIR`（tokenless） | `~/.local/bin` | `/usr/bin` |
| `LIBEXECDIR`（rtk, toon） | `~/.local/libexec/anolisa/tokenless` | `/usr/libexec/anolisa/tokenless` |
| `SHARE_DIR`（适配器资源） | `~/.local/share/anolisa/adapters/tokenless` | `/usr/share/anolisa/adapters/tokenless` |
| `COSH_EXTENSION_DIR` | `~/.copilot-shell/extensions/tokenless` | `/usr/share/anolisa/extensions/tokenless` |

`rtk`/`toon` 实际位于 `LIBEXECDIR`，并在 `BINDIR` 建符号链接以便 PATH 发现。源码构建可覆盖：

```bash
make install INSTALL_PROFILE=system DESTDIR=/staging
make setup                # build + install + 注册全部适配器
make adapter-scan         # 查看已注册适配器能力
```

### cosh Hook 清单

| Hook 事件 | 脚本 | 功能 | matcher | timeout |
|----------|------|------|---------|---------|
| PreToolUse | `tool_ready_hook.sh`（bash） | Tool Ready 预检 + 自动修复 + skip-retry | `""`（all，sequential） | 10000ms |
| PreToolUse | `rewrite_hook.py`（python3） | RTK 命令重写 | `^(Bash\|run_shell_command\|terminal\|Shell\|exec\|process)$` | 5000ms |
| PostToolUse | `compress_response_hook.py` | 响应压缩 + TOON + 失败归因 | — | 10000ms |
| BeforeModel | `compress_schema_hook.py` | Schema 压缩 | — | 10000ms |

所有 hook 通过 `TOKENLESS_AGENT_ID=copilot-shell` 标识来源。辅助文件：`hook_utils.py`、`compress_toon_hook.py`、`run-hook.sh`、`tool_categories.json`。

### 关键文件路径

| 用途 | 路径 |
|------|---------|
| 响应压缩算法 | `crates/tokenless-schema/src/response_compressor.rs` |
| Schema 压缩算法 | `crates/tokenless-schema/src/schema_compressor.rs` |
| CLI 入口 | `crates/tokenless-cli/src/main.rs` |
| env-check 实现 | `crates/tokenless-cli/src/env_check.rs` |
| 统计记录器 | `crates/tokenless-stats/src/recorder.rs` |
| 配置加载 | `crates/tokenless-stats/src/config.rs` |
| SLS JSONL writer | `crates/tokenless-stats/src/sls.rs` |
| home 目录解析 | `crates/tokenless-stats/src/home.rs` |
| 适配器 manifest | `adapters/tokenless/manifest.json.in` |
| cosh extension manifest | `adapters/tokenless/common/cosh-extension.json` |
| Tool Ready 依赖 spec | `adapters/tokenless/common/tool-ready-spec.json` |
| 自动修复脚本 | `adapters/tokenless/common/tokenless-env-fix.sh` |
| 统计数据库（默认） | `~/.tokenless/stats.db` |
| 配置文件 | `~/.tokenless/config.json` |
| SLS JSONL（默认） | `/var/log/anolisa/sls/ops/tokenless.jsonl` |
| RPM spec | `tokenless.spec.in` |
| 构建编排 | `justfile` |

### 安全模型

- **不可伪造的身份源**：home 目录通过 `getpwuid_r(getuid())` 查询 passwd，**不信任** `$HOME`/`dirs::home_dir()`（可被任意改写）。
- **数据库路径校验**：`TOKENLESS_STATS_DB` 必须规范解析后位于用户真实 home 下，否则忽略并告警；home 为空时写入 `/dev/null/.tokenless/stats.db`（安全失败）。
- **SLS 路径校验**：`TOKENLESS_SLS_PATH` 必须位于 `/var/log/` 或 `/tmp/` 前缀下且不含 `..`；canonicalize 后校验防符号链接逃逸。
- **Tool Ready 信任路径**：系统前缀（`/usr/share`、`/usr/libexec`、`/usr/lib/anolisa`、`/usr/local/share`）直接信任；其他路径校验文件/父目录 owner（须为 current_uid 或 root）且非 world-writable。`tool_ready_hook.sh` 中的 shell 实现保持同步。
- **配置文件权限**：`~/.tokenless/config.json` 写入后 chmod 0600。
- **输入上限**：stdin/file 读取上限 64 MiB，防 OOM。

### Makefile 命令汇总

| 命令 | 功能 |
|------|------|
| `make build` | 编译 tokenless + rtk + toon + OpenClaw 插件 |
| `make build-tokenless` | 编译 tokenless + rtk（via justfile） |
| `make build-toon` | 安装 toon 二进制 |
| `make build-openclaw-plugin` | 编译 OpenClaw TS 插件 → `dist/index.js` |
| `make install` | build + 安装二进制 + 适配器资源 + cosh extension |
| `make setup` | 完整安装：`install` + `adapter-install` |
| `make test` | 全部测试（Rust + hooks） |
| `make lint` / `make fmt` / `make clean` | clippy / fmt / 清理 |
| `make dist` | 生成源码 tarball（含预补丁 rtk） |
| `make adapter-scan` | 列出已注册适配器能力 |
| `make adapter-install` / `-uninstall` | 注册/注销全部 7 个平台 |
| `make cosh-extension-install` / `-uninstall` | cosh extension |
| `make openclaw-install` / `-uninstall` | OpenClaw 插件 |
| `make hermes-install` / `-uninstall` | Hermes 插件 |
| `make qoder-install` / `-uninstall` | Qoder 插件 |
| `make claude-code-install` / `-uninstall` | Claude Code 插件 |
| `make codex-install` / `-uninstall` | Codex 插件 |
| `make qwencode-install` / `-uninstall` | Qwen Code 插件 |

---

**许可证**：MIT（tokenless core）+ Apache-2.0（vendored rtk）
**版本**：0.6.1
**文档版本**：2.1（对齐 ANOLISA-design user-guide 结构）
