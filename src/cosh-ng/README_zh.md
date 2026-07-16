# cosh-ng

[English](README.md)

## 什么是 cosh

**Computable Operating System Harness** — 面向 Agent 的确定性操作系统接口，为 AI Agent 提供跨发行版的结构化系统操作能力。

## 架构

5-crate 工作空间，严格依赖方向：

```
cosh-types          cosh-platform          cosh-cli / cosh-core / cosh-shell
  (纯类型)        ← (发行版检测 +        ← (CLI 入口、交互式 TUI、
   零副作用)        后端路由)               AI 增强 Shell)

依赖方向: cosh-cli / cosh-core / cosh-shell → cosh-platform → cosh-types
```

### Crate 布局

```
cosh-ng/
├── crates/
│   ├── cosh-types/       # 纯类型定义，零副作用
│   │   └── src/          # checkpoint.rs, config.rs, error.rs, output.rs, pkg.rs, svc.rs
│   ├── cosh-platform/    # 发行版检测 + 后端路由
│   │   └── src/          # checkpoint.rs, detect.rs, pkg.rs, svc.rs
│   ├── cosh-cli/         # CLI 入口 (二进制: cosh-cli)
│   │   ├── src/          # main.rs, cmd/{pkg,svc,checkpoint,audit}.rs
│   │   └── tests/        # CLI 集成测试
│   ├── cosh-core/        # 交互式 TUI + 无头 JSONL 后端 (二进制: cosh-core)
│   │   └── src/          # LLM 对话、工具执行、Hook 系统、会话管理
│   └── cosh-shell/       # AI 增强交互式 Shell (二进制: cosh-shell)
│       ├── src/          # PTY 宿主、OSC 标记、审批控制、流式 AI
│       └── tests/        # 协议 + 集成测试
└── Cargo.toml
```

## 快速开始

```bash
# 构建
cargo build --workspace

# 结构化 JSON 输出
cosh-cli pkg install nginx
# → {"ok":true,"data":{"package":"nginx","version":"1.24.0","already_installed":false},...}

cosh-cli pkg install nginx --dry-run   # 预览不执行

# 服务管理 (systemd)
cosh-cli svc status nginx
# → {"ok":true,"data":{"name":"nginx","active":true,"enabled":true,"recent_logs":[...]},...}

cosh-cli svc restart nginx --dry-run

# 工作空间快照（需要 ws-ckpt 守护进程）
cosh-cli checkpoint create --workspace /home/agent/project --id step-042 -m "重构前"
# → {"ok":true,"data":{"checkpoint_id":"step-042","step":42},...}

cosh-cli checkpoint restore step-040 --workspace /home/agent/project

# 安全审计
cosh-cli audit check --action "rm -rf /var/log"
# → {"ok":true,"data":{"outcome":"Deny","matched_rule":"shell-deny-destructive",...},...}

# 在当前工作空间恢复 Agent 对话
cosh-shell --resume              # 打开交互式会话选择器
cosh-shell --resume <session-id> # 选择已知的 provider 会话
```

在 cosh-shell 中，可使用 `/session` 浏览会话、`/session status`
查看已选择和已激活的身份，并通过 `/session clear ...` 在确认后清理旧记录。
会话恢复会还原模型可见的对话上下文，但不会伪装成已恢复历史终端证据。记录
默认保存在 `~/.copilot-shell/cosh-core/sessions/`，可通过
`session.persist_dir` 修改根目录。项目会话配置和相对存储路径均从
cosh-shell 传给 Core 的工作空间解析。详见
[会话恢复指南](../../docs/user-guide/zh/user-entrypoint/cosh-ng/shell/session-recovery.md)。

## 命令参考

