# Changelog

All notable changes to ANOLISA will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.8] - 2026-06-15

### Added

- `anolisa adapter enable` can now register installed adapters with OpenClaw.
- `anolisa adapter disable` can now remove OpenClaw adapter registrations.
- `anolisa adapter status` can now report OpenClaw adapter health.
- `anolisa adapter scan` can now show installed adapter resources.

### Changed

- `anolisa install` now places adapter resources needed by later enablement.
- `anolisa uninstall` now blocks components that still have enabled adapters.

## [0.1.7] - 2026-06-13

### Changed

- User-mode library paths now resolve to `~/.local/lib/anolisa`; other directories continue to follow `XDG_*` overrides.

### Fixed

- `anolisa install` no longer requires a local catalog entry before downloading from the remote repository.
- `anolisa install --dry-run` can preview files and services without downloading the full package.

## [0.1.6] - 2026-06-12

### Added

- `anolisa osbase sandbox install gvisor` now supports standalone, containerd, and substrate deployments. (#851)
- `anolisa list` can derive the component catalog from `repo.toml` configuration. (#854)

### Changed

- Replaced the legacy "capability" model with a unified component lifecycle; old state auto-migrates on next write. (#876)

### Fixed

- `anolisa list --enabled` now correctly shows installed components instead of an empty list. (#872)
- `anolisa list` no longer requires a separate local catalog file when `repo.toml` is configured. (#854)

## [0.1.5] - 2026-06-11

### Added

- `anolisa list` reads from a remote or local component catalog and returns structured JSON. (#850)
- `anolisa install <component>` downloads, verifies, and installs components from the remote repository. (#852)
- `anolisa uninstall` supports the new component model while preserving legacy fallback. (#852)

### Changed

- Simplified CLI help around `list`, `install`, `uninstall`, `status`, `doctor`, `logs`, `restart`, `update`. (#850)

### Fixed

- `anolisa list` returns an empty list with a config hint when no catalog is configured. (#850)
- Failed installs now automatically roll back partially-written files. (#852)

## [0.1.4] - 2026-06-10

### Added

- `anolisa adapter scan` detects available framework integrations. (#808)
- `anolisa adapter install` downloads verified packages and registers adapters with the target framework.
- `anolisa adapter remove` safely removes only ANOLISA-managed files, with dry-run and JSON preview support.
- `anolisa adapter install tokenless openclaw` wires up the tokenless adapter via the OpenClaw CLI.
- `anolisa enable` fetches component metadata from the remote repository, with offline fallback.
- `anolisa status` now includes component health check results.

### Changed

- Renamed subscription commands to top-level `anolisa register` / `unregister`.

### Fixed

- Adapter install/remove failures now roll back or preserve state for retry.

## [0.1.3] - 2026-06-09

### Added

- `anolisa --help` now groups commands by category (everyday vs. management).
- `list` command shows its `ls` alias in help output.
- `anolisa update self` prints a changelog link on success.

### Changed

- Corrected package license metadata to Apache-2.0.

## [0.1.2] - 2026-06-08

### Added

- `anolisa bug` generates a local diagnostic report with environment info and recent error logs.
- `anolisa self update` added as an alias for `anolisa update self`.

### Fixed

- Restored the bug report issue template.

## [0.1.1] - 2026-06-07

### Added

- `anolisa osbase sandbox install` provisions sandbox environments (firecracker and e2b backends).
- `anolisa register` / `unregister` manages data-upload consent with 30-day deferral.
- `anolisa enable` can configure log upload (ilogtail) with automatic region detection.
- `anolisa update self` downloads and applies CLI updates with integrity verification and rollback.
- Real dnf/apt package manager backends replacing placeholder stubs.
- GitHub Actions CI for the anolisa workspace.

### Fixed

- Install script uses portable bash expansion instead of `sed`.

## [0.1.0] - 2026-06-04

Initial alpha release of the ANOLISA CLI.

### Added

- CLI commands: `env`, `list`, `status`, `logs`, `enable`, `disable`, `uninstall`, `restart`, `update`, `info`, `doctor`.
- Environment detection: OS, arch, kernel, distro, container runtime, user identity (graceful degradation).
- Component lifecycle engine with preview-then-execute, integrity checks, and audit logging.
- Configuration-driven feature gates for shipping new capabilities without code changes.
- Declarative TOML component manifests with multi-architecture support.
- `install-anolisa.sh` installer with three modes (local, checkout, URL), checksum verification, and `--dry-run`.
- End-to-end smoke tests for agent-observability and token-optimization.

### Capabilities shipped

| Capability | Status |
|-----------|--------|
| agent-observability | `enable` fully wired (dry-run + real-execute) |
| Others (9 total) | Manifest-only; `enable` returns NOT_IMPLEMENTED |

### Known limitations

- Real-execute paths are Linux-only (darwin hosts can `--dry-run` only).
- No signature verification or rpm/deb backend yet.
- `update` command returns NOT_IMPLEMENTED.

---

# 变更日志

本文件记录 ANOLISA 的所有重要变更。

格式基于 [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [未发布]

## [0.1.8] - 2026-06-15

### 新增

- `anolisa adapter enable` 现可将已安装适配器注册到 OpenClaw。
- `anolisa adapter disable` 现可移除 OpenClaw 适配器注册。
- `anolisa adapter status` 现可报告 OpenClaw 适配器健康状态。
- `anolisa adapter scan` 现可显示已安装适配器资源。

### 变更

- `anolisa install` 现会放置后续启用所需的适配器资源。
- `anolisa uninstall` 现会阻止移除仍有启用适配器的组件。

## [0.1.7] - 2026-06-13

### 变更

- 用户态库路径调整为 `~/.local/lib/anolisa`；其余目录继续遵循 `XDG_*` 环境变量覆盖。

### 修复

- `anolisa install` 不再要求本地已有组件目录条目即可从远程仓库下载安装。
- `anolisa install --dry-run` 无需下载完整安装包即可预览文件和服务列表。

## [0.1.6] - 2026-06-12

### 新增

- `anolisa osbase sandbox install gvisor` 支持 standalone、containerd 和 substrate 三种部署形态。(#851)
- `anolisa list` 可从 `repo.toml` 配置自动发现组件目录。(#854)

### 变更

- 废弃旧版"能力"模型，统一为组件生命周期；旧状态在下次写入时自动迁移。(#876)

### 修复

- `anolisa list --enabled` 现在正确显示已安装组件，而非空列表。(#872)
- `anolisa list` 在已配置 `repo.toml` 时不再要求额外的本地目录文件。(#854)

## [0.1.5] - 2026-06-11

### 新增

- `anolisa list` 从远程或本地组件目录读取并返回结构化 JSON。(#850)
- `anolisa install <组件>` 从远程仓库下载、校验并安装组件。(#852)
- `anolisa uninstall` 支持新组件模型，同时保留旧版回退。(#852)

### 变更

- 简化 CLI 帮助输出，围绕 `list`、`install`、`uninstall`、`status`、`doctor`、`logs`、`restart`、`update` 重新分组。(#850)

### 修复

- 未配置组件目录时，`anolisa list` 返回空列表并提示配置方法。(#850)
- 安装中途失败时自动回滚已写入的文件。(#852)

## [0.1.4] - 2026-06-10

### 新增

- `anolisa adapter scan` 探测已安装的 Agent 框架集成。(#808)
- `anolisa adapter install` 下载校验后的安装包并注册到目标框架。
- `anolisa adapter remove` 安全移除 ANOLISA 管理的文件，支持预览和 dry-run。
- `anolisa adapter install tokenless openclaw` 通过 OpenClaw CLI 注册 tokenless 适配器。
- `anolisa enable` 从远程仓库获取组件元数据，离线时降级到本地缓存。
- `anolisa status` 输出中新增组件健康检查结果。

### 变更

- 订阅管理命令提升为顶层 `anolisa register` / `unregister`。

### 修复

- adapter 安装或移除失败时自动回滚或保留状态以便重试。

## [0.1.3] - 2026-06-09

### 新增

- `anolisa --help` 按类别分组展示命令（日常操作 vs. 管理命令）。
- `list` 命令在帮助中展示 `ls` 别名。
- `anolisa update self` 成功后输出 changelog 链接。

### 变更

- 修正包 license 元数据为 Apache-2.0。

## [0.1.2] - 2026-06-08

### 新增

- `anolisa bug` 生成本地诊断报告，包含环境信息和近期错误日志。
- `anolisa self update` 作为 `anolisa update self` 的别名。

### 修复

- 恢复 bug report issue 模板。

## [0.1.1] - 2026-06-07

### 新增

- `anolisa osbase sandbox install` 一键部署沙箱环境（支持 firecracker 和 e2b 后端）。
- `anolisa register` / `unregister` 管理数据上传授权，支持 30 天延后。
- `anolisa enable` 可配置日志上传（ilogtail），自动探测地域。
- `anolisa update self` 下载并应用 CLI 更新，含完整性校验和失败回滚。
- 真实的 dnf/apt 包管理器后端，替换占位实现。
- anolisa 工作区 GitHub Actions CI。

### 修复

- 安装脚本改用 bash 参数展开替代 `sed`，提升可移植性。

## [0.1.0] - 2026-06-04

ANOLISA CLI 首个 alpha 版本。

### 新增

- CLI 命令：`env`、`list`、`status`、`logs`、`enable`、`disable`、`uninstall`、`restart`、`update`、`info`、`doctor`。
- 环境探测：OS、架构、内核、发行版、容器运行时、用户身份（探测失败时优雅降级）。
- 组件生命周期引擎：先预览再执行，含完整性校验和操作日志。
- 配置驱动的上线门控，新能力无需改代码即可发布。
- 声明式 TOML 组件清单，支持多架构。
- `install-anolisa.sh` 安装器：三种模式（本地、checkout、URL），支持校验和 `--dry-run`。
- agent-observability 和 token-optimization 端到端冒烟测试。

### 已交付能力

| 能力 | 状态 |
|-----|------|
| agent-observability | `enable` 完整链路（dry-run + 真实执行） |
| 其余 9 个 | 仅清单；`enable` 返回 NOT_IMPLEMENTED |

### 已知限制

- 真实执行路径仅限 Linux（darwin 宿主只能 `--dry-run`）。
- 尚无签名校验和 rpm/deb 后端。
- `update` 命令返回 NOT_IMPLEMENTED。
