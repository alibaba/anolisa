# Changelog

## 0.6.0

**Self-Protection — Tamper-resistance for agent-sec-core itself**

- Added self-protect code-scan rules that block disabling/uninstalling agent-sec plugins on OpenClaw and Hermes. (#692)
- Optimized self-protect rules in code-scan to eliminate false positives on prefix-matched plugin names and cover Hermes uninstall/rm patterns. (#710)

**Prompt Scanner**

- Unified prompt-scan warning format across cosh-extension, hermes-plugin, and openclaw-plugin with structured fields (threat type, risk level, interception stage, model confidence). (#709)

**Agent-Sec-CLI**

- Added daemon process for agent-sec-cli to amortize startup latency across hook invocations. (#677)

**Adapter & Manifest**

- Added standalone ANOLISA adapter entry `anolisa-for-openclaw` to package sec-core OpenClaw adapter scripts and drive install/detect/uninstall via the adapter manifest. (#549)
- Added Hermes adapter runner: refactored the OpenClaw entry into a target-agnostic `anolisa-adapter-runner` and added `anolisa-for-hermes` wrapper, with per-agent adapter directory layout under sec-core. (#617)
- Centralized sec-core adapter manifest parsing across adapter scripts and moved the manifest under the cli package. (#617)

**OpenClaw Integration**

- Normalized OpenClaw state directory handling: use `OPENCLAW_STATE_DIR` for adapter filesystem state, unset `OPENCLAW_HOME` when invoking the OpenClaw CLI, and aligned plugin install/list/uninstall handling. (#641)

## 0.5.0

**PII Scanner — Personal information leak detection**

- Added PIIChecker scan CLI with text/file input, regex/validator-based detection, redaction, and security middleware integration. (#525)
- Added PIIChecker hooks for cosh and OpenClaw with stdin-based input passing. (#539)
- Added Hermes PII checker hook. (#556)
- Fixed scan-pii module mode detection via subprocess. (#540)

**Security Observability — Agent run metrics & posture insights**

- Added security observability schema, metrics definition, and CLI with jsonl writer for agent runs. (#488)
- Added openclaw plugin for security observability. (#515)
- Added cosh hook for security observability. (#528)
- Persisted observability records to sqldb with CLI review command. (#544)
- Added observability plugin for hermes. (#553)
- Correlated security events with observability events and supported batch query. (#578)
- Respected trace-id filter in count queries. (#595)

**Hermes Plugin — AI Agent integration framework**

- Added hermes-plugin framework with abstract hook class and code scan capability. (#536)
- Added Hermes prompt-scan capability. (#579)
- Added Hermes PII checker hook. (#556)
- Added Hermes skill ledger hook. (#565)
- Added observability plugin for hermes. (#553)
- Supported correlation context in hermes agent plugin. (#590)
- Added hermes plugin install for rpmbuild and build from scratch. (#577)
- Stabilized Hermes skill-ledger warning delivery for non-pass skill checks. (#600)

**Correlation & Tracing Context**

- Unified caller tracing context across CLI, OpenClaw, and cosh with `--trace-context` JSON and SQLite schema v2. (#569)
- Supported correlation context in hermes agent plugin. (#590)
- Correlated security events with observability events. (#578)

**Skill Ledger**

- Integrated code-scanner with skill-ledger for unified security assessment. (#505)
- Updated skill ledger security interactions. (#529)
- Made openclaw skill ledger approval configurable. (#575)
- Added Hermes skill ledger hook. (#565)
- Refined skill ledger scan workflow and aligned documentation. (#529)
- Included skill-ledger e2e in install flows. (#573)
- Fixed skill-ledger hook scope limitation. (#497)
- Fixed managed skill dirs for discovery. (#510)
- Expanded home paths for skill-ledger. (#596)
- Hardened skill ledger recovery and key UX. (#575)

**Code Scanner**

- Added code-scan requireApproval config for openclaw. (#560)
- Added OpenClaw enableBlock hook policies. (#586)

**Security Middleware & Event System**

- Fixed TOCTOU race condition at sqldb read path. (#546)
- Made SQLAlchemy lazy import for non-DB subcommands. (#581)
- Lowered frequency for SQL maintenance operations. (#546)

**Prompt Scanner**

- Added Hermes prompt-scan capability via hermes plugin. (#579)
- Fixed warmup detection from error-string matching to file-based check. (#500)
- Fixed prompt text passing via stdin instead of argv. (#579)

**Toolchain & CI**

- Added build-all support with local space install for sec-core. (#527)
- Added hermes plugin install for rpmbuild and from-scratch build. (#577)
- Included skill-ledger e2e in install flows. (#573)
- Added adapter manifest for capability discovery. (#577)

## 0.4.0

**Prompt Scanner**

- Prompt scanner hook now asks user on missing model instead of fail-open. (#463)
- Added prompt injection detection benchmark dataset and evaluation toolkit. (#464)

**Security Middleware & Event System**

- Refactored security_events SQLite storage to SQLAlchemy ORM with multi-table extensibility and typed repositories. (#459)

**Skill Ledger**

- Fixed sign-skill auto-register config (exact awk match) and parse openclaw stdout unconditionally. (#445)
- Unified XDG paths under `agent-sec/skill-ledger` vendor namespace. (#445)
- Unified single-skill verify into structured result for consistent output. (#445)
- Converted integration tests from subprocess to Typer CliRunner. (#445)

**OpenClaw Integration**

- Registered plugin at openclaw gateway explicitly to support Gateway startup planning. (#446)

**Refactoring**

- Removed deprecated agent-sec-core skill directory; aligned README and spec with agent-sec-cli workflow. (#454)

**Toolchain & CI**

- Added coverage report for sec-core CI. (#431)
- Enabled rpmbuild and e2e test CI for main branch. (#432)

## 0.3.0

**Prompt Scanner — Multi-layer prompt injection & jailbreak detection**

- Added prompt injection/jailbreak detection scanner architecture with L1 rule engine (YAML-based) and L2 ML classifier (Prompt Guard 2). (#253)
- Integrated prompt scanner into cosh hook and openclaw plugin with security middleware lifecycle. (#261, #294)
- Added `list-scanners` command, improved CLI help, and made `--scanner-version` optional. (#284)
- Added prompt scan summary and backend tests. (#294)
- Added prompt-scanner skill definition. (#256)
- Added model warmup, audit logging, and comprehensive documentation. (#253)
- Stabilized batch scanning and verdict logic with thread-safe model loading. (#253)
- Unified prompt scanner response to use "ask" instead of "block". (#341)
- Added prompt-scanner e2e test suite and Makefile target. (#352)

**Code Scanner — Static code security analysis**

- Added code scanner component with rule-based detection for obfuscation, permission abuse, and more. (#234)
- Integrated code scanner into cosh hook (with ask decision support) and openclaw plugin adapter. (#234)
- Added code scanner CLI entry, error codes, and unit tests. (#234)
- Fixed code scan bugs and added e2e test. (#342)

**Skill Ledger — Skill integrity tracking and signing**

- Added skill-ledger CLI with middleware integration for skill integrity verification. (#252)
- Added skill-ledger skill definition. (#266)
- Added skill-ledger cosh hook for PreToolUse and openclaw-plugin capability. (#292, #281)
- Improved skill-ledger CLI and cleaned up imports. (#284)
- Restructured skill-ledger config defaults and documentation. (#296)
- Aligned skill-ledger tool name and added path validation. (#317)
- Reworked skill-ledger status, output, and check signing. (#335)
- Skill-ledger hook hardening, e2e suite, and posture integration. (#339)
- **Known limitation:** skill directory resolution assumes dir name matches SKILL.md `name` field; see #381.

**Security Middleware & Event System**

- Added security middleware framework with unified CLI entry point and metrics integration. (#121, #220)
- Added sqldb writer & reader with query command at CLI interface for security event persistence. (#254)
- Fixed cross-process event loss in SecurityEventWriter. (#226)
- Applied corruption whitelist to stop false-positive DB rebuilds. (#338)
- Added e2e test and fixed bugs revealed during testing. (#330)

**Linux Sandbox**

- Added sandbox guard and failure handler hooks. (#362)

**OpenClaw Integration**

- Added hook plugin for openclaw with integrated security scanning capabilities. (#242)
- Added jq requires for openclaw hook package. (#370)

**Cosh Extension Integration**

- Integrated with new cosh extension API and added builtin commands. (#302)

**Performance**

- Lazy-load ML dependencies to speed up non-ML subcommands. (#318)

**Toolchain & CI**

- Migrated Python toolchain to uv package manager and pinned Python 3.11.6. (#227)
- Added sec-core RPM build CI and adapted nightly build pipeline. (#295)
- Initialized code format check CI with python-code-pretty. (#229)
- Added e2e test in RPM build CI. (#369)

**Bug Fixes**

- Preserved seharden wrapper defaults. (#236)
- Removed dynamic import at middleware router. (#277)
- Improved missing loongshield guidance. (#289)
- Fixed build errors. (#288)
- Removed openclaw hook examples and fixed documentation. (#282)

## 0.2.0

- Added Hardened skill signing pipeline and added `.skill-meta` layout. (#129)
- Added `Cargo.lock` to version control. (#149)
- Added `make install-sandbox` target. (#68)
- Fixed bubblewrap version compatibility for `--argv0` option. (#112)
- Changed Refactor SKILL.md to executable protocol and align sub-skills. (#130)