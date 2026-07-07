# Changelog

All notable changes to ANOLISA will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-07-07

### Added

- Raw components can now declare `conflicts` to block incompatible raw installs.

### Fixed

- `anolisa install` now rejects raw component conflicts before changing the host.
- `anolisa install --dry-run` now reports raw component conflicts instead of showing an invalid plan.

## [0.1.20] - 2026-07-03

### Added

- ANOLISA can now be distributed as `@anolisa/cli` with Linux x64 and arm64 binaries.
- `repo.toml` now enables the npm backend for component distribution.

### Changed

- `anolisa list` now shows local state, ownership, and next action for each component.
- `anolisa list --json` now includes RPM package, version, architecture, and source repository details.

### Fixed

- RPM installs and updates now keep system repositories available for dependencies.
- Adapter commands now distinguish missing component manifests from invalid manifests.

## [0.1.19] - 2026-07-02

### Fixed

- `anolisa adapter disable --dry-run` now previews cleanup without removing adapter receipts or resources.
- Read-only commands now use downloaded repo config when saving `repo.toml` fails.
- Component commands now accept package aliases consistently when targeting installed components.
- Ambiguous package aliases no longer choose an arbitrary installed component.
- Unknown component names now report no match without querying packages.

## [0.1.18] - 2026-07-01

### Added

- `anolisa install` now auto-installs missing system packages for raw components in system mode.
- `anolisa install --dry-run` now labels unresolved dependencies as auto-install or manual.
- `anolisa install` now reports packages auto-installed during raw component installs.
- `anolisa status --verbose` now shows packages auto-installed for each component.

### Changed

- Commands that need repo access now download and validate `repo.toml` on first use.
- Repo config dry-runs now fetch and validate without writing `repo.toml`.
- RPM install and update now use only the `repo.toml` RPM repository.
- User-mode raw installs now report missing dependencies with install commands before changing files.
- Failed raw installs now list any auto-installed packages left on the system.
- `anolisa update self` no longer fetches repo config before checking CLI updates.

### Fixed

- `anolisa list --installed` now includes adopted RPM components.
- `anolisa list` now shows adopted, failed, and disabled component statuses.
- Adapter commands now prefer resources from the datadir that supplied the component contract.
- RPM installs now fail before `dnf` when `[backends.rpm]` is missing.
- RPM updates now explain missing `[backends.rpm]` instead of using host repositories.

## [0.1.17] - 2026-06-30

### Added

- Repository `components.toml` can now define component names, package aliases, and raw/RPM package mappings.
- `anolisa list --installed` now filters installed components.

### Changed

- `anolisa list` and `install --all` now read components from `components.toml` instead of `catalog.json`.
- `anolisa list` now shows `NAME`, `SUMMARY`, `BACKENDS`, and `STATUS`.
- `anolisa list --enabled` is now a hidden alias for `--installed`.
- `ANOLISA_CATALOG_URL` no longer changes list sources; configure `repo.toml` instead.
- `anolisa install`, `status`, `adopt`, and `repair` now resolve RPM package aliases from `components.toml`.
- `anolisa status` now suggests `sudo anolisa adopt <component>` for untracked RPM components.

### Fixed

- `anolisa status <RPM package>` now reports the canonical component row when an alias is installed.
- `anolisa repair <RPM package>` now refreshes the canonical component row when an alias is used.
- Non-root `anolisa osbase` mutations now reach the system helper instead of failing install-mode checks.
- Commands now reject root `--install-mode user` before writing ambiguous user-mode state.
- System-mode write commands now fail before changes when sudo is missing.
- Existing installs with managed symlinks no longer show false symlink integrity failures after upgrade.
- `anolisa status` now reports `referent_mismatch` when managed symlinks point elsewhere.

## [0.1.16] - 2026-06-29

### Added

- `anolisa osbase sandbox install runc` now installs runc, containerd, Docker, and Docker client.
- `anolisa osbase sandbox install` now enables services declared by sandbox scenarios.
- `anolisa osbase sandbox install` now runs scenario verification commands after installation.
- `anolisa osbase sandbox install` now records sandbox scenarios in `installed.toml`.
- `anolisa osbase sandbox install` now reports optional scenario packages as hints.
- `rund`, `firecracker`, and `gvisor` sandbox scenarios now define post-install checks.
- `anolisa adapter enable` now supports `adapter_type = "skill_bundle"` for OpenClaw and Hermes skills.
- The RPM package now installs default `repo.toml` to `/etc/anolisa/repo.toml`.
- ANOLISA telemetry setup now installs log rotation for ops `.jsonl` files.

