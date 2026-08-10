# 更新日志

[English](CHANGELOG.md)

Tokenless 的所有重要变更都会记录在此文件中。

从 0.7.2 版本开始，发布记录遵循
[Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 格式。

## [未发布]

## [0.7.5] - 2026-08-10

### 新增

- OpenCode 用户现在可以通过无冲突的本地 Plugin 启用 Tokenless，并复用现有的就绪检查、命令重写、Schema 压缩和响应压缩 Hook（[1233cfcf](https://github.com/alibaba/anolisa/commit/1233cfcfd863de4bca7819b0a98615c569da2c9a)）。

### 变更

- Qoder Adapter 现在使用原生 Plugin 和 Hook 约定，在保持 fail-open 行为的同时原位替换压缩后的 Tool Output（[13817938](https://github.com/alibaba/anolisa/commit/13817938f0a8cf2b8df78d3e59f97302e4fb1947)）。

### 修复

- 重写后的 Shell 命令现在使用解析出的 `rtk` 绝对路径，因此在 `PATH` 受限的 Agent 环境中仍可正常执行（[ae83f7d3](https://github.com/alibaba/anolisa/commit/ae83f7d3ef9c85d5f42e7b1c0fd6884a0ffc4869)）。
- Qoder 和 OpenClaw Hook 现在会跨命令重写与 Proxy 边界保留 Agent、Session 和 Tool 归因信息（[#2158](https://github.com/alibaba/anolisa/issues/2158)、[2f330656](https://github.com/alibaba/anolisa/commit/2f330656fe94fc5936e1ebcaf586d7ebcd7df0d5)）。
- Adapter 安装现在可识别旧版 `/usr/local` 布局、推荐使用 RPM 升级模式，并在升级时删除过期的已打包用户手册文件（[f7ce3878](https://github.com/alibaba/anolisa/commit/f7ce38786cfba318614849a73a7f9acb693ea803)、[ec25d516](https://github.com/alibaba/anolisa/commit/ec25d516b8c05ebd8e88a703f57708749c8032ab)、[917f151e](https://github.com/alibaba/anolisa/commit/917f151ea4157850b51162668b3e1a441fb04262)）。

## [0.7.4] - 2026-07-31

### 新增

- Tokenless 现在可通过 npm 安装到 Linux 和 macOS x64/arm64，并包含 `tokenless`、`rtk`、`toon` 二进制文件以及 Framework Adapter（[#1929](https://github.com/alibaba/anolisa/pull/1929)）。
- `tokenless stats diff` 现在可通过文本或 JSON 报告以及有界 Unified Diff，说明 Record、Session 和 Tool Use 的预计节省量（[#1991](https://github.com/alibaba/anolisa/pull/1991)）。
- `TOKENLESS_DATA_DIR` 现在可为统计数据库和可逆压缩数据库设置一个可信目录，同时保留各数据库的独立覆盖项（[#2038](https://github.com/alibaba/anolisa/pull/2038)）。

### 修复

- Qwencode Adapter 现在声明其提供的 `compress-toon` Capability，使 Adapter 发现结果与其压缩行为保持一致（[#1945](https://github.com/alibaba/anolisa/pull/1945)）。
- Hermes 副本安装现在可从可信的系统、XDG 和用户数据路径解析共享 Hook 资源，并在找不到安全候选路径时提供可操作的诊断信息（[#2058](https://github.com/alibaba/anolisa/pull/2058)）。

## [0.7.3] - 2026-07-28

### 新增

- ANOLISA 现在可在 macOS 上安装 Tokenless，并将 Qwencode 作为独立 Adapter 启用（[#1964](https://github.com/alibaba/anolisa/pull/1964)）。

### 变更

- Adapter Hook 现在可在用户、`/usr/local`、RPM 和旧版安装布局中发现 `tokenless`、`rtk` 和 `toon`（[#1957](https://github.com/alibaba/anolisa/pull/1957)）。
- Hook Launcher 现在优先使用当前安装中的资源，避免多个 Tokenless 安装共存时混用不同版本（[#1964](https://github.com/alibaba/anolisa/pull/1964)）。

### 修复

- Tool Schema 压缩现在读取 Cosh 和 Cosh-NG 的规范请求字段，因此 Schema 会被压缩，而不再静默原样通过（[#1894](https://github.com/alibaba/anolisa/pull/1894)）。
- 存在 Hook 环境变量时，Cosh-NG 压缩统计现在归因到 `cosh-ng`（[#1894](https://github.com/alibaba/anolisa/pull/1894)）。
- Qoder Plugin 安装现在展开缓存的 Hook 路径，避免无效的 `/rewrite_hook.py` 命令阻塞 Tool Call；用户手册也包含受影响升级的恢复步骤（[#1924](https://github.com/alibaba/anolisa/pull/1924)）。
- ANOLISA Package 现在包含 Tokenless Adapter 所需的共享 Hook 资源（[#1964](https://github.com/alibaba/anolisa/pull/1964)）。

## [0.7.2] - 2026-07-27

### 新增

- Tokenless 现在通过替换原有 Model 可见内容来压缩 Cosh-NG Tool Response（[#1669](https://github.com/alibaba/anolisa/pull/1669)）。
- Tokenless 现在可重写受支持的 Cosh-NG Shell 命令，以生成更紧凑的输出（[#1669](https://github.com/alibaba/anolisa/pull/1669)）。

### 变更

- Shell 环境检查现在只报告当前命令引用的推荐工具（[#1598](https://github.com/alibaba/anolisa/pull/1598)）。
- `tokenless env-check --fix` 现在只安装必需依赖，不会修改可选推荐项（[#1598](https://github.com/alibaba/anolisa/pull/1598)）。
- 自动依赖修复现在会针对认证、网络或权限问题快速失败并提供可操作信息，而不再提示输入 sudo（[#1598](https://github.com/alibaba/anolisa/pull/1598)）。
- Cosh-NG 压缩统计现在记录在 `cosh-ng` Agent 下（[#1669](https://github.com/alibaba/anolisa/pull/1669)）。
- Cosh-NG 压缩现在从 Model Context 中排除仅供显示的内容（[#1669](https://github.com/alibaba/anolisa/pull/1669)）。
- 无法检测版本的 Cosh-NG 运行现在会保持原始 Tool Response 不变（[#1669](https://github.com/alibaba/anolisa/pull/1669)）。
- 压缩结果不够小时，Tokenless 现在会保持 Tool Result 不变（[#1674](https://github.com/alibaba/anolisa/pull/1674)）。
- Tokenless 用户手册现在位于 ANOLISA 中央指南中，而不再随 RPM Package 提供（[#1586](https://github.com/alibaba/anolisa/pull/1586)）。

### 修复

- Claude Code 2.1.121 及更高版本现在会用压缩结果替换原始 Tool Result，避免 Context 重复（[#1674](https://github.com/alibaba/anolisa/pull/1674)、[#1686](https://github.com/alibaba/anolisa/pull/1686)）。
- 较旧或无法检测版本的 Claude Code 现在会原样传递 Tool Result，不再复制压缩后的 Context（[#1674](https://github.com/alibaba/anolisa/pull/1674)、[#1686](https://github.com/alibaba/anolisa/pull/1686)）。
- Claude Code 替换现在会保留内置 Tool Result 格式，包括空字段（[#1674](https://github.com/alibaba/anolisa/pull/1674)、[#1686](https://github.com/alibaba/anolisa/pull/1686)）。
- ANOLISA 现在可以正确识别已打包的 Tokenless 版本（[#1587](https://github.com/alibaba/anolisa/pull/1587)）。

## 0.7.1

- 修复 RPM Tarball，使其排除生成的 `.anolisa/component.toml`，确保 rpmbuild 始终从权威 `.toml.in` Template 重新生成 Adapter Contract；此前签入的过期副本会缺少 claude-code、codex 和 cosh Adapter 声明（关闭 #1470）
- 同步 Adapter Contract：在 `component.toml.in` 中声明所有已交付 Driver（qoder、claude-code、codex、cosh、qwencode），并增加 CI 检查 `check-component-contract` 以保持同步
- 将测试覆盖率从 75% 提高到 90%：为四个 Crate 增加约 170 个单元测试，覆盖压缩边界情况、Stash 往返、Schema Migration、SLS Writer 和 CLI Dispatch
- 强化测试隔离：使用 RAII `TempDbGuard` / `EnvGuard` 替换不安全的环境变量修改，避免测试接触真实的 `~/.tokenless` 状态；在 Makefile 中强制使用 `--test-threads=1`（Rust 2024 的 `set_var` 是 unsafe）

## 0.7.0

- 增加 MCP `tokenless_retrieve` stdio Server（`tokenless mcp serve`），使连接 MCP 的 Agent 可按需恢复截断 Payload；这是 `tokenless retrieve` CLI 的 MCP 对应实现，补齐与 Headroom CCR `headroom_retrieve` 的 Stash MCP 差距
- 完成其余有损路径的可逆压缩（Stash / CCR）覆盖：`ResponseCompressor` 字符串截断、`ResponseCompressor` 深度截断和 `SchemaCompressor` 描述截断现在都由 Stash 支持并使用 `<<tokenless:KEY>>` Marker；写入 Stash 前进行容量检查以避免孤立条目，共享的 `stash_suffix()` Helper 保持 Marker 预算一致
- 为 `compress-schema` 增加 `--no-stash` / `--stash-db` Flag（与 `compress-response` 一致）；Dry Run（`compression_on=false`）会跳过 Stash，确保没有可检索条目时 Marker 不会进入 LLM
- 为 `SqliteStore` 增加惰性 TTL 清理：在 Retrieve 查询前物理删除过期行，避免 Stash DB 无限增长
- 增加实际节省率展示：`StatsSummary::actual_savings_percent(session_total_tokens)`；`format_summary()` / `format_summary_json()` 接受可选的 Session 总量，并输出“Overall Savings vs Total Consumption”区段及新的 JSON 字段（`session_total_tokens`、`actual_savings_tokens`、`actual_savings_percent`）；未提供时保持向后兼容
- 为压缩统计增加 Stash 写入与大小计数器（扩展 `record_compression_stats`）；Retrieve 端的命中/未命中统计延后到出现明确使用场景时实现
- 增加 Qoder Framework Driver（qodercli 安装、settings.json Merge/Prune、`AdapterOps::read_file`、Symlink-safe 原子 `write_file`）；仅允许 Qoder 使用 `adapter_type=plugin`；对伪造 Receipt fail closed，并要求所有受管 Hook 存在
- 将测试覆盖率从 59% 提高到 75%：四个 Crate 新增 100 多个单元测试和 18 个 CLI 集成测试；测试代码通过 `include!()` 移至 `src/tests/`，使代码结构更清晰
- 增加可逆压缩用户手册（`docs/stash-reversible-compression.md`）并更新 README：为 `tokenless-ccr` 增加架构树条目，增加说明 Hash/Marker 输入和 `--no-stash` / `--stash-db` 的 Retrieve 小节，并按场景映射重写“适用场景与预期效果”章节
- 将 Tokenless 文档从 `*_CN.md` 重命名为 `*_zh.md`，增加双向双语链接，并创建 `README.md` 与 `README_zh.md`
- 处理 Adapter Review 发现：信任 Codex Symlink Target 的已打包数据目录 Root；按组件限定 Claude Code Marketplace 并 fail closed；启用前拒绝 Framework/Adapter Type 不匹配
- 消除 rustc 1.94 stable 在现有测试中发现的 Clippy Warning（tokenless-cli 中的 `field_reassign_with_default`，tokenless-stats 中的 `bool_assert_comparison` 和 `default_constructed_unit_structs`）

## 0.6.1

- 将 tool_categories.json 打包进 dist，用于 npm 安装
- 使用 node: Prefix，并在 OpenClaw Plugin 中移除 Shell Subprocess

## 0.6.0

- 在 JSON 输出中增加绝对保存值和 Schema Version
- 在 OpenClaw Plugin 中使用 import.meta.dirname 替代 __dirname
- 为 Qwen Code Extension 增加 Qwencode Adapter
- 修复 RTK pytest 的“No tests collected”回归
- 为 Codex Script 中的 hook_utils Import 增加可信 FHS Fallback Path
- 增加带配置开关的 SLS JSONL 数据收集
- 增加 Tokenless RPM Component Contract（发布元数据）
- 增加带 Dry-run 比较模式的压缩开关（`TOKENLESS_COMPRESSION_ENABLED`、`stats summary --compare`）
- 默认启用 SLS 记录并补充使用文档
- 对齐压缩模式的 Serde/DB 形式并去重配置加载
- 扩展 RPM Component Contract（bundle.entry + hermes）
- 将 SLS Writer 改为仅追加，并在日志文件不存在时跳过
- Qwencode Hook 优先使用 tool_call_id，而不是内部 tool_use_id
- 将 Vendored RTK 升级至 v0.43.0；针对重构后的 Runner 重做 pytest stderr 输出 Patch；删除 grep-fallback-fix（上游已修复根因）和 preflight-skip-python（上游已撤销）
- 将 Makefile 和 Spec 中的 toon-format 同步到 0.5.0（此前仍为 0.4.6）

## 0.5.1

- 为 `stats summary` 增加 `--json` 输出
- 实现统一 Tool 分类和三层压缩策略
- 增加 RTK grep Fallback Pattern 修复 Patch
- 增加 RTK pytest Error Report Patch

## 0.5.0

- 增加 Hermes Adapter Runner
- 移除 TOON Wrapper Prefix 并精简诊断 Tag
- 在各 Adapter 中统一 RTK Rewrite Exit Code 3 的处理
- 保护 env-fix 和 Hook 中的 Shell 变量插值
- 增加 Subprocess Return Code 检查，并提取共享 Hook Utility
- 保护 resolveBinaryPath 并改进 Binary Cache 失效逻辑
- 在测试中使用 mktemp 和安全的 Home 展开
- 限制 `SchemaCompressor` 递归深度，防止 Stack Overflow
- 传播 env-fix Subprocess Failure，不再只返回 stdout
- 基于 getpwuid_r 进行 Home 查询，并对候选 Binary 执行 Trust Check
- 使用 UID Trust Check 强化 env-fix 安装路径，并将 stderr 转存到日志
- Stats Recorder 从 Poisoned Mutex 恢复，不再直接失败
- 增加输入大小限制并校验 DB Path
- 在 Response Compressor 中预留 Truncation Marker 长度
- 将 OpenClaw Plugin Name 重命名为 Tokenless，ID 重命名为 tokenless
- 增加 Qoder CLI Adapter
- 支持对 Array Input 执行 compress-schema
- 压缩被跳过时发出警告
- 增加 Stats Command Syntax
- 增加 Claude Code Adapter Plugin
- TTY stdin 输入时返回错误，不再挂起
- 增加 Codex Adapter Plugin
- 修复压缩 Pipeline 的输出膨胀、截断和 Hook Timeout
- 强化 env-fix、版本提取、文件信任、Schema 和权限
- 处理 Review 发现：Trailing Newline、chmod Guard、Rate-limited Log 和 Comment
- 使 skip-tools 条目可进行环境归因
- 增加 selective-claw Context Engine Plugin
- 处理 selective-claw Plugin Review 发现
- 从 selective-claw 中移除无效的 `2` 依赖
- 恢复 compress_response_hook.py 中的缩进
- 强化 Hook Exit Code 处理和 Trust Model 一致性
- 只对真正异常的 RTK Exit Code 发出警告
- 去重 rewrite_hook，并从 hook_utils 导入

## 0.4.1

- 修复 env_check.rs 中 version_ge 的三段版本截断问题（比较所有分段）
- 增加 Qoder、Claude Code、Codex Adapter Plugin 和文档
- 将 manifest.json 与 Template 同步，以包含全部六个 Agent
- 更新 README 和用户手册，记录新的 Agent Integration
- 将 __pycache__ 加入根 `.gitignore`
- 更新 response-compression.md，记录全部 Agent Integration Path
- 从 Cargo.toml 派生 Makefile 版本，并修复 Spec Changelog 的星期
- 将 Adapter 版本号统一为 0.4.0
- 从 Cargo.toml 派生 Adapter Plugin 版本，不再硬编码

## 0.4.0

- 修复 Stats、命名、SQL、路径和权限中的 5 个 Bug
- 对齐 FHS 路径、重构 Adapter 目录并移除 install.sh
- 处理 Schema、env-check、Hook 和 Plugin 中的 Code Review 发现
- 增加 Hermes Agent Plugin
- 强化安全性并修复关键算法正确性
- 修复行为正确性和逻辑问题
- 去重、删除 Dead Code 并进行表面清理
- 支持 Staged Install
- 支持 Debian/Ubuntu FHS 路径并强化 Binary Resolution
- 将 OpenClaw Plugin 构建到 dist/index.js

## 0.3.2

- 使用 libc::getuid() Syscall 替换可伪造的 Home Directory UID 推导，保证 Trust Chain 完整性
- 使用进程内 toon_format::encode_default() Library Call 替换 Subprocess `toon -e` 调用
- 使用 crates.io Dependency 和内联 toon-format Source 替换 RTK/TOON Git Submodule
- justfile 的 setup-rtk Recipe 在 RTK Stats Patch 失败时 Hard Fail
- 统一 compress-toon、compress-schema 和 compress-response 的 Error Exit Code（均为 2）
- 从 Makefile 的 toon 安装中移除 `2>/dev/null || true`（Binary 缺失时 Hard Fail）
- 删除已具有 `#[from]` 的 thiserror Variant 上多余的 `#[source]` Attribute
- 将 Python Hook 的 FHS Path Constant 去重到共享 hook_utils Module
- 将 libc 加入 Workspace Dependency，用于 UID Syscall
- 在 spec.in 中增加 rust >= 1.89 的详细注释，说明 CI Pin 的原因

## 0.3.0

- 增加与 Cosh Extension 集成的 Tool-ready 四阶段环境预检查
- 无 Token 节省时跳过压缩和统计
- 通过 `.rewrite-context` 文件向 RTK Stats 传递 Caller Context
- 从 install.sh 中移除多余的 Cosh Extension 安装/卸载
- 按 Cosh 开发指南将 Cosh Hook 转换为 Extension 格式
- 跳过零压缩和统计记录
- 在 OpenClaw Plugin 中使用 isExecutable() 和解析后的路径
- 为 RPM 安装的 Plugin 解析 RTK/TOON Binary Path
- 修正 RPM 安装路径，使其符合 install.sh 预期
- 在 TOON Encoding 中保留 Tool Result Message 结构
- 将安装路径与 FHS 对齐
- 使用 Hook Payload 中真实的 tool_use_id 自动记录 Stats
- 重构 RPM 目录并移除自动 Plugin/Hook 安装

## 0.2.0

- 增加基于真实数据自动记录的压缩统计
- 增加 TOON Context 压缩支持
- 对 Skill 和内容检索 Tool 跳过压缩

## 0.1.0

- 将 Tokenless 引入 ANOLISA（#199）

[未发布]: https://github.com/alibaba/anolisa/compare/tokenless/v0.7.5...HEAD
[0.7.5]: https://github.com/alibaba/anolisa/compare/tokenless/v0.7.4...tokenless/v0.7.5
[0.7.4]: https://github.com/alibaba/anolisa/compare/tokenless/v0.7.3...tokenless/v0.7.4
[0.7.3]: https://github.com/alibaba/anolisa/compare/tokenless/v0.7.2...tokenless/v0.7.3
[0.7.2]: https://github.com/alibaba/anolisa/compare/tokenless/v0.7.1...tokenless/v0.7.2
