# Hermes Agent 识别方案

## 1. 背景

Hermes Agent 是 Nous Research 开发的自我改进型 AI Agent。它是一个基于 Python 的应用，通过 `uv`/`pip` 安装后以 `hermes` 命令启动，支持 CLI 交互模式和 Gateway 消息网关模式。

discovery 模块当前已支持识别 OpenClaw 和 Cosh 两个 agent，需要新增 Hermes agent 的识别能力。

## 2. Hermes Agent 运行特征分析

### 2.1 安装方式

```bash
# 官方安装脚本
curl -fsSL https://raw.githubusercontent.com/NousResearch/hermes-agent/main/scripts/install.sh | bash

# 手动安装（开发模式）
uv venv venv --python 3.11
source venv/bin/activate
uv pip install -e ".[all,dev]"
```

安装后在 `~/.local/bin/hermes` 生成 Python console-scripts 入口。

### 2.2 运行模式

| 模式 | 启动命令 | 说明 |
|------|---------|------|
| CLI 交互 | `hermes` | 终端 TUI 交互模式 |
| Gateway | `hermes gateway` | 消息网关（Telegram/Discord/Slack 等） |
| Setup | `hermes setup` | 配置向导（短时运行） |
| 其他子命令 | `hermes model`/`hermes tools` 等 | 配置类子命令（短时运行） |

### 2.3 进程特征（实测数据，来源：47.239.201.118）

Hermes v0.12.0 安装路径：`/usr/local/lib/hermes-agent/`

**进程树：**

```
script(3990774)───hermes(3990777)───python(3992710)
  │                   │                   │
  │                   │                   └─ gateway subprocess
  │                   └─ main process (CLI)
  └─ PTY wrapper (noise)
```

| 进程 | comm | cmdline | 说明 |
|------|------|---------|------|
| script 包装 | `script` | `script -qc /usr/local/lib/hermes-agent/venv/bin/hermes /dev/null` | PTY 包装，**不匹配** |
| **主进程** | **`hermes`** | `/usr/local/lib/hermes-agent/venv/bin/python3 /usr/local/lib/hermes-agent/venv/bin/hermes` | **需匹配** |
| **Gateway 子进程** | **`python`** | `/usr/local/lib/hermes-agent/venv/bin/python -m hermes_cli.main gateway run --replace` | **需匹配** |

> **关键发现**：Python console-scripts 入口会将进程名（comm）重命名为 `hermes`，而非 `python3`。这与方案初稿的假设不同。

## 3. 识别方案设计

### 3.1 匹配策略

采用与 CoshMatcher/OpenClawMatcher 一致的 **自定义 Matcher 模式**，核心逻辑：

```
comm 为 "hermes"（直接匹配） OR (comm 为 python/python3 且 cmdline 包含 "hermes")
```

### 3.2 匹配规则

**Case 1：直接匹配（主进程）**

```text
comm 匹配 "hermes"（版本后缀容忍）
```

Python console-scripts 入口会将进程名重命名为 `hermes`，这是最直接的匹配方式。

**Case 2：Python + cmdline 匹配（Gateway 子进程）**

```text
comm 匹配 "python3" 或 "python"（版本后缀容忍）
AND cmdline_args 中存在包含 "hermes" 的参数
```

### 3.3 误识别风险与防范

| 风险场景 | 防范措施 |
|---------|---------|
| 其他名为 hermes 的进程 | Case 1 的 "hermes" 是 Python 重命名后的进程名，其他程序的 hermes 不太可能用此名称 |
| Python 进程恰好有 hermes 参数 | Case 2 要求 cmdline 包含 hermes 关键字，实际只有 Hermes agent 的 gateway 子进程满足 |
| script 包装进程 | 明确不匹配 comm 为 "script" 的进程 |
| 其他 Python 服务 | 不含 "hermes" 的 cmdline 不会被匹配 |

## 4. 代码实现方案

### 4.1 新增文件

**`src/agentsight/src/discovery/agents/hermes.rs`**

```rust
//! Hermes agent matcher

use crate::discovery::agent::AgentInfo;
use crate::discovery::matcher::{match_name_with_version_suffix, AgentMatcher, ProcessContext};

pub struct HermesMatcher {
    info: AgentInfo,
}

impl HermesMatcher {
    pub fn new() -> Self {
        Self {
            info: AgentInfo::new(
                "Hermes",
                vec!["hermes", "python3", "python"],
                "Hermes - self-improving AI agent by Nous Research",
                "ai-assistant",
            ),
        }
    }
}

impl AgentMatcher for HermesMatcher {
    fn info(&self) -> &AgentInfo {
        &self.info
    }

    fn matches(&self, ctx: &ProcessContext) -> bool {
        let comm_lower = ctx.comm.to_lowercase();

        // Case 1: Direct "hermes" process
        if match_name_with_version_suffix(&comm_lower, "hermes") {
            return true;
        }

        // Case 2: Python process with "hermes" in cmdline
        let is_python = match_name_with_version_suffix(&comm_lower, "python3")
            || match_name_with_version_suffix(&comm_lower, "python");
        if is_python {
            let has_hermes = ctx.cmdline_args.iter().any(|arg| {
                let arg_lower = arg.to_lowercase();
                arg_lower.contains("hermes")
            });
            if has_hermes {
                return true;
            }
        }

        false
    }
}
```

