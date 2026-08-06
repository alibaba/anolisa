# cosh-ng 快速开始

[English](../../../en/user-entrypoint/cosh-ng/QUICKSTART.md)

cosh-ng 在普通 bash 或 zsh 会话中加入 Agent。启动 `cosh` 后可以照常运行 Shell 命令，也可以在需要时用自然语言描述更复杂的任务。

## 1. 安装

安装 ANOLISA CLI 和 cosh-ng：

```bash
curl -fsSL https://get.agentic-os.sh | bash
sudo anolisa --install-mode system install cosh-ng
```

Alibaba Cloud Linux 用户也可以改用 RPM：

```bash
sudo yum install cosh-ng
```

验证两个用户命令：

```bash
cosh --version
cosh-cli --version
```

修改软件包和服务通常需要 root 权限；工作区快照命令还需要运行中的 `ws-ckpt` 守护进程。

以上安装方式面向 Linux。源码构建仅供贡献者使用；完成上述安装方式后，请参阅[开发者入门指南](../../../../developer-guide/zh/cosh-ng/getting-started.md)。

## 2. 启动终端

在 Agent 要处理的项目或系统目录中启动 `cosh`：

```bash
cd your-project
cosh
```

在同一个会话中运行命令，也可以把更复杂的任务作为普通输入交给 Agent：

```text
$ git status
```

例如，可以要求 Agent 分析上次部署失败的原因，先检查而不做任何修改。

操作需要同意时，cosh 会先显示审批卡片或问题卡片。

常用的起始命令：

```text
/auth
/help
/status
/mode approval recommend
/session list
```

`/auth` 用于选择或更新模型提供商认证，`/help` 查看斜杠命令，`/status` 查看运行时和会话状态，`/mode approval recommend` 在每次 Agent 工具调用前请求确认，`/session list` 列出当前工作区可恢复的会话。

使用 `/session list --all` 可同时列出其他工作区创建的会话；恢复会话时，请先进入创建它的工作区。

## 3. 复用技能

列出并查看当前工作区可用的 Skills：

```text
/skills list
/skills detail service-health
```

工作区、用户、扩展和系统 Skill 目录会按优先级合并。搜索顺序和文件格式见 [Skills](core/skills.md)。

## 4. 继续完成任务

| 目标 | 继续阅读 |
|---|---|
| 控制审批和安全策略 | [工具审批](shell/approval.md) |
| 恢复或压缩会话 | [会话恢复](shell/session-recovery.md) |
| 选择模型并完成认证 | [模型提供商](core/providers.md) |
| 接入其他服务提供的工具 | [接入 MCP 服务](mcp.md) |
| 自动处理软件包、服务、快照或审计工作 | [结构化 OS CLI](cli/overview.md) |
| 集成其他前端 | [无界面模式](core/headless-mode.md) |

[完整用户手册](README.md)按任务整理了其余内容。
