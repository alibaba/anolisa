# Changelog

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