### 4.2 修改文件

#### 4.2.1 `src/agentsight/src/discovery/agents/mod.rs`

新增 `hermes` 子模块声明：

```rust
pub mod cosh;
pub mod hermes;    // 新增
pub mod openclaw;
```

#### 4.2.2 `src/agentsight/src/discovery/registry.rs`

在 `known_agents()` 中注册 HermesMatcher：

```rust
use super::agents::hermes::HermesMatcher;    // 新增

pub fn known_agents() -> Vec<Box<dyn AgentMatcher>> {
    vec![
        Box::new(OpenClawMatcher::new()),
        Box::new(CoshMatcher::new()),
        Box::new(HermesMatcher::new()),    // 新增
    ]
}
```

## 5. 测试方案

### 5.1 单元测试

| 测试用例 | comm | cmdline_args | 预期结果 | 说明 |
|---------|------|-------------|---------|------|
| 直接匹配（主进程） | `hermes` | `[".../python3", ".../hermes"]` | ✅ 匹配 | console-scripts 重命名 |
| Gateway 子进程 | `python` | `[".../python", "-m", "hermes_cli.main", "gateway", "run"]` | ✅ 匹配 | 实测场景 |
| python3 + hermes | `python3` | `[".../python3", "-m", "hermes_cli.main"]` | ✅ 匹配 | 变体 |
| python3.11 + hermes | `python3.11` | `[".../python3.11", ".../hermes"]` | ✅ 匹配 | 版本后缀 |
| 开发模式 | `python3` | `["python3", "/path/to/hermes-agent/..."]` | ✅ 匹配 | 开发安装 |
| 非 Python 进程 | `node` | `["node", ".../hermes"]` | ❌ 不匹配 | comm 不符 |
| 普通 Python 进程 | `python3` | `["python3", "manage.py", "runserver"]` | ❌ 不匹配 | 无 hermes 关键字 |
| script 包装 | `script` | `["script", "-qc", ".../hermes"]` | ❌ 不匹配 | 不匹配 comm=script |

### 5.2 集成测试（测试环境 47.239.201.118）

**第一阶段：确认进程特征**

```bash
ssh root@47.239.201.118
hermes &
HERMES_PID=$(pgrep -f hermes | head -1)
cat /proc/$HERMES_PID/comm
cat /proc/$HERMES_PID/cmdline | tr '\0' '\n'
```

**第二阶段：构建部署**

```bash
# 本地构建
cd ~/anolisa/src/agentsight && cargo build --release
scp target/release/agentsight root@47.239.201.118:/root/agentsight
```

**第三阶段：验证发现**

```bash
ssh root@47.239.201.118
rm -f /var/log/sysak/.agentsight/genai_events.db*
RUST_LOG=debug /root/agentsight trace > /tmp/agentsight_trace_debug.log 2>&1 &
sleep 3
hermes &
sleep 5
grep -i hermes /tmp/agentsight_trace_debug.log
```

**第四阶段：验证生命周期**

```bash
kill $(pgrep -f hermes)
sleep 3
grep -i 'process exited' /tmp/agentsight_trace_debug.log | tail -5
```

## 6. 与现有 Matcher 的对比

| 特性 | CoshMatcher | OpenClawMatcher | HermesMatcher |
|------|------------|-----------------|---------------|
| 运行时 | Node.js | Node.js / 直部二进制 | Python |
| comm 匹配 | `node` | `openclaw-gatewa` / `node` | `hermes` / `python` / `python3` |
| cmdline 匹配 | `/usr/bin/co` 等固定路径 | `openclaw` + `gateway` | 包含 `hermes` |
| 多模式支持 | 否 | 是（直部 + node） | 是（直部 + python） |
| 忽略包装进程 | 否 | 否 | 是（script 不匹配） |

## 7. 变更记录

| 日期 | 变更 | 原因 |
|------|------|------|
| 2026-05-06 | 初稿：仅匹配 python3 + hermes | 基于文档推测 |
| 2026-05-06 | 修订：增加 comm=hermes 直接匹配 | 实测发现 console-scripts 重命名进程名 |
| 2026-05-06 | 修订：增加 python + hermes_cli 匹配 | 实测发现 gateway 子进程使用 python -m |
| 2026-05-06 | 修订：排除 script 包装进程 | 实测发现 hermes 使用 script 做 PTY 包装 |
