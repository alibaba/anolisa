# ANOLISA — An Agentic OS Implementation

[中文版](README_CN.md)

ANOLISA, the Agentic evolution of Anolis OS, aims to deliver the
best-practice implementation of Agentic OS — a server-side operating
system built for AI Agent workloads.

> **A**gentic **N**exus **O**perating **L**ayer & **I**nterface **S**ystem **A**rchitecture

## Components

| Component | Description |
|-----------|-------------|
| [Copilot Shell](src/copilot-shell/) | AI-powered terminal assistant for code understanding, task automation, and system management. Built on [Qwen Code](https://github.com/QwenLM/qwen-code). |
| [Agent Sec Core](src/agent-sec-core/) | OS-level security kernel — system hardening, sandboxing, asset integrity verification, and security decision-making. |
| [AgentSight](src/agentsight/) | eBPF-based observability for AI Agents — zero-intrusion monitoring of LLM API calls, token consumption, and process behavior. |
| [Token-less](src/tokenless/) | LLM token optimization toolkit — schema/response compression and command rewriting to reduce token consumption. |
| [OS Skills](src/os-skills/) | Curated skill library for system administration, monitoring, security, DevOps, and cloud integration. |

See each component's README for detailed documentation.

## Getting Started

```bash
# Install all components via RPM
sudo yum install copilot-shell agent-sec-core agentsight tokenless os-skills

# Launch Copilot Shell
cosh
```

## License

Apache License 2.0 — see [LICENSE](LICENSE).
