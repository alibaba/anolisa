# Changelog

All notable changes to ANOLISA will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.3] - 2026-06-09

### Added

- **Grouped CLI help**: top-level `anolisa --help` now separates everyday
  capability commands from independent management surfaces, with sections
  generated from the clap command model so new subcommands appear in the
  correct group automatically
- **Help alias display**: the `list` command now exposes its `ls` alias in
  help output
- **Self-update changelog link**: successful `anolisa update self` runs now
  print the published CLI changelog URL

### Changed

- Corrected workspace package license metadata to Apache-2.0

## [0.1.2] - 2026-06-08

### Added

- **Bug report command**: `anolisa bug` generates local diagnostic Markdown
  and JSON payloads with environment facts, enabled capability state, and
  recent warn/error central-log records
- **Self-update alias**: `anolisa self update` now delegates to the existing
  `anolisa update self` flow

### Fixed

- Restored and simplified the bug report issue template

## [0.1.1] - 2026-06-07

### Added

- **Sandbox install pipeline**: 5-phase orchestrator for sandbox
  provisioning with firecracker standard and e2b backend variants,
  including pre-flight checks, package installation, OS primitives,
  service setup, and post-verify phases
- **Subscription consent management**: Token collection state machine
  (register/unregister/later) with atomic-write persistence, 30-day
  later expiry, and sysom co-registration detection
- **Upload enablement**: ilogtail install/configure/teardown with
  region-id probing (metadata API → cloud-init → public fallback),
  SLS account management, and enable_sls_log marker
- **Self-update**: `anolisa update self` with release-manifest based
  updater, tar.gz artifact download, checksum verification, extraction,
  exclusive locking, and replacement rollback
- **Package manager backends**: Real dnf/apt implementations replacing
  placeholder stubs
- **CI integration**: GitHub Actions automation for anolisa workspace

### Fixed

- Replace `sed` with bash parameter expansion in install script for
  improved portability and correctness

## [0.1.0] - 2026-06-04

Initial alpha release of the ANOLISA CLI.

### Added

- **Workspace scaffold**: Cargo workspace with five crates (anolisa-cli,
  anolisa-core, anolisa-env, anolisa-build, anolisa-platform)
- **CLI command surface**: `env`, `list`, `status`, `logs`, `enable`,
  `disable`, `uninstall`, `restart`, `update`, `info`, `doctor` commands
  via clap derive
- **Environment detection**: Stateless `EnvService` probing OS, arch,
  libc, kernel, distro family, BTF, CAP_BPF, container runtime, and
  user identity with graceful degradation
- **Capability lifecycle engine**: Plan-then-execute semantics for
  enable/disable/uninstall/purge with journaled transactions, sha256
  verification, central audit log, and exclusive install lock
- **Execution policy**: TOML-driven capability graduation gate allowing
  new capabilities to ship without code changes
- **Manifest system**: Declarative TOML manifests for capabilities,
  components (runtime + osbase), and distribution index with multi-arch
  artifact resolution
- **Installer**: `install-anolisa.sh` supporting three modes (from-local,
  auto-checkout, URL-fetch) with staging-then-promote flow, checksum
  verification, `--strict` audit, and `--dry-run`
- **Demo scripts**: End-to-end smoke tests for agent-observability
  (enable/disable/uninstall) and token-optimization lifecycle
- **Schema templates**: Seven TOML templates documenting canonical
  manifest schemas for all entity types

### Capabilities shipped

| Capability | Status |
|-----------|--------|
| agent-observability | `enable` fully wired (dry-run + real-execute) |
| Others (9 total) | Manifest-only; `enable` returns NOT_IMPLEMENTED |

### Known limitations

- Linux-only for real-execute paths (darwin hosts can `--dry-run` only)
- Distribution index carries placeholder sha256 (P1-J operations pending)
- No signature verification, no rpm/deb backend yet
- `update` command returns NOT_IMPLEMENTED

---

# 变更日志

本文件记录 ANOLISA 的所有重要变更。