| 子命令 | 示例 | 后端 |
|--------|------|------|
| `cosh-cli pkg install <name>` | `cosh-cli pkg install nginx` | dnf / apt-get / zypper |
| `cosh-cli pkg remove <name>` | `cosh-cli pkg remove nginx` | dnf / apt-get / zypper |
| `cosh-cli pkg search <query>` | `cosh-cli pkg search "web server"` | dnf / apt-cache / zypper |
| `cosh-cli svc status <name>` | `cosh-cli svc status nginx` | systemctl show |
| `cosh-cli svc start/stop/restart` | `cosh-cli svc restart nginx` | systemctl |
| `cosh-cli svc enable/disable` | `cosh-cli svc enable nginx` | systemctl |
| `cosh-cli svc list` | `cosh-cli svc list --state running` | systemctl list-units |
| `cosh-cli checkpoint create` | `cosh-cli checkpoint create --workspace /path --id snap-001 -m "msg"` | ws-ckpt daemon |
| `cosh-cli checkpoint list` | `cosh-cli checkpoint list --workspace /path` | ws-ckpt daemon |
| `cosh-cli checkpoint restore <id>` | `cosh-cli checkpoint restore step-003 --workspace /path` | ws-ckpt daemon |
| `cosh-cli checkpoint status` | `cosh-cli checkpoint status` | ws-ckpt daemon |
| `cosh-cli checkpoint init` | `cosh-cli checkpoint init --workspace /path` | ws-ckpt daemon |
| `cosh-cli checkpoint delete` | `cosh-cli checkpoint delete --snapshot snap-001` | ws-ckpt daemon |
| `cosh-cli checkpoint diff` | `cosh-cli checkpoint diff --workspace /path --from a --to b` | ws-ckpt daemon |
| `cosh-cli audit check` | `cosh-cli audit check --action "rm -rf /"` | 策略引擎 |
| `cosh-cli audit log` | `cosh-cli audit log --session abc123` | 策略引擎 |
| `cosh-cli audit policy show` | `cosh-cli audit policy show` | 策略引擎 |

## 输出格式

所有命令输出统一 JSON 信封（`CoshResponse<T>`）：

```json
{"ok":true,"data":{...},"meta":{"subsystem":"pkg","duration_ms":342,"distro":"alinux","dry_run":false}}
```

错误时：

```json
{"ok":false,"error":{"code":"PkgNotFound","message":"package 'nginx-extra' not found","recoverable":true,"hint":"try 'cosh-cli pkg search nginx'","subsystem":"pkg"},"meta":{...}}
```

Agent 关键字段：`ok`（是否成功？）、`error.recoverable`（值得重试？）、`error.hint`（下一步建议）。

## Agent 价值

1. **零学习成本** — Agent 无需知道 dnf 还是 apt
2. **结构化输出** — JSON，无需正则文本解析
3. **可逆操作** — checkpoint → 执行 → 失败时回滚
4. **分类错误** — `recoverable` 告诉 Agent 是否该重试
5. **预演模式** — 所有写操作支持 `--dry-run`，执行前预览

## MCP 工具

`cosh-core --headless` 可以通过 stdio 连接受信任的本地 MCP Server，在启动时发现其工具，
并以 `mcp__<server>__<tool>` 暴露给 Agent。MCP Server 仅从用户或系统级配置加载；
除 `trust` 模式外，其工具调用都需要审批。详见 [MCP 配置说明](../../docs/user-guide/zh/user-entrypoint/cosh-ng/configuration.md#mcp-stdio-server)。

## 日志

所有二进制使用 `tracing` 结构化日志。日志写入 `~/.copilot-shell/logs/`，按天轮转。

### 日志级别控制

| 方式 | 示例 | 作用域 |
|------|------|--------|
| 配置文件 | `[ui] log_level = "debug"` (cosh-shell) | 持久化 |
| 配置文件 | `[logging] level = "info"` (cosh-core) | 持久化 |
| 环境变量 | `COSH_LOG=debug cosh-shell raw` | 单次调用 |
| CLI 标志 | `cosh-core --verbose` | 单次调用 |
| 旧版 | `COSH_SHELL_DEBUG=1`（映射为 debug） | 单次调用 |

优先级：`COSH_LOG` > `RUST_LOG` > `--verbose` > 配置文件 > 默认值（`warn`）

有效级别：`error`、`warn`、`info`、`debug`、`trace`

### 日志文件

```
~/.copilot-shell/logs/
├── cosh-shell.log.2026-06-26    # 按天轮转
├── cosh-core.log.2026-06-26
└── ...
```

## 支持的发行版

| 发行版 | 包管理器 | 服务管理器 |
|--------|----------|-----------|
| Alinux 2/3 | dnf | systemd |
| CentOS 7/8/9 | dnf | systemd |
| Fedora | dnf | systemd |
| Ubuntu | apt-get | systemd |
| Debian | apt-get | systemd |
| openSUSE | zypper | systemd |

## 构建和测试

```bash
cargo build --workspace
cargo test --workspace
cargo test --package cosh-cli --test cli_integration  # 仅集成测试
```

**前置条件**：Linux，Rust 1.74+，pkg/svc 命令需要 root/sudo，checkpoint 命令需要 ws-ckpt 守护进程。

## 许可证

Apache-2.0