### Changed

- `anolisa osbase sandbox install --dry-run` now shows preflight, package, service, verify, and state phases.
- `anolisa osbase sandbox install runc` now requires Linux kernel 4.18 or newer.
- `anolisa osbase sandbox install` now reports verification failures as warnings when other phases succeed.
- `repo.toml` now points RPM installs to the agentic-os repository path.
- `anolisa update self --json` now reports apply mode, RPM package, and RPM version observations.
- `anolisa adapter status` now treats skill bundles as healthy without plugin registration.
- `anolisa adapter enable` now rejects skill bundles that declare framework config entries.

### Fixed

- RPM-backed commands now use the component name as the default package name.
- `anolisa update self` now delegates RPM-owned CLI updates to `dnf`.
- Non-root sandbox installs now show package, service, verify, and state phases.
- Telemetry setup now runs the ilogtail installer with bash-compatible script handling.
- `anolisa adapter disable` now cleans skill bundles without plugin unregister errors.

## [0.1.15] - 2026-06-25

### Added

- `anolisa doctor` now reports component health, dependency status, and suggested fixes.
- Raw components can now declare runtime dependencies for install and update preflight checks.
- `anolisa install --dry-run` now previews runtime dependency status for raw components.

### Changed

- `anolisa install` and `update <component>` now refuse raw components with missing runtime dependencies before changing files.
- `anolisa restart <component>` now restarts service units shipped by RPM-backed components.
- `anolisa restart <component>` now shows guidance for RPM-backed template services instead of failing.

### Fixed

- `anolisa adapter enable` now expands `{datadir}` from the package that provided the adapter metadata.
- After `anolisa uninstall` or `forget`, adapter commands no longer see stale component metadata.

## [0.1.14] - 2026-06-24

### Added

- Raw components can now place systemd unit files with `{unitdir}`.
- Raw components can now place user service unit files with `{userunitdir}`.
- User-mode `anolisa install` now activates declared user-scope services.

### Changed

- User-mode `anolisa install` now resolves `%u` service templates to the current user.
- System-mode `anolisa install` now preserves `%u` user service templates for later per-user activation.
- `anolisa uninstall` now reloads systemd after removing declared service unit files.
- `anolisa restart <component>` now restarts user-scope services from user-mode installs.

### Fixed

- `anolisa install` now starts freshly installed service units without a manual systemd reload.
- `anolisa uninstall` now deactivates user-scope services from user-mode installs.
- `anolisa adapter enable` now finds `{datadir}` skills from the package directory that provides the adapter.

## [0.1.13] - 2026-06-23

### Added

- `anolisa adapter enable` now supports Hermes plugins.
- `anolisa adapter enable` now installs declared OpenClaw skills.
- `anolisa adapter enable` now applies declared OpenClaw config values.
- `anolisa install` now starts declared services for raw components.
- `anolisa install` now applies declared file capabilities for raw components.
- `anolisa install` now runs declared hooks for raw components.
- `anolisa update <component>` now restarts declared services for raw components.
- `anolisa update <component>` now reapplies declared file capabilities for raw components.
- `anolisa uninstall` now runs declared hooks for raw components.
- `anolisa uninstall` now disables declared services after stopping them.

### Changed

- `anolisa adapter scan` now honors declared adapter resource locations.
- `anolisa adapter enable` now reads package-installed adapter resources.
- `anolisa install --dry-run` now previews declared capabilities for raw components.
- `anolisa register status` now reports the latest registration after repeated changes.
- Cancelled `anolisa register` and `unregister` prompts now exit successfully.

### Fixed

- `anolisa adapter status` now detects OpenClaw plugins from wrapped table output.
- `anolisa adapter status` now ignores bundled Hermes plugins during checks.
- `anolisa adapter` commands now find metadata shipped by RPM-installed components.
- `anolisa register status` now reports sysom console registrations as active.

## [0.1.12] - 2026-06-22

### Added

- `anolisa update <component>` can update raw-managed components from the raw backend.
- `anolisa osbase sandbox list` shows scenarios from `sandbox.toml`.
- `anolisa osbase sandbox uninstall <scenario>` can remove packages for a sandbox scenario.
- `anolisa system setup` can install the helper service for non-root osbase commands.
- `anolisa system status` can show helper health, version, uptime, and last operation.
- `anolisa system teardown` can remove the helper service and sandbox config.
- `anolisa env --json` includes distro identity fields.

