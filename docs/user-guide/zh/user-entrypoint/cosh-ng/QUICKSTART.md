# cosh-ng 快速开始

[English](../../../en/user-entrypoint/cosh-ng/QUICKSTART.md)

cosh-ng 在普通 bash 或 zsh 会话中加入 Agent。运行 `cosh` 后，既可以照常执行命令，
也可以直接用自然语言交代更复杂的任务，全程无需离开终端。

## 1. 安装

安装 ANOLISA CLI 和 cosh-ng。

```bash
curl -fsSL https://get.agentic-os.sh | bash
sudo anolisa --install-mode system install cosh-ng
```

Alibaba Cloud Linux 用户也可以直接安装 RPM。

```bash
sudo yum install cosh-ng
```

以上安装方式目前面向 Linux。macOS 请按照
[开发者入门指南](../../../../developer-guide/zh/cosh-ng/getting-started.md)从源码构建。

验证安装。

```bash
cosh --version
cosh-cli --version
```

修改软件包和服务需要 root 权限；工作空间 checkpoint 命令还需要运行中的
`ws-ckpt` daemon。

## 2. 进入 AI 终端

在希望 Agent 工作的项目或系统目录中启动 cosh。

```bash
cd your-project
cosh
```

你可以像以前一样运行 Shell 命令，也可以直接用自然语言描述任务。

```text
$ git status
$ 找出上次部署失败的原因；先检查，不要做任何修改
```

Agent 会在终端中持续输出处理过程。操作需要你同意时，cosh 会显示审批卡片或
问题卡片，等待你选择。

刚开始时，这几条命令最常用。

```text
/auth                         选择或更新 provider 认证
/help                         查看可用 slash 命令
/status                       查看当前运行时和会话
/mode approval recommend      每次 Agent 工具调用都请求确认
/session list                 列出当前工作空间可恢复的对话
```

用 `/session list --all` 可以找到其他工作空间里创建的对话。需要恢复时，先进入创建该
对话的工作空间，再启动 cosh。

## 3. 复用工作

查看当前工作空间可用的 Skills。

```text
/skills list
/skills detail service-health
```

工作空间、用户、扩展和系统中的 Skill 会按优先级合并。搜索顺序和文件格式见
[Skills](core/skills.md)。

## 4. 接着完成手里的任务

| 你要完成的事 | 继续阅读 |
|---|---|
| 控制审批和安全策略 | [工具审批](shell/approval.md) |
| 恢复或压缩对话 | [会话恢复](shell/session-recovery.md) |
| 选择模型并完成认证 | [模型 Provider](core/providers.md) |
| 接入其他服务提供的工具 | [接入 MCP server](mcp.md) |
| 自动处理软件包、服务、快照或审计工作 | [管理系统操作](cli/overview.md) |
| 集成其他前端 | [Headless 模式](core/headless-mode.md) |

[完整用户手册](README.md)按用户任务整理了其余页面。
