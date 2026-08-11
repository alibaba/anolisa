<div align="center">

<picture>
  <source
    media="(prefers-color-scheme: dark)"
    srcset="docs/images/brand/anolisa-lockup-dark.svg"
  >
  <source
    media="(prefers-color-scheme: light)"
    srcset="docs/images/brand/anolisa-lockup-light.svg"
  >
  <img
    src="docs/images/brand/anolisa-lockup-light.svg"
    alt="ANOLISA"
    width="320"
  >
</picture>

<sub>**A**gentic **N**exus **O**perating **L**ayer & **I**nterface **S**ystem **A**rchitecture</sub>

**面向 Agent 工作负载的操作系统层。**

让 Agent 在你的终端里直接指挥系统干活，并在工具响应进入模型之前去掉冗余，
同时保留你现有的 Shell、Agent 框架和沙箱。

[English](README.md) · [项目网站](https://agentic-os.sh/) ·
[快速开始](docs/QUICKSTART_zh.md) ·
[用户指南](docs/user-guide/zh/README.md) ·
[参与贡献](CONTRIBUTING_zh.md)

[![许可证](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![平台](https://img.shields.io/badge/Platform-Linux%20%7C%20macOS-lightgrey.svg)](docs/user-guide/zh/installation.md)

</div>

---

ANOLISA 是面向 AI Agent 工作负载的服务端操作系统层。它从终端入口、Token
开销和执行环境三个方向解决 Agent 运行中的关键问题，同时保留现有的 Shell、
Agent 框架和沙箱。ANOLISA CLI 提供统一的安装入口，各项能力可以按需启用。

<p align="center">
  <img
    src="docs/images/readme/highlights-zh.png"
    alt="终端原生、工具响应 Token 减少 62.9%、一行安装"
  />
</p>

## 解决什么问题

<p align="center"><strong>01 · AGENT INTERFACE</strong></p>

<h3 align="center">让 Agent 直接在终端工作</h3>

cosh-ng 是面向 AI 时代重构的 Linux 终端。它保留熟悉的 Bash/Zsh 行为，同时加入
能理解意图、调用工具与 Skills、并在高风险操作前请求确认的 Agent。Shell 命令和
自然语言共用一个终端，不必切换到独立的聊天应用。

[开始使用 cosh-ng →](docs/user-guide/zh/user-entrypoint/cosh-ng/QUICKSTART.md)

<p align="center"><strong>02 · CONTEXT EFFICIENCY</strong></p>

<h3 align="center">看清 Token 去向，在内容进入模型前减少无效消耗</h3>

Token-less 去掉工具 Schema 和响应中的冗余，[Agent Memory](src/agent-memory/README_zh.md)
复用跨会话信息，[SkillFS](src/skillfs/README_zh.md) 按视图暴露、按需挂载
Skills，只把用得到的技能放进上下文，[AgentSight](src/agentsight/README_zh.md)
记录 Token 实际花在哪。

### 使用 Claude Code，3 分钟体验 Token-less

安装 Token-less，并将它接入 Claude Code：

```bash
curl -fsSL https://get.agentic-os.sh | bash
export PATH="$HOME/.local/bin:$PATH"
anolisa install tokenless
anolisa adapter enable tokenless claude-code
```

重启 Claude Code，运行一次工具密集型任务，再查看结果：

```bash
tokenless stats summary
tokenless stats list --limit 5
```

[打开完整 Token-less 快速体验 →](https://agentic-os.sh/zh/docs/user-guide/token-saving/tokenless/quickstart/)
· [阅读完整用户手册](https://agentic-os.sh/zh/docs/user-guide/token-saving/tokenless/user-manual/)

<table align="center" cellpadding="0" cellspacing="0">
  <tr>
    <td>
      <video
        controls
        muted
        src="https://github.com/user-attachments/assets/b372ae72-44fa-492f-9feb-e6cd137b631a"
      ></video>
    </td>
  </tr>
</table>

<p align="center">
  <sub>
    在一次编码任务的单次观测中，Token-less 节省了 317K Tokens（40.5%，
    基于 AgentSight 观测）。
    实际效果因工作负载而异。
  </sub>
</p>

<p align="center">
  <img
    src="docs/images/readme/tokenless-response.png"
    alt="终端中的 Token-less 响应压缩"
  />
</p>

`debug`、`trace` 命中字段黑名单，`metadata` 为 null，`tags` / `extra` 为空值，
均被移除。压缩在 Agent 与模型之间执行，无需改动 Agent 框架代码；被截断的数组
元素可通过 `<<tokenless:KEY>>` 标记取回，压缩过程可逆。

| 工具响应 | 工具 Schema | 整体压缩 |
|----------|-------------|----------|
| **Token 减少 65.8%** | **Token 减少 47.3%** | **Token 减少 62.9%** |
| ResponseCompressor · 46.85 µs | SchemaCompressor · 11.44 µs | 198.91 µs |

节省比例针对进入上下文的工具响应，不代表整个会话的账单。具体工作负载的估算方法
见 [Token-less README](src/tokenless/README_zh.md)。

<p align="center"><strong>03 · EXECUTION RUNTIME</strong></p>

<h3 align="center">让 Agent 的每次执行都有边界，也留有退路</h3>

ANOLISA 正在完善面向 Agent 的执行环境。
[Agent Sec Core](src/agent-sec-core/README_zh.md) 隔离高风险操作，
[ws-ckpt](src/ws-ckpt/README_zh.md) 为工作区变更保留恢复点。

[通过 ANOLISA CLI 开始 →](docs/user-guide/zh/user-entrypoint/anolisa-cli.md)

## 安装

ANOLISA CLI 是统一的安装入口。cosh-ng 使用 system mode 安装；Token-less 和
其他能力可独立按需添加。

```bash
curl -fsSL https://get.agentic-os.sh | bash

sudo anolisa --install-mode system install cosh-ng
anolisa install tokenless
```

运行 `cosh` 进入 AI 原生终端。Token-less 也可直接优化现有 Agent 的工具调用，
无需更换 Agent 框架。

[查看快速开始 →](docs/QUICKSTART_zh.md)

## 文档

[快速开始](docs/QUICKSTART_zh.md) ·
[安装指南](docs/user-guide/zh/installation.md) ·
[用户指南](docs/user-guide/zh/README.md) ·
[故障排查](docs/user-guide/zh/troubleshooting.md) ·
[源码构建](docs/BUILDING_zh.md) ·
[变更日志](CHANGELOG_zh.md)

## 社区

<div align="center">

<img src="docs/images/readme/dingtalk-qr.png" alt="ANOLISA 钉钉社区二维码" width="180"/>

使用钉钉扫码加入 ANOLISA 社区。

</div>

- 遇到问题或有新的 Agent 场景，欢迎[提交 Issue](https://github.com/alibaba/anolisa/issues)。
- 提交 Pull Request 前，请先阅读[贡献指南](CONTRIBUTING_zh.md)。
- 安全问题请通过[安全策略](SECURITY.md)中的渠道报告。

## 许可证

ANOLISA 基于 [Apache License 2.0](LICENSE) 发布。
