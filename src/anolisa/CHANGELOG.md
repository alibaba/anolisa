# Changelog

All notable changes to ANOLISA will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
