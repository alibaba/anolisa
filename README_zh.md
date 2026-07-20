<div align="center">

<img src="docs/images/readme/agentic-os.png" alt="ANOLISA Agentic OS" width="360"/>

# ANOLISA

**面向 Agent 工作负载的操作系统层。**

[English](README.md) · [项目网站](https://agentic-os.sh/) ·
[快速开始](docs/QUICKSTART_zh.md) ·
[用户指南](docs/user-guide/zh/README.md) ·
[参与贡献](CONTRIBUTING_zh.md)

[![许可证](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![平台](https://img.shields.io/badge/Platform-Linux%20%7C%20macOS%20%7C%20Windows-lightgrey.svg)](docs/user-guide/zh/installation.md)

</div>

ANOLISA 是面向 AI Agent 工作负载的服务端操作系统层。它从终端入口、Token
开销和执行环境三个方向解决 Agent 运行中的关键问题，同时保留现有的 Shell、
Agent 框架和沙箱。ANOLISA CLI 提供统一的安装入口，各项能力可以按需启用。

## 解决什么问题

<p align="center"><strong>01 · AGENT INTERFACE</strong></p>

<h3 align="center">让 Agent 直接在终端工作</h3>

cosh-ng 为 Agent 提供结构化、可预期的 Shell 和系统操作接口。保留现有的 Agent
框架和沙箱，让系统操作继续在熟悉的终端工作流中完成。

[开始使用 cosh-ng →](docs/user-guide/zh/user-entrypoint/cosh-ng/QUICKSTART.md)

<p align="center"><strong>02 · CONTEXT EFFICIENCY</strong></p>

<h3 align="center">看清 Token 去向，在内容进入模型前减少无效消耗</h3>

Token-less 去掉工具 Schema、响应和命令输出中的冗余，Agent Memory 复用跨会话
信息，AgentSight 记录 Token 实际花在哪。

| 工具响应 | 工具 Schema | 命令输出 | 整体压缩 |
|----------|-------------|----------|----------|
| **Token 减少 65.8%** | **Token 减少 47.3%** | **输出减少 58.6–98.6%** | **Token 减少 62.9%** |
| ResponseCompressor · 46.85 µs | SchemaCompressor · 11.44 µs | RTK · 3 条命令实测 | 198.91 µs |

[开始使用 Token-less →](docs/user-guide/zh/token-saving/tokenless/QUICKSTART.md)

<p align="center"><strong>03 · EXECUTION RUNTIME</strong></p>

<h3 align="center">让 Agent 的每次执行都有边界，也留有退路</h3>

ANOLISA 正在完善面向 Agent 的执行环境：[Blaze](src/blaze/README_zh.md) 负责
沙箱编排，[Agent Sec Core](src/agent-sec-core/README_zh.md) 隔离高风险操作，
[ws-ckpt](src/ws-ckpt/README_zh.md) 为工作区变更保留恢复点，
[SkillFS](src/skillfs/README_zh.md) 按需挂载 Skills。

[通过 ANOLISA CLI 开始 →](docs/user-guide/zh/user-entrypoint/anolisa-cli.md)

## 安装

ANOLISA CLI 是统一的安装入口，cosh-ng、Token-less 和其他能力都可以按需启用。

```bash
curl -fsSL https://agentic-os.sh/install.sh | bash

anolisa install cosh-ng
anolisa install tokenless
```

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