### Changed

- `anolisa osbase sandbox install <scenario>` now installs scenarios defined in `sandbox.toml`.
- Omitting `--install-mode` now selects `system` for root and `user` otherwise.
- `anolisa update <component> --dry-run` now lists raw backend candidate versions.

### Fixed

- Legacy `yum` backend names in `repo.toml` and `--backend` now resolve to `rpm`.
- Raw components installed with `--package` now update from the same package name.
- `anolisa update <component>` now refuses raw updates that would downgrade a component.
- `anolisa update <component>` now refuses raw updates when versions cannot be safely compared.

## [0.1.11] - 2026-06-18

### Added

- `anolisa adopt <component>` can track a pre-installed system RPM without installing it.
- `anolisa repair <component>` can refresh RPM component state after package details change.
- `anolisa forget <component>` can stop tracking a component without removing packages or files.

### Changed

- `anolisa status <component>` now reports drifted RPM components when system package details change.
- `anolisa uninstall` now keeps observed system RPMs unless `--remove-system-package` is used.
- `anolisa install` now preserves adapter package resources when adopting RPM components.

## [0.1.10] - 2026-06-17

### Added

- `anolisa install --backend rpm` can install missing RPM components through `dnf` and track them as managed.
- `anolisa install` can adopt matching pre-installed system RPMs without downloading a raw package.
- `anolisa update <component>` can update RPM-managed and RPM-observed components through `dnf`.
- `anolisa status` now shows package, version, architecture, and source repo for RPM-backed components.
- `anolisa status <component>` now reports matching untracked system RPMs as observed.

### Changed

- `anolisa update runtime <component>` is now `anolisa update <component>`; `self` and `all` stay subcommands.
- `repo.toml` now uses `[backends.rpm]` instead of `[backends.yum]`.
- `anolisa install --all` now lists adopted RPM components in the batch summary.

### Fixed

- `anolisa install --all` now prints the reason for each failed component in human output.
- `anolisa install` now refuses automatic RPM detection when `rpm` or `dnf` is missing, with a `--backend raw` hint.
- `anolisa install` no longer replaces a raw install if another install finishes first.

## [0.1.9] - 2026-06-16

### Added

- `anolisa install --all` can install every available component from the catalog.
- `anolisa install --all --fail-fast` can stop after the first failed component.
- `anolisa install --all --json` returns one batch summary with per-component results.
- `anolisa status` now shows adapter summaries for installed components.

### Changed

- `installed.toml` now distinguishes ANOLISA-managed packages from observed system RPMs.

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

## [0.2.0] - 2026-07-07

### 新增

- Raw 组件现可声明 `conflicts` 阻止不兼容安装。

### 修复

- `anolisa install` 现会在变更主机前拒绝 Raw 组件冲突。
- `anolisa install --dry-run` 现报告 Raw 组件冲突，不再显示无效计划。

## [0.1.20] - 2026-07-03

### 新增

- ANOLISA 现可通过 `@anolisa/cli` 发布 Linux x64 和 arm64 二进制。
- `repo.toml` 现启用 npm 后端用于组件分发。

### 变更

- `anolisa list` 现显示本地状态、归属和下一步操作。
- `anolisa list --json` 现包含 RPM 包名、版本、架构和来源。

### 修复

- RPM 安装和更新现可继续使用系统软件源解析依赖。
- Adapter 命令现区分缺失清单和无效清单。

## [0.1.19] - 2026-07-02

### 修复

- `anolisa adapter disable --dry-run` 现只预览清理。
- 只读命令保存 `repo.toml` 失败时仍可使用已下载配置。
- 组件命令现可一致接受软件包别名。
- 模糊软件包别名不再误选已安装组件。
- 未知组件名不再先查询软件包。

## [0.1.18] - 2026-07-01

### 新增

- `anolisa install` 系统模式现自动安装缺失系统包。
- `anolisa install --dry-run` 现标出依赖处理方式。
- `anolisa install` 现显示自动安装的系统包。
- `anolisa status --verbose` 现显示组件自动安装包。

### 变更

- 需要仓库的命令现首次使用会下载 `repo.toml`。
- 仓库配置 dry-run 现只校验不写入。
- RPM 安装和更新现只使用 `repo.toml` 源。
- 用户模式 raw 安装现先提示缺失依赖。
- raw 安装失败现提示已保留的自动安装包。
- `anolisa update self` 不再预先获取仓库配置。

