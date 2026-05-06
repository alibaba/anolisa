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

安装后在 `~/.local/bin/hermes` 生成 Python console_scripts 入口。

### 2.2 运行模式

| 模式 | 启动命令 | 说明 |
|------|---------|------|
| CLI 交互 | `hermes` | 终端 TUI 交互模式 |
| Gateway | `hermes gateway` | 消息网关（Telegram/Discord/Slack 等） |
| Setup | `hermes setup` | 配置向导（短时运行） |
| 其他子命令 | `hermes model`/`hermes tools` 等 | 配置类子命令（短时运行） |

### 2.3 进程特征

Hermes 是 Python 应用，其进程特征如下：

| 属性 | 预期值 |
|------|--------|
| `comm`（进程名） | `python3` 或 `python3.XX`（如 `python3.11`） |
| `exe_path`（可执行路径） | Python 解释器路径，如 `/usr/bin/python3.11`、`/home/user/.local/share/hermes/venv/bin/python3` |
| `cmdline_args`（命令行参数） | 包含 hermes 入口脚本路径，如 `/home/user/.local/bin/hermes`，后跟子命令如 `gateway` |

**典型 cmdline 示例：**

```
# CLI 模式
["/home/user/.local/share/hermes/venv/bin/python3", "/home/user/.local/bin/hermes"]

# Gateway 模式
["/home/user/.local/share/hermes/venv/bin/python3", "/home/user/.local/bin/hermes", "gateway"]

# 开发模式
["/path/to/hermes-agent/venv/bin/python3", "/path/to/hermes-agent/scripts/run_hermes.py"]
```

> **注意：** Python console_scripts 入口生成的包装脚本在执行时，`comm` 显示为 `python3`（或带版本号），`exe_path` 指向 Python 解释器，而 `cmdline_args` 的第二个元素是 hermes 入口脚本的路径。

## 3. 识别方案设计

### 3.1 匹配策略

采用与 CoshMatcher/OpenClawMatcher 一致的 **自定义 Matcher 模式**，核心逻辑：

```
comm 匹配 python3（版本后缀容忍） AND cmdline 包含 hermes 相关路径
```

### 3.2 匹配规则

```text
条件 1：comm 为 "python3"（允许版本后缀，如 python3.11、python3.12）
条件 2：cmdline_args 中存在包含 "hermes" 的路径参数
```

**详细匹配逻辑：**

1. **comm 检查**：使用 `match_name_with_version_suffix(&comm_lower, "python3")` 匹配，支持 `python3`、`python3.11`、`python3.12` 等变体
2. **cmdline 检查**：遍历 `cmdline_args`，检查是否有参数包含 `hermes` 关键字（大小写不敏感），典型匹配路径包括：
   - `/home/user/.local/bin/hermes`
   - `/usr/local/bin/hermes`
   - `/path/to/hermes-agent/...`（开发模式）

### 3.3 误识别风险与防范

| 风险场景 | 防范措施 |
|---------|---------|
| 其他名为 hermes 的 Python 项目 | 检查 cmdline 中路径是否为已知安装路径模式（`/bin/hermes`、`hermes-agent`），而非仅仅匹配字符串 `hermes` |
| Python 进程恰好加载了 hermes 相关模块 | 只匹配 cmdline_args 中的 **可执行路径**（argv[0] 或 argv[1]），不匹配 import 路径 |
| 短暂运行的 `hermes setup`/`hermes model` 等子命令 | 可接受，这些也是 Hermes agent 的运行实例 |

## 4. 代码实现方案

### 4.1 新增文件

**`src/agentsight/src/discovery/agents/hermes.rs`**

