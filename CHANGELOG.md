# Changelog

[中文版](CHANGELOG_zh.md)

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0] - 2026-07-06

### Component Versions

| Component | Version |
|-----------|--------|
| copilot-shell | 2.6.1 |
| agent-sec-core | 0.7.0 |
| agentsight | 0.7.1 |
| tokenless | 0.6.1 |
| agent-memory | 0.2.1 |
| os-skills | 0.6.1 |
| anolisa | 0.1.20 |
| skillfs | 0.3.2 |
| ws-ckpt | 0.4.1 |
| cosh-ng | 0.11.0 |

### Highlights

- **anolisa**: Updated to v0.1.20, delivered unified CLI gateway with full component lifecycle and adapter orchestration, users can install/update/diagnose all components with a single command
- **cosh-ng**: Updated to v0.11.0, completed Core/Shell separation and AI-augmented terminal, Agent can execute structured OS operations deterministically across distros
- **agent-memory**: Updated to v0.2.1, added user data sovereignty and 4-type memory classification, users can query/forget/control auto-captured memories
- **tokenless**: Updated to v0.6.1, added compression toggle with A/B testing and QwenCode adapter, users can quantify Token savings per strategy without affecting task execution

### New Components

- **anolisa**: First release v0.1.16, built unified CLI gateway managing component install/update/uninstall with dual-backend (RPM + Raw), users can deploy the entire ANOLISA stack with `anolisa install --all`
- **cosh-ng**: First release v0.11.0, implemented deterministic Agent-OS interface with 5-crate workspace, Agent can execute cross-distro structured system operations via stable API
- **skillfs**: First release v0.3.2, built FUSE virtual filesystem for agent skills with view-based SKILL.md exposure, Agent can discover and load skills from a mounted directory

### Updated

- **agent-memory**: Updated to v0.2.1, added sovereignty tools (about/forget/consent), AMA export/import, 4-type classification, and incremental consolidation resilient to SIGKILL, users can control memory retention and migrate memories across agents
- **tokenless**: Updated to v0.6.1, added compression on/off toggle with dry-run mode, SLS JSONL telemetry default-on, and QwenCode adapter, developers can A/B test compression strategies and monitor Token savings in SLS dashboard
- **agentsight**: Updated to v0.7.1, added Token saving visualization (strategy pie chart + line-level diff), security dashboard, and container/K8s full support, users can visually assess which optimization saves the most Tokens
- **copilot-shell**: Updated to v2.6.1, added `/model` dialog for multi-provider switching and SLS session telemetry (32-field JSONL), users can freely switch LLM providers without losing configuration
- **agent-sec-core**: Updated to v0.7.0, added Skill Ledger integrity chain with GPG signing workflow and Prompt Scanner, users can audit skill security status and get confirmation prompts before risky operations
- **os-skills**: Updated to v0.6.1, added ANOLISA Guide knowledge skill (13 official docs) and OpenClaw pre-check with bootstrap, Agent can reference accurate product documentation in responses
- **ws-ckpt**: Updated to v0.4.1, added auto-cleanup scheduling and TOML config hot-reload, users can set retention policies that take effect without restarting the daemon

### Changed

- Documentation governance established via `specs/documentation-standard.md`
- Bilingual naming convention unified to `_zh.md` (migrated from legacy `_CN.md`)

## [0.6] - 2026-06-12

### Component Versions

| Component | Version |
|-----------|--------|
| copilot-shell | 2.4.1 |
| agent-sec-core | 0.5.0 |
| agentsight | 0.5.0 |
| tokenless | 0.4.1 |
| agent-memory | 0.1.0 |
| os-skills | 0.5.0 |
| cosh-ng | 0.1.0 (MVP) |

### Highlights

- **agent-memory**: First release v0.1.0, delivered sandboxed filesystem MCP memory server, Agent can persistently store and retrieve context across sessions via BM25 search
- **tokenless**: Updated to v0.4.1, added Hermes Agent plugin and Tool Ready 4-stage pre-check, Agent environments are automatically validated before tool execution to avoid wasted retries
- **agentsight**: Updated to v0.5.0, added Skill-level Token metrics and Hermes support, users can pinpoint which Skills consume the most Tokens

### New Components

