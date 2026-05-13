# ANOLISA, An Agentic OS Implementation

![License](https://img.shields.io/github/license/alibaba/anolisa)
![Last Commit](https://img.shields.io/github/last-commit/alibaba/anolisa)
![Repo Stars](https://img.shields.io/github/stars/alibaba/anolisa?style=social)

[中文版](README_CN.md)

ANOLISA, the Agentic evolution of Anolis OS, is a server-side operating system stack built for AI agent workloads. It combines terminal tooling, security controls, observability, token-efficiency tooling, and reusable operating skills into a single Agentic OS implementation.

> **A**gentic **N**exus **O**perating **L**ayer & **I**nterface **S**ystem **A**rchitecture

## Table of Contents
- [Why ANOLISA](#why-anolisa)
- [Components](#components)
- [Quick Start](#quick-start)
- [Build from Source](#build-from-source)
- [Repository Layout](#repository-layout)
- [Documentation](#documentation)
- [License](#license)

## Why ANOLISA
ANOLISA is aimed at teams building or operating AI agents on servers. It packages several core capabilities that are usually spread across separate tools:

- AI-native shell workflows for coding and task execution
- sandboxing and security enforcement for agent actions
- observability for agent processes and LLM usage
- token optimization helpers for lower-cost inference
- reusable OS-oriented skills for operations workflows

## Components

| Component | Description |
|-----------|-------------|
| [Copilot Shell](src/copilot-shell/) | AI-powered terminal assistant for code understanding, task automation, and system management. Built on [Qwen Code](https://github.com/QwenLM/qwen-code). |
| [Agent Sec Core](src/agent-sec-core/) | OS-level security kernel for system hardening, sandboxing, asset integrity verification, and security decision-making. |
| [AgentSight](src/agentsight/) | eBPF-based observability for AI agents, including process behavior and LLM API monitoring. |
| [Token-less](src/tokenless/) | Token optimization toolkit for schema compression, response compression, and command rewriting. |
| [OS Skills](src/os-skills/) | Curated skill library for system administration, monitoring, security, DevOps, and cloud integration. |

## Quick Start
### Install packaged components

```bash
sudo yum install copilot-shell agent-sec-core agentsight tokenless os-skills
cosh
```

### Enable sandbox hooks after installation
Run this once inside Copilot Shell to activate the bundled sandbox policy hooks:

```text
/hooks install
```

## Build from Source
For a full source build:

```bash
git clone https://github.com/alibaba/anolisa.git
cd anolisa
./scripts/build-all.sh
```

Useful variants:

```bash
./scripts/build-all.sh --no-install
./scripts/build-all.sh --ignore-deps
./scripts/build-all.sh --component cosh --component sec-core
./scripts/build-all.sh --component cosh --component skills --component sec-core --component sight
```

For detailed dependency and platform notes, see [docs/BUILDING.md](docs/BUILDING.md).

## Repository Layout

```text
anolisa/
├── src/
│   ├── copilot-shell/
│   ├── os-skills/
│   ├── agent-sec-core/
│   └── agentsight/
├── scripts/
│   ├── build-all.sh
│   └── rpm-build.sh
├── tests/
│   └── run-all-tests.sh
├── docs/
└── Makefile
```

## Documentation
- [Build from source](docs/BUILDING.md)
- [Chinese README](README_CN.md)
- component-specific README files under `src/`

## License
Apache License 2.0, see [LICENSE](LICENSE).
