# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## News

- **2026-03-30**: ANOLISA open sourced. [Announcement](#)

## [agentsight/v0.2.0] - 2026-04-12

### Added

- AgentSight Dashboard web UI with real-time monitoring interface. (#74)
- Agent health monitoring with offline alerting and hung process dashboard restart.
- One-click navigation from dashboard to ATIF trace analysis page.
- Conversation & session CRUD APIs.
- Token breakdown, HTTP server, and SQLite genai storage.
- 'metrics' CLI subcommand for Prometheus text output.
- /metrics endpoint to expose standard Prometheus-format data.
- Support for HTTP 2.0 protocol. (#147)
- Support to build RPM package.
- llm-tokenizer module with model name to model ID conversion.
- Makefile with build and install targets.
- OS-aware dependency installation in build-all.sh and unified Rust minimum version to 1.91.

### Fixed

- Token capture failure after openclaw gateway restart.
- HTTP2 token analysis.
- Cargo vendor error.
- Qwen template test validation error by adding user message.
- Home directory resolution.

### Changed

- Implemented ATIF Semantic Specification Adaptation. (#105)
- Added manual token computation as fallback mechanism.
- Added project origin documentation to README files.
- Merged agentsight from alibaba/anolisa repository.
- Code refactoring and preprocessor improvements.

## [0.0.1] - 2026-03-30

### Added

- **Initial Release (init 0.0.1)**: Integrating 4 core components into a unified AI Agent operating system package
- **copilot-shell**: AI-powered terminal assistant CLI for code understanding, task automation, and system management
- **agent-sec-core**: OS-level security baseline and hardening framework with sandbox isolation, and asset integrity verification
- **os-skills**: Operation and maintenance skill collection for AI Agents, covering system administration, monitoring, security, and DevOps
- **agentsight**: eBPF-based zero-intrusion AI Agent observability probe for monitoring LLM traffic, Token consumption, and running behaviors

### Security

- Skill full-link security encryption with digital signatures
- Hardware-level security sandbox for risk isolation
- Identity authentication and integrity verification for Skill calls

---

For detailed changelogs of individual components, see:
- [copilot-shell CHANGELOG](src/copilot-shell/CHANGELOG.md)
- [os-skills CHANGELOG](src/os-skills/CHANGELOG.md)