```rust
//! Hermes agent matcher
//!
//! Hermes Agent (by Nous Research) is a self-improving AI agent that runs via Python.
//! This matcher identifies it by checking if the process is python3 with
//! "hermes" in its command line arguments.

use crate::discovery::agent::AgentInfo;
use crate::discovery::matcher::{AgentMatcher, ProcessContext, match_name_with_version_suffix};

/// Custom matcher for Hermes Agent
///
/// Matches by: comm is "python3" (or python3.XX) and cmdline contains "hermes"
pub struct HermesMatcher {
    info: AgentInfo,
}

impl HermesMatcher {
    pub fn new() -> Self {
        Self {
            info: AgentInfo::new(
                "Hermes",
                vec!["python3"],
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

        // Match: python3 runtime with "hermes" in cmdline args
        let is_python3 = match_name_with_version_suffix(&comm_lower, "python3");
        if !is_python3 {
            return false;
        }

        // Check cmdline args for hermes-related paths
        // Focus on argv[0] or argv[1] which typically contain the entry point path
        ctx.cmdline_args.iter().take(2).any(|arg| {
            let arg_lower = arg.to_lowercase();
            // Match common installation paths
            arg_lower.contains("/hermes") || arg_lower.contains("hermes-agent")
        })
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
use super::agents::cosh::CoshMatcher;
use super::agents::hermes::HermesMatcher;    // 新增
use super::agents::openclaw::OpenClawMatcher;
use super::matcher::AgentMatcher;

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

在 `hermes.rs` 中添加单元测试，覆盖以下场景：

| 测试用例 | comm | cmdline_args | 预期结果 |
|---------|------|-------------|---------|
| CLI 模式匹配 | `python3` | `["/usr/bin/python3", "/home/user/.local/bin/hermes"]` | ✅ 匹配 |
| 带版本号匹配 | `python3.11` | `["/usr/bin/python3.11", "/home/user/.local/bin/hermes"]` | ✅ 匹配 |
| Gateway 模式匹配 | `python3` | `["python3", "/usr/local/bin/hermes", "gateway"]` | ✅ 匹配 |
| 开发模式匹配 | `python3` | `["python3", "/home/user/hermes-agent/scripts/run.py"]` | ✅ 匹配 |
| 非 python3 进程不匹配 | `node` | `["node", "/usr/local/bin/hermes"]` | ❌ 不匹配 |
| cmdline 无 hermes 不匹配 | `python3` | `["python3", "manage.py", "runserver"]` | ❌ 不匹配 |
| hermes 在 argv[2+] 不匹配 | `python3` | `["python3", "script.py", "--hermes"]` | ❌ 不匹配 |

### 5.2 集成测试

在已有的 e2e 测试框架中新增 Hermes agent 的发现测试：

1. 启动 Hermes 进程（`hermes` 或 `hermes gateway`）
2. 执行 `AgentScanner::scan()`
3. 验证返回结果包含 Hermes agent
4. 停止 Hermes 进程
5. 验证 `on_process_exit()` 正确清理

## 6. 与现有 Matcher 的对比

| 特性 | CoshMatcher | OpenClawMatcher | HermesMatcher |
|------|------------|-----------------|---------------|
| 运行时 | Node.js | Node.js / 直部二进制 | Python3 |
| comm 匹配 | `node` | `openclaw-gatewa` / `node` | `python3` |
| cmdline 匹配 | `/usr/bin/co` 等固定路径 | `openclaw` + `gateway` | 包含 `hermes` 的路径 |
| 匹配范围 | argv 全量 | argv 全量 | argv 前 2 个元素 |
| 多模式支持 | 否 | 是（直部 + node） | 否（仅 python3） |

## 7. 后续优化方向

1. **精确路径匹配**：随着 Hermes 安装方式的多样化，可增加对更多已知安装路径的识别（如 Docker 容器内路径）
2. **子命令识别**：区分 CLI 模式和 Gateway 模式，可在 `DiscoveredAgent` 中增加 mode 字段
3. **环境变量检测**：Hermes 可能设置特定环境变量（如 `HERMES_HOME`），可作为辅助识别手段
4. **配置文件检测**：检查 `~/.hermes/` 目录是否存在，作为辅助判断