### 修复

- `anolisa list --installed` 现包含已收编 RPM。
- `anolisa list` 现显示 adopted、failed、disabled 状态。
- Adapter 命令现优先使用契约所在数据目录资源。
- 缺少 `[backends.rpm]` 时 RPM 安装不再调用 `dnf`。
- RPM 更新缺少 `[backends.rpm]` 时不再使用主机源。

## [0.1.17] - 2026-06-30

### 新增

- 仓库 `components.toml` 现可声明组件与包名映射。
- `anolisa list --installed` 现过滤已安装组件。

### 变更

- `anolisa list` 和 `install --all` 现读取 `components.toml`。
- `anolisa list` 现显示 NAME、SUMMARY、BACKENDS、STATUS。
- `anolisa list --enabled` 现作为隐藏别名保留。
- `ANOLISA_CATALOG_URL` 不再控制列表来源。
- `install`、`status`、`adopt`、`repair` 现解析 RPM 包别名。
- `anolisa status` 现提示用 `sudo anolisa adopt` 收编 RPM。

### 修复

- `status <RPM 包>` 现显示规范组件行。
- `repair <RPM 包>` 现刷新规范组件行。
- 非 root `osbase` 变更命令现可进入系统 helper。
- root 用户模式现会在写入前被拒绝。
- 缺少 sudo 的系统模式写入现提前失败。
- 旧符号链接安装不再误报完整性失败。
- `status` 现报告符号链接目标不匹配。

## [0.1.16] - 2026-06-29

### 新增

- `anolisa osbase sandbox install runc` 现安装 runc、containerd、Docker 和客户端。
- `anolisa osbase sandbox install` 现启用场景声明的服务。
- `anolisa osbase sandbox install` 现执行场景安装校验。
- `anolisa osbase sandbox install` 现记录沙箱安装状态。
- `anolisa osbase sandbox install` 现提示可选场景包。
- `rund`、`firecracker`、`gvisor` 场景现声明安装校验。
- `anolisa adapter enable` 现支持 `adapter_type = "skill_bundle"`。
- RPM 包现安装默认 `/etc/anolisa/repo.toml`。
- ANOLISA 遥测现为 `.jsonl` 运维日志配置轮转。

### 变更

- `anolisa osbase sandbox install --dry-run` 现显示五个安装阶段。
- `anolisa osbase sandbox install runc` 现要求 Linux 4.18 及以上。
- `anolisa osbase sandbox install` 校验失败现作为警告报告。
- `repo.toml` 默认 RPM 源现指向 agentic-os 路径。
- `anolisa update self --json` 现包含 RPM 包和版本信息。
- `anolisa adapter status` 现不要求技能包注册插件。
- `anolisa adapter enable` 现拒绝带配置的技能包。

### 修复

- RPM 相关命令现默认用组件名作为包名。
- `anolisa update self` 现通过 `dnf` 更新 RPM 安装。
- 非 root 沙箱安装现显示完整阶段结果。
- ilogtail 安装脚本需 bash 时现能正常运行。
- `anolisa adapter disable` 清理技能包时不再报插件卸载错误。

## [0.1.15] - 2026-06-25

### 新增

- `anolisa doctor` 现输出组件健康、依赖和修复建议。
- raw 组件现可声明运行依赖供安装和更新检查。
- `anolisa install --dry-run` 现预览 raw 组件依赖状态。

### 变更

- `anolisa install` 和 `update <component>` 缺依赖时先停止。
- `anolisa restart <component>` 现重启 RPM 组件服务。
- `anolisa restart <component>` 遇到 RPM 模板服务时给出指引。

### 修复

- `anolisa adapter enable` 现按包内元数据展开 `{datadir}`。
- `anolisa uninstall` 和 `forget` 后 adapter 不再见旧元数据。

## [0.1.14] - 2026-06-24

### 新增

- raw 组件现可用 `{unitdir}` 放置系统单元。
- raw 组件现可用 `{userunitdir}` 放置用户单元。
- 用户模式 `anolisa install` 现会激活用户服务。

### 变更

- 用户模式 `anolisa install` 现将 `%u` 展开为当前用户。
- 系统模式 `anolisa install` 现保留 `%u` 用户模板。
- `anolisa uninstall` 删除单元文件后现会重载 systemd。
- `anolisa restart <component>` 现会重启用户服务。

### 修复

- `anolisa install` 现无需手动重载即可启动新单元。
- `anolisa uninstall` 现会停用用户模式安装的服务。
- `anolisa adapter enable` 现从包目录查找 `{datadir}` 技能。