- **agent-memory**: First release v0.1.0, built 19-tool MCP server with namespace isolation and BM25 background index, Agent can read/write/search persistent memory in a sandboxed filesystem
- **cosh-ng**: First release (MVP), completed production-ready functionality for deterministic OS operations, Agent can execute structured commands with predictable output format

### Updated

- **tokenless**: Updated to v0.4.1, added Hermes adapter runner and Tool Ready mechanism (4-stage env pre-check as cosh extension), Agent tool calls are pre-validated reducing Token waste from environment failures
- **agentsight**: Updated to v0.5.0, added Skill-dimension Token/call metrics and Hermes matcher with SSL support, users can see per-Skill Token breakdown in the dashboard
- **agent-sec-core**: Updated to v0.5.0, added PIIChecker (output PII detection + desensitization) and Skill Scanner (text/code scan + lifecycle trigger), Agent output containing sensitive information is automatically intercepted
- **copilot-shell**: Updated to v2.4.1, added cross-session auto memory extraction and hook reason visibility in UI, users can see exactly why a security hook blocked an operation

## [0.5] - 2026-05-28

### Component Versions

| Component | Version |
|-----------|--------|
| copilot-shell | 2.4.0 |
| agent-sec-core | 0.4.0 |
| agentsight | 0.4.0 |
| tokenless | 0.4.0 |
| os-skills | 0.4.0 |

### Highlights

- **tokenless**: Updated to v0.4.0, added Hermes plugin and Tool Ready environment mechanism, Agent tool execution failures due to missing dependencies are prevented before Token consumption
- **agent-sec-core**: Updated to v0.4.0, delivered PIIChecker and Skill Scanner first version, Agent output is scanned for sensitive information leakage

### Updated

- **tokenless**: Updated to v0.4.0, developed Hermes Agent plugin with Tool Ready 4-stage env pre-check and history compression, Agent runtime dependencies are auto-verified before execution
- **agent-sec-core**: Updated to v0.4.0, added PIIChecker for output PII detection and Skill Scanner baseline capabilities, users are protected from unintentional sensitive data exposure
- **agentsight**: Updated to v0.4.0, added Skill-level metrics display, users can view Token consumption grouped by Skill
- **os-skills**: Updated to v0.4.0, added Nightly automated test coverage, skill quality is continuously validated

## [0.4] - 2026-05-13

### Component Versions

| Component | Version |
|-----------|--------|
| copilot-shell | 2.3.0 |
| agent-sec-core | 0.4.1 |
| agentsight | 0.4.0 |
| tokenless | 0.3.0 |
| os-skills | 0.3.0 |
| ws-ckpt | 0.2.0 |

### Highlights

- **agent-sec-core**: Updated to v0.4.1, established Skill security full lifecycle with Prompt Scanner ask policy, users receive confirmation prompts before Agent executes risky instructions
- **tokenless**: Updated to v0.3.0, built 4-suite Benchmark comparison baselines, developers can quantify Token savings across different Skill/OS environments
- **ws-ckpt**: Updated to v0.2.0, expanded snapshot management commands, users can auto-clean historical snapshots by count or age policy

### Updated

- **agent-sec-core**: Updated to v0.4.1, integrated Prompt Scanner into cosh hook and OpenClaw plugin with ask strategy, users get interactive confirmation before dangerous operations
- **tokenless**: Updated to v0.3.0, built batch-concurrent Benchmark platform with comparison reports, developers can one-click benchmark and compare Token savings across configurations
- **agentsight**: Updated to v0.4.0, optimized resident process memory footprint, 2C2G small-spec instances can run observability stably
- **copilot-shell**: Updated to v2.3.0, adapted SWEBench evaluation framework, developers can execute code-fix tasks and verify pass rates via cosh
- **ws-ckpt**: Updated to v0.2.0, enriched snapshot CRUD capabilities, users can manage workspace checkpoints with flexible retention policies

## [0.3] - 2026-04-30

### Component Versions

| Component | Version |
|-----------|--------|
| copilot-shell | 2.2.1 |
| agent-sec-core | 0.3.0 |
| agentsight | 0.3.1 |
| tokenless | 0.2.0 |
| os-skills | 0.3.0 |
| ws-ckpt | 0.1.0 |

### Highlights

