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

**The operating system layer for Agent workloads.**

Let Agents drive the system straight from your terminal, and strip the tool
responses that reach the model before they cost you — while keeping the Shell,
Agent framework, and sandbox you already run.

[中文版](README_zh.md) · [Website](https://agentic-os.sh/) ·
[Quick Start](docs/QUICKSTART.md) ·
[User Guide](docs/user-guide/en/README.md) ·
[Contributing](CONTRIBUTING.md)

[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Linux%20%7C%20macOS-lightgrey.svg)](docs/user-guide/en/installation.md)

</div>

---

ANOLISA is a server-side operating layer for AI Agent workloads. It addresses
three practical constraints of Agent execution: terminal entry, Token cost, and
execution environments. Keep the Shell, Agent framework, and sandbox you
already use. ANOLISA CLI provides a single installation entry point, while each
capability can be enabled independently.

<p align="center">
  <img
    src="docs/images/readme/highlights.png"
    alt="ANOLISA product highlights"
  />
</p>

## What it solves

<p align="center"><strong>01 · AGENT INTERFACE</strong></p>

<h3 align="center">Let the Agent work directly in the terminal</h3>

cosh-ng is an AI-native Linux terminal: it keeps familiar Bash/Zsh behavior,
then adds an Agent that can understand intent, use tools and Skills, and ask
for approval before risky work. Shell commands and natural language share one
terminal instead of forcing users into a separate chat application.

[Get started with cosh-ng →](docs/user-guide/en/user-entrypoint/cosh-ng/QUICKSTART.md)

<p align="center"><strong>02 · CONTEXT EFFICIENCY</strong></p>

<h3 align="center">See where Tokens go and cut waste before it reaches the model</h3>

Token-less removes redundancy from tool schemas and responses.
[Agent Memory](src/agent-memory/README.md) reuses context across sessions,
[SkillFS](src/skillfs/README.md) exposes Skills as views and mounts them on
demand so only the relevant ones enter the context, and
[AgentSight](src/agentsight/README.md) records where Tokens are actually spent.

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
    In one observed coding task, Token-less saved 317K Tokens (40.5%), based
    on AgentSight measurements.
    Results vary by workload.
  </sub>
</p>

<p align="center">
  <img
    src="docs/images/readme/tokenless-response.png"
    alt="Token-less response compression in the terminal"
  />
</p>

`debug` and `trace` are dropped by the field blacklist, `metadata` as null, and
`tags` / `extra` as empty values. Compression runs between the Agent and the
model, so no Agent framework code changes. Dropped array items stay retrievable
through a `<<tokenless:KEY>>` marker, which keeps the compression reversible.

| Tool responses | Tool schemas | Full pipeline |
|----------------|--------------|---------------|
| **65.8% fewer Tokens** | **47.3% fewer Tokens** | **62.9% fewer Tokens** |
| ResponseCompressor · 46.85 µs | SchemaCompressor · 11.44 µs | 198.91 µs |

Savings apply to the tool responses entering the context, not to the whole
session bill — the [Token-less README](src/tokenless/README.md) explains how to
estimate the effect for a given workload.

[Read the Token-less user manual →](docs/user-guide/en/token-saving/tokenless/user-manual.md)

<p align="center"><strong>03 · EXECUTION RUNTIME</strong></p>

<h3 align="center">Give every Agent execution a boundary and a way back</h3>

ANOLISA is building out the Agent execution environment:
[Agent Sec Core](src/agent-sec-core/README.md) isolates risky operations, and
[ws-ckpt](src/ws-ckpt/README.md) keeps recovery points for workspace changes.

[Start with ANOLISA CLI →](docs/user-guide/en/user-entrypoint/anolisa-cli.md)

## Install

ANOLISA CLI is the common installation entry point. cosh-ng is installed in
system mode; Token-less and other capabilities can be added independently.

```bash
curl -fsSL https://get.agentic-os.sh | bash

sudo anolisa --install-mode system install cosh-ng
anolisa install tokenless
```

Run `cosh` to enter the AI-native terminal. Token-less can also optimize tool
calls from an existing Agent without changing its framework.

[Read the Quick Start →](docs/QUICKSTART.md)

## Documentation

[Quick Start](docs/QUICKSTART.md) ·
[Installation](docs/user-guide/en/installation.md) ·
[User Guide](docs/user-guide/en/README.md) ·
[Troubleshooting](docs/user-guide/en/troubleshooting.md) ·
[Build from Source](docs/BUILDING.md) ·
[Changelog](CHANGELOG.md)

## Community

<div align="center">

<img src="docs/images/readme/dingtalk-qr.png" alt="ANOLISA DingTalk community QR code" width="180"/>

Scan with DingTalk to join the ANOLISA community.

</div>

- [Open an issue](https://github.com/alibaba/anolisa/issues) for bugs and
  feature requests.
- Read [CONTRIBUTING.md](CONTRIBUTING.md) before submitting a pull request.
- Report vulnerabilities through the [Security Policy](SECURITY.md).

## License

ANOLISA is released under the [Apache License 2.0](LICENSE).