格式基于 [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [未发布]

## [0.1.3] - 2026-06-09

### 新增

- **分组 CLI 帮助**：顶层 `anolisa --help` 现在区分日常 capability 命令和独立
  management 管理面，分组内容由 clap 命令模型生成，新子命令会自动出现在对应分组
- **帮助中的别名展示**：`list` 命令现在会在帮助输出中展示 `ls` 别名
- **自更新 changelog 链接**：`anolisa update self` 成功更新后会输出已发布的 CLI
  changelog URL

### 变更

- 将 workspace package license 元数据修正为 Apache-2.0

## [0.1.2] - 2026-06-08

### 新增

- **Bug report 命令**：`anolisa bug` 生成本地诊断 Markdown 和 JSON
  payload，包含环境事实、已启用能力状态、近期 warn/error central-log 记录
- **自更新别名**：`anolisa self update` 复用现有 `anolisa update self` 流程

### 修复

- 恢复并简化 bug report issue template

## [0.1.1] - 2026-06-07

### 新增

- **沙箱安装流水线**：5 阶段编排器，支持 firecracker 标准和 e2b 后端变体，
  包含预检、包安装、OS 原语、服务配置和安装后验证阶段
- **订阅同意管理**：令牌采集状态机（register/unregister/later），支持原子写入
  持久化、30 天 later 过期、sysom 联合注册检测
- **上传使能**：ilogtail 安装/配置/拆卸，支持 region-id 探测（metadata API →
  cloud-init → 公网回退）、SLS 账号管理及 enable_sls_log 标记文件
- **自更新**：`anolisa update self` 基于发布清单的更新器，支持 tar.gz
  产物下载、校验和验证、解压、排他锁及替换回滚
- **包管理器后端**：dnf/apt 真实实现，替换占位符 stub
- **CI 集成**：anolisa 工作区的 GitHub Actions 自动化

### 修复

- 安装脚本中用 bash 参数展开替代 `sed`，提升可移植性和正确性

## [0.1.0] - 2026-06-04

ANOLISA CLI 首个 alpha 版本。

### 新增

- **工作区脚手架**：Cargo workspace 包含五个 crate（anolisa-cli、
  anolisa-core、anolisa-env、anolisa-build、anolisa-platform）
- **CLI 命令面**：通过 clap derive 实现 `env`、`list`、`status`、`logs`、
  `enable`、`disable`、`uninstall`、`restart`、`update`、`info`、`doctor`
  命令
- **环境探测**：无状态 `EnvService`，探测 OS、架构、libc、内核、发行版族、
  BTF、CAP_BPF、容器运行时及用户身份，所有探针优雅降级
- **能力生命周期引擎**：enable/disable/uninstall/purge 采用
  plan-then-execute 语义，支持日志式事务、sha256 校验、集中审计日志、
  排他安装锁
- **执行策略**：TOML 驱动的能力毕业门控，新能力无需改代码即可上线
- **清单系统**：声明式 TOML 清单，覆盖 capability、component（runtime +
  osbase）和 distribution index，支持多架构产物解析
- **安装器**：`install-anolisa.sh` 支持三种模式（from-local、auto-checkout、
  URL-fetch），采用暂存后提升流程，支持校验和验证、`--strict` 审计及
  `--dry-run`
- **演示脚本**：agent-observability（enable/disable/uninstall）和
  token-optimization 生命周期端到端冒烟测试
- **模式模板**：七个 TOML 模板文件，文档化所有实体类型的规范清单结构

### 已交付能力

| 能力 | 状态 |
|-----|------|
| agent-observability | `enable` 完整链路（dry-run + 真实执行） |
| 其余 9 个 | 仅清单；`enable` 返回 NOT_IMPLEMENTED |

### 已知限制

- 真实执行路径仅限 Linux（darwin 宿主只能 `--dry-run`）
- Distribution index 中 sha256 为占位符（P1-J 运维工作待完成）
- 尚无签名校验、rpm/deb 后端
- `update` 命令返回 NOT_IMPLEMENTED
