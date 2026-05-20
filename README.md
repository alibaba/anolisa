# ANOLISA — An Agentic OS Implementation

[![License](https://img.shields.io/github/license/alibaba/anolisa?style=flat-square)](https://github.com/alibaba/anolisa/blob/main/LICENSE)
[![Last commit](https://img.shields.io/github/last-commit/alibaba/anolisa?style=flat-square)](https://github.com/alibaba/anolisa/commits/main)
[![Open issues](https://img.shields.io/github/issues/alibaba/anolisa?style=flat-square)](https://github.com/alibaba/anolisa/issues)

[中文版](README_CN.md)

ANOLISA, the Agentic evolution of Anolis OS, aims to deliver a best-practice implementation of an Agentic OS, a server-side operating system designed for AI agent workloads.

> **A**gentic **N**exus **O**perating **L**ayer & **I**nterface **S**ystem **A**rchitecture

## Table of Contents

- [What ANOLISA is](#what-anolisa-is)
- [Core components](#core-components)
- [Why it matters](#why-it-matters)
- [Getting started](#getting-started)
- [Typical use cases](#typical-use-cases)
- [Repository layout](#repository-layout)
- [Documentation and contribution](#documentation-and-contribution)
- [License](#license)

## What ANOLISA is

ANOLISA packages several agent-oriented system capabilities into one operating-system-level stack. The project focuses on three things:

- providing AI agents with a safer execution environment
- improving observability for agent runtime behavior
- shipping reusable system-side capabilities such as shell interaction, skill execution, and token optimization

## Core components

| Component | What it does |
|-----------|---------------|
| [Copilot Shell](src/copilot-shell/) | AI-powered terminal assistant for code understanding, task automation, and system management, built on Qwen Code. |
| [Agent Sec Core](src/agent-sec-core/) | Security kernel for hardening, sandboxing, asset integrity verification, and security decisions. |
| [AgentSight](src/agentsight/) | eBPF-based observability for LLM API calls, token consumption, and process behavior. |
| [Token-less](src/tokenless/) | Token optimization toolkit for schema compression, response compression, and command rewriting. |
| [OS Skills](src/os-skills/) | Curated skill library for administration, monitoring, security, DevOps, and cloud integration. |

## Why it matters

Agent systems need more than model calls. They also need controllable execution, security boundaries, visibility into runtime behavior, and lower token overhead. ANOLISA brings those concerns together at the OS layer instead of treating them as afterthoughts.

## Getting started

### Install from RPM packages

```bash
sudo yum install copilot-shell agent-sec-core agentsight tokenless os-skills
```

### Launch Copilot Shell

```bash
cosh
```

### Next steps

- explore each component README under `src/`
- review repository docs and changelog
- open issues or discussions for integration questions

## Typical use cases

- hardening infrastructure that runs AI agents continuously
- observing agent behavior across processes and model calls
- reducing token overhead in system prompts and command payloads
- building server-side environments with reusable agent skills

## Repository layout

- `src/`: component source directories
- `docker/`: container-related assets
- `scripts/`: utility scripts for development and packaging
- `docs/`: project documentation
- `tests/`: automated tests

## Documentation and contribution

Useful starting points:

- [CHANGELOG.md](CHANGELOG.md)
- [CONTRIBUTING.md](CONTRIBUTING.md)
- [SECURITY.md](SECURITY.md)
- `README_CN.md` for the Chinese version

## License

Apache License 2.0. See [LICENSE](LICENSE).
