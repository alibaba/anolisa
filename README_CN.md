# ANOLISA — 一个 Agentic OS 实现

[English](README.md)

ANOLISA 是 Anolis OS 的 Agentic 演进，旨在提供 Agentic OS 的最佳实践实现——一个为 AI Agent 工作负载构建的服务端操作系统。

> **A**gentic **N**exus **O**perating **L**ayer & **I**nterface **S**ystem **A**rchitecture

## 组件

| 组件 | 说明 |
|------|------|
| [Copilot Shell](src/copilot-shell/) | AI 驱动的终端助手，支持代码理解、任务自动化和系统管理。基于 [Qwen Code](https://github.com/QwenLM/qwen-code) 构建。 |
| [Agent Sec Core](src/agent-sec-core/) | OS 级安全核心组件——系统加固、沙箱隔离、资产完整性校验与安全决策。 |
| [AgentSight](src/agentsight/) | 基于 eBPF 的 AI Agent 可观测工具——零侵入监控 LLM API 调用、Token 消耗与进程行为。 |
| [Token-less](src/tokenless/) | LLM Token 优化工具包——通过 Schema/响应压缩和命令重写节省 Token 消耗。 |
| [OS Skills](src/os-skills/) | 运维技能库，涵盖系统管理、监控、安全、DevOps 和云集成。 |

详细文档请参阅各组件的 README。

## 快速开始

```bash
# 通过 RPM 安装所有组件
sudo yum install copilot-shell agent-sec-core agentsight tokenless os-skills

# 启动 Copilot Shell
cosh
```

## 许可证

Apache License 2.0 — 详见 [LICENSE](LICENSE)。
