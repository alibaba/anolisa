# 更新日志

本文件记录 cosh-ng 项目的所有 notable changes。

格式基于 [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)，并遵循 [Semantic Versioning](https://semver.org/spec/v2.0.0.html)。

## [未发布]

## [0.17.2] — 2026-08-19

### 修复
- 将 `run_command` 输出上限设为 32 MB，防止 `dd if=/dev/zero`、`yes` 等失控命令导致进程被 OOM 杀死；超出上限时杀死进程组并返回 `OutputTooLarge` 错误，而非无限增长 (#2405)
- 使 RPM `%post` 的 /etc/shells 注册幂等（先规范化并探测再追加），并新增 `%preun` 脚本：当用户仍以 `/usr/bin/cosh` 为登录 shell 时擦除操作失败关闭，防止 `rpm -e cosh-ng` 后遗留悬空 shell (#2599)

## [0.17.1] — 2026-08-18

### 修复
- 通过事件驱动改造 raw relay 与 SIGINT 路径，将交互回显延迟从 p50 20 毫秒降至亚毫秒级，并消除长时间会话的高频空闲 CPU 轮询 (#2622)
- 在 best-effort 审计存储写入失败时，仍保持已允许命令正常执行 (#2631)
- 允许已审批流水线中的单引号参数包含换行，例如多行 jq 或 awk 脚本 (#2638)

## [0.17.0] — 2026-08-17

### 新功能
- 当 hook id 同时存在于 shell 层与 agent 层时，`/hooks enable|disable <id>` 弹出交互式消歧面板，可选择切换 Shell hook、Agent hook 或两者 (#2400)

### 变更
- 空闲轮询时跳过冗余的历史事件处理，长时间运行的交互会话不再在空闲时重复执行与历史事件规模相当的工作 (#2546)

### 修复
- 修复 hook 输出显式 `"decision": null` 被当作透传的问题：现视为无效 hook 输出并默认拦截对应工具调用；若 hook 配置 `fail_open = true` 则放行但记录 `hook_failure` 通知；输出 `{}` 仍为合法透传 (#2529)
- 修复 dnf 系统上 `cosh pkg install/remove --dry-run` 将可安装/可移除软件包的成功模拟误报为后端错误的问题 (#2605)

## [0.16.1] — 2026-08-14

### 修复
- 优化 CJK（中日韩）文本换行，充分利用终端行宽并保持标点与相邻文字相连 (#2446)
- 在保存 `/auth` 认证配置前先验证凭据、端点和模型，失败时保持认证面板打开并定位到问题字段 (#2458)
- 允许扩展 hook 在成功但无输出时通过，避免空输出被错误拒绝 (#2506)

## [0.16.0] — 2026-08-13

### 新功能
- 新增 raw packaging 接口，支持确定性归档构建、跨平台二进制验证和可移植 macOS launcher (#2411)

### 变更
- 收敛 cosh-core 与 cosh-shell 运行时路径，统一 Claude 和 Qwen provider driver，新增显式 shell/core protocol v1 协商与旧版回退 (#2403)
- 移除已废弃的 `/draft` 斜杠命令别名，统一使用 `/agent` (#2441)

### 修复
- 使用单调时钟计算 input-wait 超时，避免时钟偏移导致卡顿 (#2176)
- 修复 panel-family hint card 渲染问题 (#2196)
- 按 SSE 规范解码事件，对格式错误的输入立即报错 (#2209)
- 在 core 中约束并保护敏感文件写入 (#2211, #2378)
- 在 core 和 shell 中透传原始 prompt 文本，不做归一化 (#2256)
- 修复 Enter 键触发 review marker 的问题 (#2274)
- 收窄交互式取消检测范围，并在关联拦截后配对 ledger 完成 (#2352, #2353)
- drop 时向 stdout 发送 raw-mode 禁用指令，防止终端状态泄漏 (#2357)
- 加固 core 和 shell 中的 hooks (#2359)
- 避免使用可预测的临时文件路径 (#2361)
- 在行截断分支中强制字节上限 (#2370)
- 列出所有斜杠提示匹配项，而非仅返回首个 (#2410)
- 将缺失的失败退出码映射为 -1 哨兵值，保证错误报告一致性 (#2412)
- 在 core 中暴露 runtime context，确保状态访问一致 (#2428)

## [0.15.0] — 2026-08-09

### 修复
- 恢复 shell 中 inline hint 之后的提示光标 (#2172)
- 支持 ID_LIKE 回退用于 OS 发行版检测 (#2200)
- 在 `/help` 输出中列出 hook 命令 (#2208)
- 跳过服务生命周期操作的审计日志 (#2213)
- 支持 core 中的 macOS 特定文件读取 (#2220)
- 序列化 handoff 状态以防止竞态条件 (#2226)
- 归一化 apt search glob 模式 (#2227)
- 在 core 中恢复 compaction 后的上下文预算 (#2244)
- 在 shell 中去重 hook 通知 (#2259)

## [0.14.0] — 2026-08-04

### 新功能
- `/status`、`/about` 和 `/stats` 斜杠命令用于运行时自省 (#1778)
- `/mcp` 斜杠命令用于 MCP server 管理 (#1949)
- `/session list --all` 枚举跨 workspace 的会话 (#2139)
- DashScope prompt cache 支持，降低 token 成本 (#2046)
- `cached_tokens` 可观测性，用于缓存命中诊断 (#2075)
- OpenAI provider 中按模型动态设置 `max_tokens` (#2165)
- 在 MCP client 初始化时声明 `roots` 能力 (#2007)
- core 中的 hook 工具和环境支持 (#1894)
- 在 core 和 shell 中呈现工具参数状态并限制重试次数 (#1925)
- shell 中自动执行完全只读的复合命令 (#1959)
- 在 core 和 shell 中扩展有上限的回合运行 (#2035)
- 改进 core 和 shell 的认证菜单 (#2062)
- 在 shell 中限制 agent handoff 的输入等待 (#2168)

### 变更
- shell 中终端无关的多行 prompt 输入 (#1947)

### 修复
- Shell handoff 和 hooks：保留 LLM 输入、丢弃过期 handoff 文本、在 provider 会话内恢复 handoff 回退、脱敏 handoff 证据、通过一次性 claim token 关闭秘密脱敏的 handoff、收敛 Han NL 输入归属、将敏感 NL 路由到 agent、并为 send-to-shell 运行 project hooks (#1955, #2010, #2055, #2074, #2130, #2137, #2151, #2154)
- 审批生命周期：通过 lifecycle ledger 和兜底超时保证终态、重置 auth 输入、在 trust 模式下呈现 sandbox-bypass 审批、并拒绝零空闲超时 (#1934, #1939, #1968, #2116)
- 认证流程：ESC 时回退 `/auth`、在 `/help` 中列出 `/auth`、并在无认证启动时提示 `/auth` (#1891, #1906, #2166)
- 斜杠命令和 prompt 输入：防止拦截时斜杠回显重复、Up 键召回斜杠命令、拦截含斜杠的 NL prompt、支持 NL prompt 中的软换行、保持 card submit 的 type-ahead、归一化 CSI-u 退格、并按路由键 ghost 归属 (#1868, #1899, #1911, #1922, #1942, #1993, #2167)
- Shell 渲染：阻止 extdebug 泄漏到 prompt hooks、高亮代码块语法、用用户语言回复、精简技能列表、并禁用隐式分页 (#1849, #1904, #1910, #1921, #1998)
- 命令风险和安全：评估所有复合命令段的风险、门控不可恢复的系统控制命令、并分类解释器风险 (#1905, #2081, #2119)
- 文件 IO 加固：拒绝占位写入、使文件写入原子化、限制并约束读取工具、在 drop-write 前恢复阻塞、并将 fd-dup 重定向视为非写入 (#1918, #2069, #2120, #2121, #2124, #2127)
- Shell 恢复和漂移：防止恢复风暴并使用 zsh preexec `$3` 处理漂移 (#2072, #2073)
- 审计日志：展示 hook 上下文 (#2082)
- Core 运行时：保留工具参数、对齐 compaction、展示真实 session prompt、emit 错误时 fail closed、并使截断 UTF-8 安全 (#1844, #1847, #2003, #2005, #2118)
- 类型、wire 和打包：修复 wire 错误、移除跨 workspace dev-dep、对齐 RPM 身份、对齐 bundle 健康检查、并在 skill tool 中添加 base-dir 提示 (#1514, #1933, #1937, #1984, #2140)

## [0.13.0] — 2026-07-26

### 新功能
- 通过 `/session`、`/resume` 和 `--resume` 启动选项实现交互式会话恢复 (#1546, #1592)
- 带 schema 版本控制和旧会话迁移的 workspace 级会话持久化 (#1546, #1592)
- MCP 工具支持，用于扩展 agent 能力 (#1530)
- 上下文相关的 shell insight 交互 (#1537)
- 跨 core 和 shell 层的秘密脱敏 (#1555)
- 通过 `cosh doctor` 导出诊断 bundle (#1576, #1597)
- core 和 shell 中的扩展平台 (#1583)
- 个性化 prompt 推荐 (#1606)
- 会话压缩以管理持久化增长 (#1668)
- PostToolUse 响应替换和 hook 适配 (#1669)
- 启动建议门控 (#1671)
- 跨 core 和 shell 的审计日志 (#1679)
- 通过斜杠命令改进和 SysOM `/auth` 快捷方式改善会话体验 (#1726, #1813)
- 将未解析的自然语言输入路由到 Agent (#1742)
- ESC 取消活跃的 agent 运行 (#1761)
- 回合级批量审批同意 (#1825)

### 变更
- 回退持久化的凭据加密 (#1748)
- 对齐任务 scope 文档 (#1445)
- 稳定 core 和 shell 测试门控 (#1699)
- 加速 raw CLI 测试和 fake stream 节奏 (#1797)
- 将 cosh-shell 测试基线跟踪至 2469 (#1827)

### 修复
- 会话命令解析、信号退出、斜杠参数处理和 prompt ghost ESC (#1632, #1634, #1636, #1663, #1724, #1843)
- Agent 提问交互、建议控制、推荐 scope 和 tab 重绘 (#1725, #1741, #1749, #1758, #1821)
- 审批 card 布局、阻塞标题对齐和空回车处理 (#1786, #1788, #1838)
- 认证和信任加固：验证 provider、加固认证、保留 trust 块、加密凭据并扩展路径 (#1627, #1673, #1701, #1722, #1777, #1784, #1791, #1809, #1816, #1841)
- 审计日志修复：kill 树、拆分复合命令、脱敏秘密、scope claim 并保留导出路径 (#1611, #1613, #1635, #1765, #1772, #1840, #1842)
- Core 运行时稳定性：JSONL 验证、工具选择错误、流式状态、ai-like 表、修订时钟、free-text 清除、布局门控和 SysOM 终结符 (#1599, #1661, #1689, #1730, #1731, #1799, #1800, #1803, #1839)
- 平台和 CLI 正确性：遵循 dry-run、处理跳过的 checkpoint、允许搜索模式、修复 package dry-run/搜索结果、遵循 cargo config、同步 Bash HISTFILE、验证 workspace 路径、将 --help 路由到 stdout、添加 skill 参数检查、对脱敏写入发出警告、跳过 DEBUG trap、恢复 utility 工具、阻止 BASHOPTS extdebug 泄漏、处理 null 重定向、保持 provider handoff 存活、更新 DashScope URL、raw action relay 看门狗、恢复启动健康行、隐藏 receipt audit ref 并更新测试基线 (#1426, #1440, #1633, #1637, #1642, #1646, #1672, #1675, #1676, #1710, #1719, #1733, #1783, #1787, #1790, #1795, #1808, #1812, #1818, #1820, #1845)

## [0.12.0] — 2026-07-12

### 新功能
- 将认证归属移入 cosh-core，采用隔离的配置层 (f028ad90)

### 变更
- 在统一运行时模块下整合日志 (db86b3dd)
- 将组件文档迁移到 user-guide/developer-guide 并添加 cosh-ng 文档 (317d3f26, adf63ac2)
- 将 `*_CN.md` 文档重命名为 `*_zh.md` 并修复交叉引用 (82f8dab4)

### 修复
- 在 platform 和 cli 中遵循 svc dry-run (4c593050)
- 保留手动 aliyun 回退和旧版 STS 认证 (924dd76b, ee1dd179)
- 保护 auth provider 编辑；优先 aliyun 认证选项 (f0c97efa, 904655fb)
- 限制 host 执行的 shell 预览 (a6da7301)
- 路由非交互式 cosh launcher 调用；支持 raw 命令透传 (ecb56739, 1490eb3c)
- 绑定启动 prompt 和 agent 请求上下文 (14d6336f, 25d9d28f)
- 在 shell 中拥有 prompt 边界 (#1310)
- 避免循环检测中的 UTF-8 分割 (ef7f5147)
- 移除 provider 可见的 skill 提示；引导诊断 skill 使用 (a6024873, 7f695178)
- 丢弃冗余的 format 借用；满足 clippy 诊断 (5e14a686, 063217f1)
- 稳定 CI、raw-cli、PTY 和服务测试 (d573796d, 3563a5ab, 0fb34ea6, fc28da5f, ad138d45, 65421d25, 5b17b892, 3e43ba6e, 707bc3c0)

## [0.11.0] — 2026-06-28

### 新功能
- 阿里云认证 provider，支持 ECS 自动检测、STS 凭据和二维码流程
- SysOM 阿里云 provider，使用 ACS3 签名进行 LLM API 访问
- 每回合 SLS JSONL 日志用于可观测性
- SysOM 请求来源标识头
- 跨所有 crate 的结构化 tracing 日志系统
- PostToolUseFailure hook 事件上的 sandbox bypass 审批流程
- 启动健康扫描用于环境诊断
- 扩展/hook/skill 启用/禁用命令（`/extensions`、`/hooks`、`/skills`）
- 统一组件状态管理模块
- 专用 HOOK 审批面板，简化 UI
- UserPromptSubmit hook，带 Ask 审批强制
- cosh-core 中的 shell evidence 读取准入控制
- cosh-shell 中的工具活动渲染
- 启动横幅中的 `cosh-switch` 提示，用于在 cosh-ng 和 copilot-shell 之间切换

### 变更
- **BREAKING**：CLI 二进制从 `cosh` 重命名为 `cosh-cli`；移除 dispatch_core
- RPM spec：安装 cosh-cli 二进制、`/usr/bin/cosh` launcher、cosh-switch 脚本、`Conflicts: copilot-shell`
- 用结构化 tracing 宏替换 eprintln
- 用 fold_decision 统一 hook 决策聚合
- 将 workspace 仓库 URL 更新为 github.com/alibaba/anolisa

### 修复
- 认证 ECS 流程和阶段转换时的面板重叠
- 认证二维码渲染不含 ANSI 转义码
- 认证成功后使用 SysomProvider 处理 aliyun
- 对齐 hook 输入字段和 AfterModel/wrap_tool_response 与 copilot-shell 协议
- tool_result 去重守卫和可见性
- 在 shell handoff 前恢复 prompt
- 抑制重复的 evidence 读取
- 减少失败命令自动分析噪声
- Skill 存在性检查和加固审批匹配
- 解决 cosh-core 和 cosh-shell 中的 clippy 警告

## [0.10.0] — 2026-06-23

### 新功能
- Shell evidence 协议，用于捕获和重放命令执行上下文
- cosh-core 中的 shell evidence 控制工具，用于 evidence 生命周期管理
- 与 copilot-shell 对齐的 hook 协议，实现零变更扩展支持
- 通过通知协议传播每 hook 决策
- cosh-shell 中按 hook 决策着色渲染 hook 警告

### 变更
- 跨 cosh-shell 模块共享 agent 错误显示文本

### 测试
- 覆盖 shell evidence raw CLI 流程

## [0.9.0] — 2026-06-22

### 新功能
- Hook 通知集成到审批面板，带 ⚠ 警告显示
- Hook ask 决策即使在 Trust/Auto 模式下也强制用户审批
- 扩展 hook 系统，带 tool_use_id 关联和新事件类型
- 用于 /extensions /skills /hooks 斜杠命令的注册表协议

### 修复
- 在 /help 输出中显示 Registry 组

### 变更
- 移除无用的 skill 管理代码

## [0.8.0] — 2026-06-18

### 变更
- 将 `cosh-tui` crate 和二进制重命名为 `cosh-core`，覆盖整个 workspace
- 更新 adapter 系统：`CoshTuiAdapter` → `CoshCoreAdapter`、`AdapterKind::CoshTui` → `CoshCore`
- 更新环境变量 `COSH_TUI_PATH` → `COSH_CORE_PATH`
- 更新 RPM spec、文档和所有测试 fixture

### 修复
- 在流式 card 中中和 agent 状态文本
- 对齐 cosh-shell 中的流式 card 宽度

## [0.7.0] — 2026-06-17

### 新功能
- 扩展发现和加载模块，支持 `cosh-extension.json` manifest
- 扩展 hooks 集成到启动生命周期
- Skill 模块，支持多级加载（内置、用户、项目）和热重载
- SkillManager 集成到工具注册表和启动流程
- 可用技能注入系统 prompt 供 LLM 发现

### 修复
- `expand_env_vars` 在环境变量未定义时的无限循环
- 对不支持的扩展 hook 事件（`PostToolUseFailure`、`BeforeModel`、`AfterModel`）发出警告而非静默丢弃
- 对齐扩展 hooks 格式与 copilot-shell 的嵌套组结构
- 从 skill tool schema 中移除未使用的 `args` 参数，避免误导 LLM
- 在 cosh-shell 中展示 question free text 答案
- 加固前台 shell handoff
- 共享 copilot shell 配置路径并保留旧版配置回退

### 变更
- 归一化 cosh-shell 配置键
- 标准化 cosh-shell 代码和测试组织
- 将用户状态移至 copilot shell scope 下

## [0.6.0] — 2026-06-16

### 新功能
- cosh-tui 中的 P0 hook 系统，含 5 个生命周期事件（`on_session_start`、`on_turn_start`、`on_turn_end`、`on_tool_call`、`on_session_end`）
- cosh-shell 中的 shell 审批分类和 hook 来源跟踪
- 将当前 cosh shell 迁移到 monorepo workspace

### 修复
- 解决 cosh-shell 中的审批 review 发现
- 加固 shell evidence 连续性以防止上下文丢失
- 在 cosh-tui 中归一化工具调用流式协议
- 修复 cosh-shell 中子命令的透传

## [0.5.0] — 2026-06-16

### 新功能
- CoshTuiAdapter 持久进程模式（spawn 一次，跨 agent 运行复用，死亡时自动重启）
- `ask_user` 通过控制协议往返（agent 可以提问内联问题路由到 TUI）

### 变更
- 将 cosh-tui main 拆分为 cli/headless/interactive 模块
- 将二进制从 cosh-tui-core 重命名回 cosh-tui

## [0.4.1] — 2026-06-15

### 新功能
- settings.json → config.toml 自动迁移，含 AES-256-GCM 加密 API key 解密
- JSONL 协议和工具审批集成测试

### 修复
- 在 PROMPT_COMMAND 前置 precmd 以捕获真实退出码（Alibaba Cloud Linux /etc/bashrc 问题）

## [0.4.0] — 2026-06-15

### 新功能
- JSONL wire 协议（InputMessage / OutputMessage），用于 cosh-shell ↔ cosh-tui 通信
- Provider 抽象，支持 OpenAI 兼容流式（DashScope、OpenAI、DeepSeek、Generic profiles）
- 工具执行框架，含 7 个内置工具（shell、read_file、write_file、edit、grep、todo、skill）和审批控制
- 上下文窗口管理、消息截断、循环检测、对话压缩
- 生命周期 hooks 框架
- CoshCore agent 循环引擎
- 基于 TOML 的多 provider 配置，支持环境变量展开

### 变更
- **BREAKING**：二进制接口从 ratatui 交互式 TUI 改为 JSONL stdin/stdout 后端
- **BREAKING**：配置格式从 settings.json 改为 config.toml
- 用单文件 JSON 持久化重写 session store

### 移除
- 旧版基于 ratatui 的 TUI 代码（app、commands、llm、logger、theme、tools、ui 模块）

## [0.3.0] — 2026-06-15

### 新功能
- **cosh-shell crate** — 基于 PTY 的 AI 增强 shell host，含 OSC marker 协议
- Claude、Qwen、Fake AI adapter，支持流式
- 内联渲染引擎（审批、提问、推荐、活动面板）
- 带审批模式的治理层
- 通过信号处理器（SIGTERM/SIGHUP/SIGHUP/SIGQUIT）和 panic hook 实现终端恢复
- 退出码分类，含 8 个类别（Smart/Auto/Manual 分析模式）
- 工具显示引擎，按工具类型解析和 ANSI 颜色分类
- Hook 引擎，含内置 hooks（FailedCommandHook、TestFailureHook）和 skill 路由
- 从 ~/.config/cosh/hooks/ 加载外部 hook，支持子进程执行
- 原生 shell 兼容性（rcfile 加载、PS1、history、login shell 检测）
- 上下文窗口，带滑动窗口（最大命令数、最大年龄、token 预算）
- Prompt 意图优化（do → Bash 工具、know → 散文）
- 自然语言拦截，带视觉反馈
- InputClassifier 保守模式用于原生模式
- 分析节流（30s 冷却，最多 3 次连续）
- 咨询 card 渲染，带键盘捕获
- 用于工具审批往返的控制协议
- 启动横幅，带渐变 ASCII art logo
- `/mode` 和 `/hooks` 斜杠命令
- 架构文档

### 修复
- 原生模式中 powerlevel10k 双行 prompt 的输入渲染
- 原生模式中通过 buffered-then-judge 策略的斜杠/NL 拦截
- command_not_found 的 zsh preexec 拦截
- CJK 输入和退格的 CandidateRedraw 行清除
- 原生模式中抑制 cosh-osc$ prompt 泄漏
- bash 工具执行器中的工具显示标签匹配
- buffer 提取中的宽字符占位单元格处理

### 变更
- 统一 workspace 版本（0.3.0）适用于所有 crate（cosh-types、cosh-platform、cosh-cli、cosh-shell、cosh-tui）

## [0.2.0] — 2026-05-16

加固 + 审计子系统发布。Workspace 版本与发布配置和 lockfile 提交一起升级到 `0.2.0`。

### 新功能

- **`audit` 子系统**，含 PEP/PDP/log 分离：`cosh audit check` / `cosh audit log` 用于命令安全门控和每会话检索。
- **Workspace 发布配置**（`opt-level = 3`、`lto = true`、`strip = true`、`codegen-units = 1`），提交 `Cargo.lock`，workspace 级依赖锁定，以及原生 CA 证书支持。
- **命令超时、输入验证和 panic 安全的 JSON 输出**，覆盖 `cosh-cli` 和 `cosh-platform`，使 panic 仍在 stderr 发出 `CoshResponse` 信封而非空退出。
- **`forbid(unsafe_code)`** 作用于 `cosh-cli` / `cosh-platform`，以及 `svc list --state` 过滤验证使用 allow-list。
- **`pkg search` 交叉引用安装状态**，使结果显示哪些匹配已安装。
- **`ResponseMeta.warning`** 字段用于非致命警告；`audit` 响应通过此字段显式标记为 stub。
- **cosh-tui 中的 LLM 工具面扩展**：pkg / svc / checkpoint 包装工具，以及 `svc enable` / `svc disable --dry-run`。
- **cosh-tui 中 LLM 和外部命令工具的超时 + 指数退避重试**；60s shell 工具超时。

### 变更

- TUI `/help` 与完整命令集对齐；标题栏版本和 markdown 前缀剥离修正。
- 跨 workspace 解决 clippy 警告；移除 dead-code allow；测试代码与生产 lint 级别对齐。
- 消除构建警告并改进 cosh-tui / cosh-platform 的版本检测。

### 修复

- **Shell 安全检查标记化**，关闭 tab/换行/重定向/链绕过；对原始命令字符串的子串匹配替换为空白字符（含 `\t`/`\n`/`\r`）标记化和元字符拒绝（`;` `|` `&` `>` `<` `$` `` ` `` `(` `)` `{` `}`）— `is_safe_command` in `crates/cosh-tui/src/tools/shell.rs`。
- 即使在 Yolo 审批模式下也禁止工具调用。
- `cosh-cli` 包装工具输出有界，防止嘈杂子命令撑爆 LLM 上下文窗口。
- 通过进程级计数器合成 tool-call ID 以保证跨 agentic loop 唯一。
- `settings.json` 和 session 文件以 `0600` 权限原子写入。
- 对 agentic loop、history、config 和 tool messages 强制运行时边界；scrollback 有界且截断 UTF-8 安全。
- TUI 中安装 panic hook；panic 后恢复 history 导航。
- ws-ckpt IPC 响应大小限制为 64 MiB。
- 通过 `LoadState=not-found` 检测不存在的 systemd 服务，而非错误归类为 "inactive"。

### 安全

- Audit-stub `recoverable` / `hint` 语义通过标准 `CoshError` 信封清晰呈现给 agent。
- 凭据文件的原子重命名 + `0600` 权限。

## [0.1.0] — 2026-05-10

将 workspace 从 `agos-core` 重命名为 `cosh-ng` 并添加交互式 TUI crate 后的初始公开形态发布。

### 新功能

- **4-crate workspace**：`cosh-types`、`cosh-platform`、`cosh-cli`、`cosh-tui`，严格依赖方向 `cosh-cli` / `cosh-tui` → `cosh-platform` → `cosh-types`。
- **`cosh` CLI 二进制**，双模式分发：`cosh`（无参数）exec 进入 `cosh-tui`，`cosh <subsystem> <action>` 返回结构化 JSON。
- **跨发行版 `pkg` 子系统**：`install` / `remove` / `search` / `list`，基于 `Distro::detect()` 读取 `/etc/os-release` 在 `dnf` / `apt-get`（`apt-cache` 用于 search）/ `zypper` 之间路由。
- **`svc` 子系统**，基于 `systemctl`：`status` / `start` / `stop` / `restart` / `enable` / `disable` / `list`，含运行时间和 `list` 中正确的列映射。
- **`checkpoint` 子系统**，通过 Unix-socket IPC 与 `ws-ckpt` daemon 通信；bincode wire 格式带 4 字节 LE 长度前缀和显式协议版本控制 + 错误处理。命令：`init` / `create` / `list` / `restore` / `recover` / `delete` / `diff` / `cleanup` / `status`。
- **`cosh-tui`** 基于 `ratatui` + `crossterm` 的交互式 TUI：斜杠命令系统含自动补全、会话管理、主题、自定义边框集、echo-on-submit。
- **Agentic loop 含 cosh-cli 包装工具**，在 cosh-tui 中将 pkg / svc / checkpoint 工具带到 LLM（初始发布为 `cosh-tui v0.4.0`）。
- **LLM 聊天集成**，含配置驱动的 provider 和 UI 呈现。
- **统一 `settings.json` V2 配置**，整合先前分散的配置文件。
- **AES-256-GCM 解密**，用于加密凭据。
- **macOS 检测 + Homebrew 后端**，在 `cosh-platform` 中，含单元测试。
- **统一 JSON 信封** `CoshResponse<T>`，含 `ok` / `data` / `error` / `meta`，分类的 `CoshError` 携带 `recoverable` 和 `hint` 用于 agent 重试决策。
- **`pkg` 和 `checkpoint` CLI 命令的集成测试**。

### 变更

- Workspace 从 `agos-core`（含 `agos-types` / `agos-platform` / `agos-cli`）重命名为 `cosh-ng`（含 `cosh-*` crate）；`agos-cli` 和 `agos-platform` 在同一 commit 中移除。
- `cosh-tui` checkpoint 工具适配新 daemon 协议。

### 修复

- `cosh-cli` stdout 验证为 JSON 后再转发给 LLM，防止格式错误字节导致解析器混乱。

## [pre-0.1.0] — 2026-05-03 → 2026-05-08

重命名前 `agos-core` 基础。

### 新功能

- 初始 2-crate workspace `agos-types` + `agos-platform`。
- `agos-cli` 跨发行版 CLI 原型，含 `pkg`、`svc`、`checkpoint`、`audit` 命令形态。
- MVP v2 CLI Gateway 架构文档和双语（英文/中文）使用指南。
