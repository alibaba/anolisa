# 快速开始

cosh-ng（Computable Operating System Harness）为 AI Agent 提供确定性的跨发行版系统操作接口。它由三个二进制组成：

- **cosh-cli** — 结构化 JSON CLI，覆盖包管理、服务管理、工作区快照和安全审计
- **cosh-core** — 无头 JSONL 后端，集成 LLM 提供商、钩子、工具和技能
- **cosh-shell** — AI 增强的交互式终端，提供 PTY 主机、流式分析和工具审批

## 前置条件

- Linux（Alinux / CentOS / Ubuntu / Debian / Fedora / openSUSE）或 macOS（有限功能）
- Rust 1.74+
- pkg/svc 命令需要 root 或 sudo 权限
- checkpoint 命令需要运行中的 ws-ckpt 守护进程

## 构建

```bash
cd src/cosh-ng
cargo build --workspace
```

构建产物位于 `target/debug/` 下：`cosh-cli`、`cosh-core`、`cosh-shell`。

发布构建：

```bash
cargo build --workspace --release
```

## 第一次运行

### cosh-cli：结构化系统操作

```bash
# 安装一个包（JSON 输出）
cosh-cli pkg install nginx
# → {"ok":true,"data":{"package":"nginx","version":"1.24.0","already_installed":false},...}

# 预览模式（不实际执行）
cosh-cli pkg install nginx --dry-run

# 查看服务状态
cosh-cli svc status nginx
# → {"ok":true,"data":{"name":"nginx","active":true,"enabled":true,"recent_logs":[...]},...}
```

### cosh-core：AI Agent 后端

```bash
# 单条提示执行
cosh-core --headless "帮我查看磁盘使用情况"

# 或通过管道进入 headless 模式
echo '{"type":"user","message":{"role":"user","content":"列出当前目录文件"}}' | cosh-core --headless
```

### cosh-shell：交互式终端

```bash
# 启动交互式 AI Shell
cosh-shell
```

## 配置

配置文件位于 `~/.copilot-shell/config.toml`。首次运行时自动创建默认配置。

详见 [配置文档](configuration.md)。

## 下一步

- [cosh-cli 总览](cli/overview.md) — 了解 CLI 子系统
- [cosh-core 总览](core/overview.md) — 了解无头模式与 LLM 集成
- [cosh-shell 总览](shell/overview.md) — 了解交互式终端
- [输出格式](output-format.md) — 理解 JSON 信封与错误码