- **tokenless**: Updated to v0.2.0, delivered command rewriting and TOON context compression, CLI output Token consumption reduced by 60–90%
- **agentsight**: Updated to v0.3.1, added Token saving Dashboard and Agent anomaly diagnostics, users can visualize savings and detect Agent interruptions
- **agent-sec-core**: Updated to v0.3.0, added Skill Ledger integrity tracking and Prompt Scanner, every Skill's signature chain is auditable end-to-end

### New Components

- **ws-ckpt**: First release v0.1.0, built btrfs-based workspace checkpoint daemon, Agent can create sub-millisecond snapshots and instantly rollback filesystem state

### Updated

- **tokenless**: Updated to v0.2.0, added command rewriting via RTK and TOON context compression, Agent CLI interactions consume 60–90% fewer Tokens
- **agentsight**: Updated to v0.3.1, added Token saving Dashboard (session/time-range stats) and Agent interrupt detection with drain mechanism, users can monitor savings trends and get alerted on Agent failures
- **agent-sec-core**: Updated to v0.3.0, added Skill Ledger full lifecycle (check/certify/bypass/status/audit) and Prompt Scanner with jailbreak detection, users can track and enforce Skill integrity policies
- **copilot-shell**: Updated to v2.2.1, added extension architecture (command extension + system Hook + instant activation), Skill marketplace integration, and session export (Markdown/HTML/JSON), users can extend cosh capabilities via plugins and export conversation history
- **os-skills**: Updated to v0.3.0, added Skill marketplace listing, Hermes install skill, and utility skills (xlsx/pdf-reader/image-gen/humanizer), users can discover and install skills from a marketplace

## [0.2] - 2026-04-15

### Component Versions

| Component | Version |
|-----------|--------|
| copilot-shell | 2.0.4 |
| agent-sec-core | 0.2.0 |
| agentsight | 0.2.2 |
| os-skills | 0.2.2 |
| tokenless | 0.1.0 |

### Updated

- **agentsight**: Updated to v0.2.2, added Token consumption observability with precise Tokenizer counting, users can view per-message Token breakdown in real time
- **copilot-shell**: Updated to v2.0.4, added independent auth (STS/ECS RAM Role) and Skill marketplace browsing, users can authenticate without AK/SK and discover available skills
- **os-skills**: Updated to v0.2.2, added SysAdmin skills (Linux IO/network/load diagnostics), Agent can independently diagnose common OS performance issues
- **tokenless**: First release v0.1.0, built Skills-level benchmark test cases, developers can compare Token consumption across different Skills quantitatively

## [0.1] - 2026-03-30

### Component Versions

| Component | Version |
|-----------|--------|
| copilot-shell | 2.0.1 |
| agent-sec-core | 0.1 |
| agentsight | 0.1 |
| os-skills | 0.1 |

### New Components

- **copilot-shell**: First release v2.0.1, built AI-powered terminal assistant with Tab completion, /bash mode, sudo support, and hook security, users get an AI-native CLI experience on first login
- **agent-sec-core**: First release v0.1, delivered Skill signature verification, security sandbox, and system hardening, Agent operations run in a controlled least-privilege environment
- **agentsight**: First release v0.1, built eBPF-based zero-intrusion observability probe, users can monitor LLM API calls and Token consumption without modifying Agent code
- **os-skills**: First release v0.1, curated system administration, SysOM, DevOps, and cloud skills, Agent can autonomously perform common OS operations

### Security

- Skill full-link encryption with digital signatures
- Hardware-level security sandbox for risk isolation
- Identity authentication and integrity verification for Skill calls

---

For detailed changelogs of individual components, see:

**User Entrypoint**
- [copilot-shell](src/copilot-shell/CHANGELOG.md)
- [cosh-ng](src/cosh-ng/CHANGELOG.md)
- [anolisa](src/anolisa/CHANGELOG.md)
- [os-skills](src/os-skills/CHANGELOG.md)

**Token Saving**
- [tokenless](src/tokenless/CHANGELOG.md)

**Runtime**
- [agent-memory](src/agent-memory/CHANGELOG.md)
- [skillfs](src/skillfs/CHANGELOG.md)
- [ws-ckpt](src/ws-ckpt/CHANGELOG.md)

**Agent Observability**
- [agentsight](src/agentsight/CHANGELOG.md)

**Agent Security**
- [agent-sec-core](src/agent-sec-core/CHANGELOG.md)
