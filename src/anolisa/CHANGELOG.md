# Changelog

All notable changes to ANOLISA will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.6] - 2026-06-12

### Added

- **gVisor sandbox install path**: `anolisa osbase sandbox install gvisor`
  now supports standalone, containerd runtime, and substrate control-plane
  deployments, including runsc/shim/atelet/ateom-gvisor RPM packaging,
  TOML-AST containerd config patching, Docker runtime registration, substrate
  directory provisioning, and validation/dry-run coverage (#851)
- **Repo-config catalog discovery**: `anolisa list` can now derive the
  component catalog URL from `[backends.raw].base_url` in `repo.toml`, while
  `ANOLISA_CATALOG_URL` remains an explicit override (#854)

### Changed

- **Component-only lifecycle model**: legacy capability resolver, enable,
  disable, manifests, templates, execution-policy staging, and old capability
  demo scripts were removed; CLI and JSON wire formats now consistently use
  component terminology such as `data.components`, `.component`, and
  `installed_components` (#876)
- Legacy capability state still deserializes for upgrade compatibility and is
  pruned with a warn-level audit record on the next state write (#876)

### Fixed

- `anolisa list` now reads `installed.toml` so catalog rows reflect
  `installed`/`not_installed`, and `list --enabled` returns installed
  components instead of an empty list (#872)
- `anolisa list` no longer requires a separate local `config.toml` catalog
  source when `repo.toml` or the default raw backend can provide one; missing
  catalog hints now point to `repo.toml` / raw backend configuration (#854)

## [0.1.5] - 2026-06-11

### Added

- **Component catalog listing**: `anolisa list` now reads a v1 JSON
  component catalog from `ANOLISA_CATALOG_URL` or `[catalog].url`, supports
  `file://` and `http(s)://` sources, and renders component-oriented JSON
  under `components` (#850)
- **Component-first raw install**: `anolisa install <component>` now resolves
  `repo.toml` backend configuration, selects the `raw` backend, downloads
  sha256-verified artifacts through the distribution index, installs
  manifest-declared files and symlinks, records installed state, and writes
  central-log audit records (#852)
- **Component lifecycle plumbing**: `anolisa uninstall` now understands
  component-first installed state while preserving legacy capability fallback;
  installed state records backend provenance, and cross-backend reinstalls are
  rejected until the component is uninstalled first (#852)

### Changed

- Simplified the top-level CLI command surface around component-first
  workflows, removing obsolete exposed capability commands and regrouping help
  output around `list`, `install`, `uninstall`, `status`, `doctor`, `logs`,
  `restart`, `update`, and adapter/management surfaces (#850)
- Decoupled bug report collection from the old `list` row builder so bug
  reports no longer depend on capability catalog rendering internals (#850)

### Fixed

- `anolisa list` now treats a missing catalog URL as an empty successful list
  with a config hint, while malformed catalog config remains an explicit error
  (#850)
- Raw installs now roll back files, tar.gz multi-file writes, symlinks, and
  temporary siblings if a later install step fails before state is persisted
  (#852)
- Enable dry-run smoke coverage now uses a temporary system prefix so registry
  cache and config lookups do not touch host paths during tests (#834)

## [0.1.4] - 2026-06-10

### Added

- **Agent-framework adapter lifecycle**: `anolisa adapter scan` now detects
  available framework integrations, while `adapter install` resolves verified
  tar.gz artifacts from the distribution index, reads embedded component
  manifests, expands safe layout placeholders, and records adapter state plus
  central-log entries
- **Safe adapter removal**: `anolisa adapter remove` now supports dry-run and
  JSON previews, deletes only ANOLISA-owned files within the active layout,
  refuses symlinks/directories, and records skipped files with reasons
- **OpenClaw adapter wiring**: `anolisa adapter install tokenless openclaw`
  and `adapter remove tokenless openclaw` now register/unregister through the
  OpenClaw CLI, including rollback or state retention when framework CLI
  operations fail
- **Remote registry-backed enable**: `anolisa enable` now fetches the default
  remote distribution index, caches it with TTL freshness, overlays published
  `meta.toml` contracts for resolved components, supports registry URL
  overrides, and degrades to bundled or cached indexes when offline
- **Component health checks**: component manifests can carry structured
  health checks; `enable` records post-install probe results, and `status`
  layers manifest health plus owned-file integrity probes into capability
  health output

### Changed

- Flattened co-build registration from the former subscription surface into
  top-level `anolisa register`, `anolisa register status`, and
  `anolisa unregister`
- Extended the tar.gz install runner to support directory sources, allowing
  adapter packages to install whole source directories rather than only
  basename-matched files

### Fixed

- Restored `AdapterSpec` parsing and exports after the adapter subsystem
  landed on main
- Hardened adapter install/remove failure handling so unsafe destinations are
  rejected before extraction, partial installs roll back, and failed removals
  keep state for retry

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

## [0.1.6] - 2026-06-12

### 新增

- **gVisor 沙箱安装链路**：`anolisa osbase sandbox install gvisor` 现在支持
  standalone、containerd runtime 和 substrate control-plane 三种部署形态，
  包含 runsc/shim/atelet/ateom-gvisor RPM 打包、containerd `config.toml`
  AST 级补丁、Docker runtime 注册、substrate 目录准备，以及验证和 dry-run
  覆盖 (#851)
- **基于 repo.toml 的 catalog 发现**：`anolisa list` 现在可从 `repo.toml`
  中的 `[backends.raw].base_url` 派生 component catalog URL，同时保留
  `ANOLISA_CATALOG_URL` 显式覆盖 (#854)

### 变更

- **组件唯一生命周期模型**：移除 legacy capability resolver、enable、disable、
  manifests、templates、execution-policy staging 和旧 capability demo 脚本；
  CLI 与 JSON wire format 现在统一使用 component 语义，例如
  `data.components`、`.component` 和 `installed_components` (#876)
- 为升级兼容，legacy capability state 仍可反序列化，并会在下一次写 state 时
  被剪除，同时写入 warn 级审计记录 (#876)

### 修复

- `anolisa list` 现在读取 `installed.toml`，catalog rows 可正确展示
  `installed`/`not_installed`；`list --enabled` 会返回已安装组件，而不是空列表
  (#872)
- `anolisa list` 不再要求单独的本地 `config.toml` catalog 来源；当
  `repo.toml` 或默认 raw backend 可提供 catalog 时即可正常工作，缺失 catalog
  提示也改为指向 `repo.toml` / raw backend 配置 (#854)

## [0.1.5] - 2026-06-11

### 新增

- **组件目录列表**：`anolisa list` 现在从 `ANOLISA_CATALOG_URL` 或
  `[catalog].url` 读取 v1 JSON component catalog，支持 `file://` 和
  `http(s)://` 来源，并在 JSON 输出中以 `components` 返回组件列表 (#850)
- **组件优先的 raw 安装**：`anolisa install <component>` 现在会解析
  `repo.toml` 后端配置，选择 `raw` 后端，通过 distribution index 下载并校验
  sha256，安装 manifest 声明的文件和 symlink，记录 installed state，并写入
  central-log 审计记录 (#852)
- **组件生命周期接线**：`anolisa uninstall` 现在理解组件优先的 installed
  state，同时保留旧 capability 卸载回退；installed state 会记录安装后端来源，
  跨后端重装会被拒绝，用户需先卸载再切换后端 (#852)

### 变更

- 顶层 CLI 命令面调整为组件优先流程，移除已过时的 exposed capability 命令，
  并围绕 `list`、`install`、`uninstall`、`status`、`doctor`、`logs`、
  `restart`、`update` 以及 adapter/management 管理面重新分组帮助输出 (#850)
- bug report 收集不再依赖旧 `list` row builder，避免诊断报告耦合 capability
  catalog 渲染内部实现 (#850)

### 修复

- `anolisa list` 现在在未配置 catalog URL 时返回带配置提示的空成功列表；
  malformed catalog config 仍然作为显式错误返回 (#850)
- raw 安装在持久化 state 前若后续步骤失败，现在会回滚已写入文件、tar.gz
  多文件写入、symlink 和临时 sibling 文件 (#852)
- enable dry-run 冒烟测试现在使用临时 system prefix，避免 registry cache 和
  config 查找触碰宿主机路径 (#834)

## [0.1.4] - 2026-06-10

### 新增

- **Agent framework adapter 生命周期**：`anolisa adapter scan` 现在可探测已安装的
  framework 集成；`adapter install` 会从 distribution index 解析并校验 tar.gz
  产物，读取产物内嵌 component manifest，展开安全的布局占位符，并写入 adapter
  状态和 central-log 记录
- **安全 adapter 移除**：`anolisa adapter remove` 现在支持 dry-run 和 JSON 预览，
  只删除当前布局内的 ANOLISA-owned 文件，拒绝 symlink/目录，并记录跳过文件及原因
- **OpenClaw adapter 接入**：`anolisa adapter install tokenless openclaw` 和
  `adapter remove tokenless openclaw` 现在会通过 OpenClaw CLI 注册/反注册，
  framework CLI 操作失败时会执行回滚或保留状态以便重试
- **远程 registry 驱动的 enable**：`anolisa enable` 现在默认拉取远程
  distribution index，按 TTL 缓存，针对已解析组件叠加已发布的 `meta.toml`
  契约，支持 registry URL 覆盖，并在离线时降级到 bundled 或 cached index
- **组件健康检查**：component manifest 现在可声明结构化 health check；`enable`
  会记录安装后探测结果，`status` 会把 manifest health 和 owned-file integrity
  探测合并到 capability health 输出

### 变更

- 将 co-build 注册从原 subscription 管理面扁平化为顶层 `anolisa register`、
  `anolisa register status` 和 `anolisa unregister`
- tar.gz install runner 现在支持目录 source，adapter 包可安装整个源目录，而不再
  仅限按目标 basename 匹配单文件

### 修复

- adapter subsystem 合入 main 后，恢复 `AdapterSpec` 解析和导出
- 强化 adapter install/remove 失败处理：解压前拒绝不安全目标，部分安装失败会回滚，
  移除失败会保留状态以便重试

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