## [0.1.13] - 2026-06-23

### 新增

- `anolisa adapter enable` 现支持 Hermes 插件。
- `anolisa adapter enable` 现安装声明的 OpenClaw 技能。
- `anolisa adapter enable` 现写入声明的 OpenClaw 配置。
- `anolisa install` 现启动 raw 组件声明的服务。
- `anolisa install` 现设置 raw 组件声明的文件能力。
- `anolisa install` 现执行 raw 组件声明的钩子。
- `anolisa update <component>` 现重启 raw 组件声明的服务。
- `anolisa update <component>` 现重设 raw 组件声明的文件能力。
- `anolisa uninstall` 现执行 raw 组件卸载钩子。
- `anolisa uninstall` 现停用已停止的声明服务。

### 变更

- `anolisa adapter scan` 现按声明位置查找资源。
- `anolisa adapter enable` 现读取包内适配器资源。
- `anolisa install --dry-run` 现预览 raw 文件能力。
- `anolisa register status` 现显示最新注册记录。
- 取消 `anolisa register` 或 `unregister` 不再报错。

### 修复

- `anolisa adapter status` 现能识别换行的 OpenClaw 表格。
- `anolisa adapter status` 检查 Hermes 时忽略内置插件。
- `anolisa adapter` 现能找到 RPM 组件附带的元数据。
- `anolisa register status` 现显示 sysom 控制台注册。

## [0.1.12] - 2026-06-22

### 新增

- `anolisa update <component>` 可更新 raw 组件。
- `anolisa osbase sandbox list` 可显示 `sandbox.toml` 场景。
- `anolisa osbase sandbox uninstall <scenario>` 可移除场景软件包。
- `anolisa system setup` 可为非 root osbase 命令安装助手服务。
- `anolisa system status` 可显示助手健康状态。
- `anolisa system teardown` 可移除助手服务和沙箱配置。
- `anolisa env --json` 现包含发行版身份字段。

### 变更

- `anolisa osbase sandbox install <scenario>` 现按 `sandbox.toml` 安装场景。
- 未指定 `--install-mode` 时，root 用 `system`，普通用户用 `user`。
- `anolisa update <component> --dry-run` 现显示 raw 候选版本。

### 修复

- 旧 `yum` 后端名现会作为 `rpm` 处理。
- 用 `--package` 安装的 raw 组件更新时复用包名。
- `anolisa update <component>` 不再允许 raw 降级。
- `anolisa update <component>` 无法比较版本时不再替换文件。

## [0.1.11] - 2026-06-18

### 新增

- `anolisa adopt <component>` 可接管预装 RPM。
- `anolisa repair <component>` 可刷新漂移的 RPM 状态。
- `anolisa forget <component>` 可停止跟踪组件。

### 变更

- `anolisa status <component>` 现报告 RPM 状态漂移。
- `anolisa uninstall` 默认保留观察到的系统 RPM。
- `anolisa install` 接管 RPM 时保留适配器资源。

## [0.1.10] - 2026-06-17

### 新增

- `anolisa install --backend rpm` 可通过 `dnf` 安装缺失 RPM 组件。
- `anolisa install` 可接管匹配的预装系统 RPM。
- `anolisa update <component>` 可通过 `dnf` 更新 RPM 组件。
- `anolisa status` 现显示 RPM 组件的软件包来源。
- `anolisa status <component>` 现显示匹配的未跟踪系统 RPM。

### 变更

- `anolisa update runtime <component>` 改为 `anolisa update <component>`。
- `repo.toml` 现使用 `[backends.rpm]` 替代 `[backends.yum]`。
- `anolisa install --all` 现在批量摘要列出接管的 RPM。

### 修复

- `anolisa install --all` 现在普通输出显示各组件失败原因。
- `anolisa install` 在缺少 `rpm` 或 `dnf` 时提示 `--backend raw`。
- `anolisa install` 不再覆盖先完成的 raw 安装。

## [0.1.9] - 2026-06-16

### 新增

- `anolisa install --all` 可安装目录中的所有可用组件。
- `anolisa install --all --fail-fast` 可在首个失败组件后停止。
- `anolisa install --all --json` 现返回按组件汇总的批量结果。
- `anolisa status` 现显示已安装组件的适配器摘要。

### 变更

- `installed.toml` 现区分 ANOLISA 管理包和只观察的系统 RPM。

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
