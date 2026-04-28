# Token-Less 用户使用手册

> LLM token optimization toolkit — Schema/Response 压缩 + 命令重写 + TOON 格式

**版本**：0.1.0
**源码**：https://code.alibaba-inc.com/Agentic-OS/Token-Less
**RPM 源码**：https://code.alibaba-inc.com/alinux/tokenless
**系统要求**：Rust 1.70+, Linux (推荐 Alinux 4)

---

## 目录

1. [产品概述](#1-产品概述)
2. [核心功能](#2-核心功能)
3. [系统要求](#3-系统要求)
4. [安装部署](#4-安装部署)
   - [RPM 包安装](#41-方法一-rpm-包安装推荐用于-alinux-4)
   - [源码一键安装](#42-方法二源码一键安装)
   - [安装脚本](#43-方法三使用安装脚本)
   - [分步安装](#44-方法四分步安装)
5. [配置使用](#5-配置使用)
   - [CLI 使用](#51-cli-使用)
   - [RPM 安装后的自动化配置](#52-rpm-安装后的自动化配置)
   - [Copilot Shell 配置](#53-copilot-shell-配置)
   - [OpenClaw 插件配置](#54-openclaw-插件配置)
6. [验证测试](#6-验证测试)
7. [故障排查](#7-故障排查)
8. [附录](#8-附录)
   - [Makefile 命令汇总](#81-makefile-命令汇总)
   - [关键文件路径](#82-关键文件路径)
   - [Fail-Open 设计](#83-fail-open-设计)
   - [默认配置汇总](#84-默认配置汇总)
   - [源码仓库](#85-源码仓库)

---

## 1. 产品概述

**Token-Less** 是一款 LLM token 优化工具包，通过 **Schema/响应压缩**、**命令重写**和 **TOON 格式**三种策略，显著降低 LLM token 消耗。

### 1.1 核心价值

| 功能 | 节省比例 | 说明 |
|------|---------|------|
| Schema 压缩 | ~57% | 压缩 OpenAI Function Calling 工具定义 |
| 响应压缩 | ~26–78% | 压缩 API/工具响应（因内容而异） |
| 命令重写 | 60–90% | 通过 RTK 过滤 CLI 命令输出 |
| TOON 格式 | 30–60% | 无损 JSON→二进制格式压缩，适合结构化/表格数据 |

### 1.2 支持的集成方式

| 集成方式 | 命令重写 | 响应压缩 | Schema 压缩 |
|---------|---------|---------|------------|
| OpenClaw 插件 | ✅ | ✅ | ⏳ (受限于 OpenClaw hook 系统) |
| Copilot Shell Hook | ✅ | ✅ | ⏳ (等待协议扩展) |

### 1.3 架构概览

```
Token-Less/
├── crates/tokenless-schema/   # 核心库：SchemaCompressor + ResponseCompressor
├── crates/tokenless-cli/      # CLI 二进制：tokenless 命令
├── crates/tokenless-stats/    # 统计记录库（SQLite）
├── openclaw/                  # OpenClaw 插件（TypeScript）
├── hooks/copilot-shell/       # Copilot Shell Hooks
├── third_party/rtk/           # RTK 子模块（命令重写引擎）
├── third_party/toon/          # TOON 子模块（二进制 JSON 编解码器）
├── Makefile                   # 统一构建系统
├── scripts/install.sh         # 一键安装脚本
└── docs/                      # 文档
```

---

## 2. 核心功能

### 2.1 Schema 压缩器 (SchemaCompressor)

压缩 OpenAI Function Calling 工具定义，减少进入上下文窗口的结构性开销。

**源码位置**：`crates/tokenless-schema/src/schema_compressor.rs`

### 2.2 响应压缩器 (ResponseCompressor)

递归遍历 JSON 值，应用 **7 条压缩规则** 缩减 token 消耗。

**源码位置**：`crates/tokenless-schema/src/response_compressor.rs`

#### 7 条压缩规则

| 规则 | 名称 | 判断条件 | 处理方式 | 默认阈值 |
|------|------|---------|---------|---------|
| R1 | 字符串截断 | 长度 > 512 字节 | 在 UTF-8 安全边界截断，追加 `… (truncated)` | 512 字节 |
| R2 | 数组截断 | 元素 > 16 个 | 保留前 16 个，追加 `<... N more items truncated>` | 16 个 |
| R3 | 字段删除 | key 匹配黑名单 | 整个字段移除 | 7 个字段 |
| R4 | null 移除 | 值为 `null` | 从对象/数组中删除 | 启用 |
| R5 | 空值移除 | 值为 `""`/`[]`/`{}` | 从对象/数组中删除 | 启用 |
| R6 | 深度截断 | 嵌套深度 > 8 | 替换为 `<{type} truncated at depth {N}>` | 8 层 |
| R7 | 原始类型保留 | bool/number | 直接保留 | — |

**R3 默认黑名单字段**：`debug`, `trace`, `traces`, `stack`, `stacktrace`, `logs`, `logging`

#### 压缩前后示例

**示例 1 — 字段删除 + null 移除 + 空值移除（R3 + R4 + R5）**

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

**示例 2 — 数组截断（R2）**

输入：
```json
[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
```

输出：
```json
[1, 2, 3, "<... 7 more items truncated>"]
```

### 2.3 命令重写 (RTK)

集成 [RTK](https://github.com/rtk-ai/rtk) 过滤和重写 CLI 命令输出，支持 70+ 命令。

| 原始命令 | 重写后 | 典型节省 |
|---------|--------|---------|
| `cargo build` | `rtk build` | ~70% |
| `cargo test` | `rtk test` | ~80% |
| `npm run build` | `rtk build` | ~65% |
| `go test ./...` | `rtk test` | ~75% |
| `python -m pytest` | `rtk test` | ~85% |

### 2.4 TOON 压缩 (TOON 格式)

TOON（Token-Oriented Object Notation）是一种**无损二进制 JSON 编解码器**，通过消除 JSON 语法开销（引号、逗号、冒号、花括号）来减少 token 消耗，同时完整保留所有数据。对于结构化数据和表格数据效果尤为显著。

**源码位置**：通过 `third_party/toon/` 子模块集成，由 CLI 作为子进程调用。

#### TOON 工作原理

TOON 将 JSON 的文本语法替换为紧凑的二进制编码：

| JSON 元素 | JSON 语法 | TOON 编码 | 节省效果 |
|-----------|-----------|----------|---------|
| 对象键名 | `"name":`（引号+冒号） | 长度前缀的原始字节 | 键名密集对象约 60-80% |
| 字符串值 | `"value"`（引号） | 长度前缀的原始字节 | 约 10-20% |
| 数组分隔符 | `, `（逗号+空格） | 隐式元素边界 | 100% |
| 结构花括号 | `{}`, `[]` | 通过类型标记隐式表示 | 100% |
| 数字/布尔 | 文本表示 | 紧凑的二进制编码 | 约 30-50% |

#### 不同数据类型的压缩效果

| 数据类型 | 典型 TOON 节省 | 示例 |
|---------|---------------|------|
| 表格/数组数据 | 40-60% | `[{"id":1,"name":"A"},...]`（实测 44%） |
| 简单扁平对象 | 10-20% | `{"name":"Alice","age":30}`（实测 15%） |
| 嵌套 schema 定义 | 5-15% | 函数调用工具 schema |

#### TOON vs 响应压缩

| 方面 | 响应压缩 | TOON 压缩 |
|------|---------|----------|
| 策略 | 有损（截断、字段删除） | 无损（完整数据保留） |
| 最佳场景 | 冗长的 API 响应、含 debug 字段的输出 | 结构化表格数据、API 结果 |
| 数据完整性 | 可能删除字段或截断字符串 | 完全可往返（编码→解码） |
| 链式使用 | 流水线中先执行 | 流水线中后执行（响应压缩之后） |

#### 链式压缩流水线

在 Copilot Shell PostToolUse hook 中，响应压缩和 TOON 压缩**顺序执行**：

```
工具响应 → ResponseCompressor（有损） → TOON 编码（无损） → 最终输出
```

这种两阶段流水线最大化节省效果：响应压缩先剥离冗余内容和调试字段，TOON 再消除剩余的 JSON 语法开销。

---

## 3. 系统要求

| 依赖 | 版本要求 | 用途 | 必需 |
|------|---------|------|------|
| Rust | >= 1.70 (stable) | 编译 tokenless 和 rtk | 构建时需要 |
| Git | 任意 | 子模块管理 | 构建时需要 |
| jq | 任意 | Hook 脚本 JSON 处理 | 是 |
| rtk | >= 0.28.0 | 命令重写 | 可选 |
| toon | >= 0.1.0 | TOON 格式压缩 | 可选 |
| tokenless | >= 0.1.0 | Schema/响应压缩 | 可选 |
| sqlite3 | 任意 | 统计数据库 | 可选 |

**注意**：Rust 和 Git 仅在源码编译时需要，使用 RPM 包安装无需这些依赖。

---

## 4. 安装部署

### 4.1 方法一：RPM 包安装（推荐用于 Alinux 4）

#### 4.1.1 构建 RPM 包

```bash
# 准备 RPM 构建环境
rpmdev-setuptree

# 复制源码到 RPM 构建目录
cp tokenless-0.1.0.tar.gz ~/rpmbuild/SOURCES/

# 使用 spec 文件构建 RPM
rpmbuild -ba tokenless.spec

# 生成的 RPM 包位置
~/rpmbuild/RPMS/x86_64/tokenless-0.1.0-3.alnx4.x86_64.rpm
```

#### 4.1.2 安装 RPM 包

```bash
# 使用 yum 安装（推荐，自动解决依赖）
sudo yum install ./tokenless-0.1.0-3.alnx4.x86_64.rpm

# 或使用 rpm 直接安装
sudo rpm -ivh tokenless-0.1.0-3.alnx4.x86_64.rpm
```

#### 4.1.3 RPM 包自动配置

RPM 包安装后会自动执行以下配置：

1. **二进制文件**：安装到 `/usr/bin/tokenless` 和 `/usr/bin/rtk`
2. **Hook 脚本**：安装到 `/usr/share/tokenless/adapters/cosh/`
3. **OpenClaw 插件**：自动检测并配置（如果已安装 OpenClaw）
4. **Copilot Shell**：自动检测并配置（如果已安装 Copilot Shell）

**验证 RPM 安装**：
```bash
# 检查二进制文件
which tokenless
# 输出：/usr/bin/tokenless

tokenless --version

# 检查 Hook 脚本（RPM 安装位置）
ls -la /usr/share/tokenless/adapters/cosh/

# 检查 OpenClaw 插件配置
cat ~/.openclaw/openclaw.json | jq '.plugins.allow'
```

### 4.2 方法二：源码一键安装

```bash
# 克隆仓库（包含子模块）
git clone --recursive https://code.alibaba-inc.com/Agentic-OS/Token-Less
cd Token-Less

# 完整安装：编译 + 安装二进制 + 部署 OpenClaw 插件 + Copilot Shell Hook
make setup
```

### 4.3 方法三：使用安装脚本

```bash
# 自动检测安装源并配置
./scripts/install.sh

# 强制源码安装
./scripts/install.sh --source

# RPM 安装后的手动配置
./scripts/install.sh --install

# 卸载清理
./scripts/install.sh --uninstall

# 仅手动配置 OpenClaw 插件
./scripts/install.sh --openclaw

# 仅手动配置 copilot-shell hooks
./scripts/install.sh --cosh
```

### 4.4 方法四：分步安装

#### 4.4.1 编译

```bash
# 编译 tokenless + rtk（release 模式）
make build

# 仅编译 tokenless
make build-tokenless

# 仅编译 rtk
make build-rtk
```

#### 4.4.2 安装二进制文件

```bash
# 安装到 /usr/share/tokenless/bin（默认）
make install

# 自定义安装路径
make install INSTALL_DIR=/usr/local/bin
```

#### 4.4.3 部署 OpenClaw 插件

```bash
# 使用 Makefile
make openclaw-install

# 自定义插件路径
make openclaw-install OPENCLAW_DIR=/usr/share/tokenless/openclaw

# 手动安装
cp -r openclaw/ /usr/share/tokenless/openclaw/
```

#### 4.4.4 部署 Copilot Shell Hook

```bash
# 使用 Makefile
make copilot-shell-install

# 手动安装
mkdir -p /usr/share/tokenless/adapters/cosh
cp hooks/copilot-shell/tokenless-*.sh /usr/share/tokenless/adapters/cosh/
chmod +x /usr/share/tokenless/adapters/cosh/tokenless-*.sh
```

---

## 5. 配置使用

### 5.1 CLI 使用

#### compress-schema

```bash
# 从文件压缩单个工具 schema
tokenless compress-schema -f tool.json

# 从 stdin 压缩
cat tool.json | tokenless compress-schema

# 批量压缩工具数组
tokenless compress-schema -f tools.json --batch
```

#### compress-response

```bash
# 从文件压缩 API 响应
tokenless compress-response -f response.json

# 从 stdin 压缩
curl -s https://api.example.com/data | tokenless compress-response
```

#### compress-toon

```bash
# 将 JSON 编码为 TOON 格式（从文件）
tokenless compress-toon -f data.json

# 从 stdin 编码
cat data.json | tokenless compress-toon

# 附带统计追踪信息
tokenless compress-toon -f data.json --agent-id my-agent --session-id sess-001
```

#### decompress-toon

```bash
# 将 TOON 格式解码回 JSON（从文件）
tokenless decompress-toon -f data.toon

# 从 stdin 解码
cat data.toon | tokenless decompress-toon

# 往返验证
tokenless compress-toon -f data.json | tokenless decompress-toon | jq .
```

### 5.2 RPM 安装后的自动化配置 {#52-rpm-安装后的自动化配置}

RPM 包安装后，安装脚本会自动检测并配置已安装的平台。

#### 5.2.1 自动检测的平台

| 平台 | 检测条件 | 自动配置内容 |
|------|---------|-------------|
| OpenClaw | `~/.openclaw/openclaw.json` 存在 | 插件部署 + plugins.allow 配置 |
| Copilot Shell | `~/.copilot-shell/settings.json` 或 `~/.qwen-code/settings.json` 存在 | Hook 脚本部署 + settings.json 配置 |

#### 5.2.2 手动触发配置

如果 RPM 安装后需要重新配置，运行：

```bash
# 完整自动检测和配置
/usr/share/tokenless/scripts/install.sh --install

# 或仅配置单个平台
/usr/share/tokenless/scripts/install.sh --cosh      # 仅 copilot-shell hooks
/usr/share/tokenless/scripts/install.sh --openclaw  # 仅 OpenClaw 插件
```

#### 5.2.3 验证自动配置

```bash
# 检查 OpenClaw 插件配置
cat ~/.openclaw/openclaw.json | jq '.plugins.allow'
# 应包含 "tokenless-openclaw"

# 检查 Copilot Shell Hook 配置
cat ~/.copilot-shell/settings.json | jq '.hooks | keys'
# 应包含 PreToolUse, PostToolUse, BeforeModel

# 检查 Hook 脚本
ls -la /usr/share/tokenless/adapters/cosh/
```

### 5.3 Copilot Shell 配置

#### 5.3.1 Hook 脚本位置

安装后 Hook 脚本位置取决于安装方式：

| 安装方式 | Hook 脚本位置 |
|---------|--------------|
| RPM 安装 | `/usr/share/tokenless/adapters/cosh/` |
| 源码安装 | `/usr/share/tokenless/adapters/cosh/` |

| 脚本 | 功能 | Hook 事件 |
|------|------|----------|
| `tokenless-rewrite.sh` | 命令重写 | PreToolUse |
| `tokenless-compress-response.sh` | 响应压缩 + TOON 压缩流水线 | PostToolUse |
| `tokenless-compress-schema.sh` | Schema 压缩 | BeforeModel |

#### 5.3.2 配置 settings.json

编辑 `~/.copilot-shell/settings.json`（或 `~/.qwen-code/settings.json`）：

**RPM 安装配置**：
```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Shell",
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/adapters/cosh/tokenless-rewrite.sh",
            "name": "tokenless-rewrite",
            "timeout": 5000
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/adapters/cosh/tokenless-compress-response.sh",
            "name": "tokenless-compress-response",
            "timeout": 10000
          }
        ]
      }
    ],
    "BeforeModel": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/adapters/cosh/tokenless-compress-schema.sh",
            "name": "tokenless-compress-schema",
            "timeout": 10000
          }
        ]
      }
    ]
  }
}
```

**源码安装配置**：
```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Shell",
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/adapters/cosh/tokenless-rewrite.sh",
            "name": "tokenless-rewrite",
            "timeout": 5000
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/adapters/cosh/tokenless-compress-response.sh",
            "name": "tokenless-compress-response",
            "timeout": 10000
          }
        ]
      }
    ],
    "BeforeModel": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/adapters/cosh/tokenless-compress-schema.sh",
            "name": "tokenless-compress-schema",
            "timeout": 10000
          }
        ]
      }
    ]
  }
}
```

> **提示**：RPM 安装会自动配置 settings.json，无需手动编辑。

#### 5.3.3 Hook 工作流程

**命令重写 (PreToolUse)**：
```
copilot-shell 触发 PreToolUse 
  → Hook 读取 stdin JSON 
  → 调用 rtk rewrite 
  → 返回重写后的命令
```

**响应压缩 (PostToolUse)**：
```
copilot-shell 触发 PostToolUse 
  → Hook 读取 tool_response 
  → 第 1 步：tokenless compress-response（有损 — 移除 debug、null 字段，截断过长内容）
  → 第 2 步：tokenless compress-toon（无损 — 消除 JSON 语法开销）
  → 两步均采用 fail-open 策略：任何一步失败都会透传原始内容
  → 返回压缩后的内容作为 additionalContext
```

**Schema 压缩 (BeforeModel)**：
```
copilot-shell 触发 BeforeModel 
  → Hook 读取 llm_request.tools 
  → 调用 tokenless compress-schema --batch 
  → 返回压缩后的 tools 数组
```

> **注意**：Schema 压缩目前为功能占位，等待 anolisa 协议扩展将 `tools` 包含在 LLMRequest 中。

### 5.4 OpenClaw 插件配置

#### 5.4.1 配置文件

编辑 `openclaw.plugin.json`：

```json
{
  "rtk_enabled": true,
  "schema_compression_enabled": true,
  "response_compression_enabled": true,
  "toon_compression_enabled": false,
  "skip_tools": [],
  "verbose": false
}
```

| 选项 | 默认值 | 说明 |
|------|-------|------|
| `rtk_enabled` | `true` | 启用 RTK 命令重写 |
| `schema_compression_enabled` | `true` | 启用工具 Schema 压缩 |
| `response_compression_enabled` | `true` | 启用工具响应压缩 |
| `toon_compression_enabled` | `false` | 启用 TOON 格式压缩（在响应压缩之后顺序执行） |
| `skip_tools` | `[]` | 压缩时跳过的工具名称列表（如 `["Read","Glob"]`） |
| `verbose` | `false` | 输出详细日志 |

#### 5.4.2 集成细节

**响应压缩跳过逻辑**：
- 当 RTK 启用且 `toolName === "exec"` 时，跳过压缩（避免双重优化）
- 自动压缩所有其他工具类型的结果（`web_search`, `web_fetch`, `read_file` 等）
- 实测节省：`web_fetch` 约 **~78%**

**Hook 事件**：
| Hook | 事件 | 功能 |
|------|------|------|
| Command rewriting | `before_tool_call` | 重写 `exec` 命令为 RTK 等效命令 |
| Schema compression | `before_tool_register` | 压缩工具 Schema |
| Response compression | `tool_result_persist` | 压缩工具响应 |
| TOON compression | `tool_result_persist` | 顺序执行 TOON 编码（需启用） |

---

## 6. 验证测试

### 6.0 实测效果展示

#### 6.0.1 测试方法

**响应压缩测试脚本：**

```bash
#!/usr/bin/bash
# 测试 tokenless-compress-response 的 mock 输入

# 构建长工具响应（>200 字节阈值）
LONG_STDOUT=""
for i in $(seq 1 50); do
  LONG_STDOUT="${LONG_STDOUT}This is line $i of verbose output from a tool execution with lots of text to compress.\n"
done

MOCK_RESPONSE="{\"stdout\":\"${LONG_STDOUT}\",\"stderr\":\"\",\"exit_code\":0}"
INPUT="{\"tool_name\":\"run_shell_command\",\"tool_response\":${MOCK_RESPONSE}}"

echo "=== 原始响应大小：${#INPUT} 字节 ==="

RESULT=$(echo "$INPUT" | bash /root/.copilot-shell/hooks/tokenless/tokenless-compress-response.sh 2>/dev/null)

echo "=== 结果 ==="
echo "$RESULT" | jq '.'

echo ""
echo "=== 压缩后上下文大小：$(echo "$RESULT" | jq -r '.hookSpecificOutput.additionalContext // empty' | wc -c) 字节 ==="
echo "=== suppressOutput: $(echo "$RESULT" | jq '.suppressOutput') ==="
```

**测试设置：**
- 生成 50 行长命令输出
- 模拟 run_shell_command 工具响应
- 测量原始大小与压缩后大小
- 验证 hook 输出格式

#### 6.0.2 测试结果

| 指标 | 数值 |
|------|------|
| 原始响应大小 | 4480 字节 |
| 压缩后大小 | 625 字节 |
| **节省比例** | **~86%** |
| suppressOutput | true (原始输出被抑制) |

#### 6.0.3 生产环境验证

**Hook 执行日志检查：**
```bash
# 检查 compress-response hook 触发
grep "tokenless-compress-response\|compress-response\|compressed response" ~/.copilot-shell/debug/*.log | head -10

# 输出：找到 3 处匹配 - hook 正确触发
```

**PostToolUse Hook 执行次数：**
```bash
# 检查 PostToolUse hook 执行
grep "firePostToolUseEvent\|PostToolUse.*completed" ~/.copilot-shell/debug/*.log | head -20

# 输出：16 处匹配 - PostToolUse hook 正常触发
# 注意：compress-response 仅触发 3 次，因为 hook 跳过 < 200 字节的响应
```

**验证结论：**
- ✅ tokenless-compress-response hook 完全可用
- ✅ Hook 按设计跳过短响应（< 200 字节）（fail-open 优化）
- ✅ 实际压缩比符合预期的 ~86% 节省

---

### 6.1 手动测试 Hook

```bash
# 测试命令重写（源码目录）
echo '{"tool_input":{"command":"cargo test"}}' | bash hooks/copilot-shell/tokenless-rewrite.sh

# 测试响应压缩（源码目录）
echo '{"tool_name":"Shell","tool_response":"{\"stdout\":\"lots of verbose output here...\"}"}' | bash hooks/copilot-shell/tokenless-compress-response.sh

# 测试 Schema 压缩（源码目录）
echo '{"llm_request":{"tools":[{"name":"test","description":"A test tool","parameters":{}}]}}' | bash hooks/copilot-shell/tokenless-compress-schema.sh

# 测试已安装的 Hook（RPM 安装）
echo '{"tool_input":{"command":"cargo test"}}' | bash /usr/share/tokenless/adapters/cosh/tokenless-rewrite.sh
```

### 6.2 测试 CLI

```bash
# 创建测试文件
echo '{"status":"success","data":{"items":[1,2,3]},"debug":{"id":"abc"}}' > test.json

# 压缩响应
tokenless compress-response -f test.json

# 压缩 Schema
echo '[{"name":"Shell","description":"Run shell commands","parameters":{"type":"object"}}]' | tokenless compress-schema

# TOON 编码
echo '{"users":[{"id":1,"name":"Alice"},{"id":2,"name":"Bob"}]}' | tokenless compress-toon

# TOON 往返验证
echo '{"name":"test","value":42}' | tokenless compress-toon | tokenless decompress-toon

# 通过统计数据库验证压缩效果
tokenless compress-toon -f large_data.json --agent-id test
tokenless stats list              # 列出最近的压缩记录
tokenless stats show <记录ID>     # 查看某条记录的压缩前后文本
tokenless stats summary           # 查看所有操作的汇总节省数据
```

### 6.3 验证安装

```bash
# 检查二进制文件
which tokenless
which rtk

# 检查版本
tokenless --version
rtk --version

# 检查 Hook 脚本（RPM 安装）
ls -la /usr/share/tokenless/adapters/cosh/

# 检查 Hook 脚本（源码安装）
ls -la /usr/share/tokenless/adapters/cosh/
```

---

## 7. 故障排查

### 7.1 Copilot Shell Hook

| 问题 | 解决方案 |
|------|---------|
| Hook 不触发 | 检查 `settings.json` 路径，重启 Copilot Shell |
| `jq not installed` | 安装 jq：`apt install jq` (Linux) 或 `brew install jq` (macOS) |
| `rtk too old` | 升级：`cargo install rtk` |
| 命令未重写 | 不是所有命令都有 RTK 等效命令，直接运行 `rtk rewrite "cmd"` 测试 |
| `tokenless not installed` | 运行 `make install` 安装 |
| 响应未压缩 | 响应 < 200 字节时跳过压缩（不值得） |
| Schema 压缩未激活 | 等待 anolisa 协议添加 `tools` 到 LLMRequest |
| JSON 解析错误 | 使用 `jq . < settings.json` 验证 JSON 格式 |
| TOON 编码失败 | 确认 `toon` 二进制在 PATH 中；仅支持 JSON 输入 |
| TOON 统计未记录 | 确认 `TOKENLESS_STATS_ENABLED` 未设置为 `0` 或 `false` |

### 7.2 OpenClaw 插件

| 问题 | 解决方案 |
|------|---------|
| 插件未加载 | 检查插件路径：`~/.openclaw/plugins/tokenless-openclaw/` |
| RTK 未生效 | 确认 `rtk` 在 `$PATH` 中，检查 `rtk_enabled` 配置 |
| 压缩未生效 | 检查 `response_compression_enabled` 配置 |
| TOON 压缩未生效 | 检查 `toon_compression_enabled` 配置，确认 `toon` 二进制在 PATH 中 |
| 超时 | 插件超时设置为 2-3 秒，复杂操作可能超时跳过 |

### 7.3 通用问题

```bash
# 重新编译安装
make clean && make build && make install

# 检查依赖
cargo --version
git --version
jq --version

# 查看日志
# OpenClaw: 设置 verbose: true 查看详细日志
# Copilot Shell: 查看 ~/.copilot-shell/logs/
```

---

## 8. 附录

### 8.1 Makefile 命令汇总

| 命令 | 功能 |
|------|------|
| `make build` | 编译 tokenless + rtk |
| `make build-tokenless` | 仅编译 tokenless |
| `make build-rtk` | 仅编译 rtk |
| `make build-toon` | 从子模块编译 TOON 编解码器 |
| `make install` | 安装二进制到 INSTALL_DIR |
| `make test` | 运行测试 |
| `make test-toon` | 运行 TOON 专项测试 |
| `make lint` | 运行 clippy 检查 |
| `make fmt` | 格式化代码 |
| `make clean` | 清理构建产物 |
| `make openclaw-install` | 安装 OpenClaw 插件 |
| `make openclaw-uninstall` | 卸载 OpenClaw 插件 |
| `make copilot-shell-install` | 安装 Copilot Shell Hook |
| `make copilot-shell-uninstall` | 卸载 Copilot Shell Hook |
| `make setup` | 完整安装：编译 + 安装 + 插件部署 |

### 8.2 关键文件路径

| 用途 | 文件路径 |
|------|---------|
| 核心压缩算法 | `crates/tokenless-schema/src/response_compressor.rs` |
| Schema 压缩 | `crates/tokenless-schema/src/schema_compressor.rs` |
| CLI 子命令 | `crates/tokenless-cli/src/main.rs` |
| 统计记录器（SQLite） | `crates/tokenless-stats/src/recorder.rs` |
| 统计记录类型 | `crates/tokenless-stats/src/record.rs` |
| OpenClaw 插件 | `openclaw/index.ts` |
| OpenClaw 插件配置 | `openclaw/openclaw.plugin.json` |
| Copilot Hook — 命令重写 | `hooks/copilot-shell/tokenless-rewrite.sh` |
| Copilot Hook — 响应压缩 | `hooks/copilot-shell/tokenless-compress-response.sh` |
| Copilot Hook — Schema 压缩 | `hooks/copilot-shell/tokenless-compress-schema.sh` |
| TOON 编解码器（子模块） | `third_party/toon/` |
| 统计数据库（默认） | `~/.tokenless/stats.db` |
| 集成测试 | `crates/tokenless-schema/tests/integration_test.rs` |
| TOON 端到端测试 | `tests/test-toon-full.sh` |
| 全量测试套件 | `tests/run-all-tests.sh` |

### 8.3 Fail-Open 设计

所有集成路径均采用 **fail-open** 策略：

- **OpenClaw 插件**：try-catch 返回 null → 原始结果透传
- **Copilot Shell Hook**：任何失败点均 `exit 0` 且不输出 → 原始结果透传
- **CLI**：错误输出到 stderr，调用方检查退出码决定是否回退

### 8.4 默认配置汇总

| 参数 | 默认值 | Builder 方法 |
|------|-------|-------------|
| `truncate_strings_at` | 512 | `with_truncate_strings_at(len)` |
| `truncate_arrays_at` | 16 | `with_truncate_arrays_at(len)` |
| `drop_nulls` | true | `with_drop_nulls(bool)` |
| `drop_empty_fields` | true | `with_drop_empty_fields(bool)` |
| `max_depth` | 8 | `with_max_depth(depth)` |
| `add_truncation_marker` | true | `with_add_truncation_marker(bool)` |
| `drop_fields` | 7 个 | `add_drop_field(field)` |

### 8.5 源码仓库

| 项目 | 地址 |
|------|------|
| Token-Less 源码 | https://code.alibaba-inc.com/Agentic-OS/Token-Less |
| RPM 构建源码 | https://code.alibaba-inc.com/alinux/tokenless |

---

**许可证**：MIT
**文档版本**：1.2
**最后更新**：2026-04-25
