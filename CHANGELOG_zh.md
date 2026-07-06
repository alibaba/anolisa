# 变更日志

[English](CHANGELOG.md)

本文件记录项目所有值得注意的变更。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，项目遵循[语义化版本](https://semver.org/lang/zh-CN/spec/v2.0.0.html)。

## [1.0] - 2026-07-06

### 组件版本

| 组件 | 版本 |
|------|------|
| copilot-shell | 2.6.1 |
| agent-sec-core | 0.7.0 |
| agentsight | 0.7.1 |
| tokenless | 0.6.1 |
| agent-memory | 0.2.1 |
| os-skills | 0.6.1 |
| anolisa | 0.1.20 |
| skillfs | 0.3.2 |
| ws-ckpt | 0.4.1 |
| cosh-ng | 0.11.0 |

### 重点特性

- **anolisa**：更新到 v0.1.20，交付统一 CLI 网关提供组件全生命周期管理与适配器编排，用户可通过一条命令安装/更新/诊断所有组件
- **cosh-ng**：更新到 v0.11.0，完成 Core/Shell 分离与 AI 增强终端，Agent 可跨发行版确定性执行结构化系统操作
- **agent-memory**：更新到 v0.2.1，新增用户数据主权与 4 类记忆分类，用户可查询/遗忘/控制自动捕获的记忆
- **tokenless**：更新到 v0.6.1，新增压缩开关与 A/B 对比及 QwenCode 适配器，用户可量化各策略的 Token 节省效果而不影响任务执行

### 新增组件

- **anolisa**：首次发布 v0.1.16，构建统一 CLI 网关管理组件安装/更新/卸载（RPM + Raw 双后端），用户可通过 `anolisa install --all` 一键部署全部组件
- **cosh-ng**：首次发布 v0.11.0，实现确定性 Agent-OS 接口（5 crate workspace），Agent 可通过稳定 API 跨发行版执行结构化系统操作
- **skillfs**：首次发布 v0.3.2，构建 FUSE 虚拟文件系统实现基于视图的 SKILL.md 暴露，Agent 可从挂载目录发现并加载技能

### 组件更新

- **agent-memory**：更新到 v0.2.1，新增主权工具集（about/forget/consent）、AMA 导入导出、4 类分类和抗 SIGKILL 增量聚合，用户可自主控制记忆留存并跨 Agent 迁移
- **tokenless**：更新到 v0.6.1，新增压缩开关（dry-run + 按模式统计）、SLS JSONL 遥测默认开启和 QwenCode 适配器，开发者可 A/B 测试压缩策略并在 SLS 大盘监控 Token 节省
- **agentsight**：更新到 v0.7.1，新增 Token 节省可视化（策略饼图 + 行级 diff）、安全大盘和容器/K8s 全面支持，用户可直观评估各优化策略的节省贡献
- **copilot-shell**：更新到 v2.6.1，新增 `/model` 多 Provider 切换对话框和 SLS 会话遥测（32 字段 JSONL），用户可自由切换 LLM Provider 而不丢失配置
- **agent-sec-core**：更新到 v0.7.0，新增 Skill Ledger 完整性链（GPG 签名工作流）和 Prompt Scanner，用户可审计 Skill 安全状态并在危险操作前收到确认提示
- **os-skills**：更新到 v0.6.1，新增 ANOLISA Guide 知识库 skill（13 份官方文档）和 OpenClaw 安装预检引导，Agent 可在回答中引用准确的产品文档
- **ws-ckpt**：更新到 v0.4.1，新增自动清理调度和 TOML 配置热重载，用户可设置保留策略并即时生效无需重启 daemon

### 变更

- 文档治理规范通过 `specs/documentation-standard.md` 建立
- 双语命名约定统一为 `_zh.md`（从遗留 `_CN.md` 迁移）

## [0.6] - 2026-06-12

### 组件版本

| 组件 | 版本 |
|------|------|
| copilot-shell | 2.4.1 |
| agent-sec-core | 0.5.0 |
| agentsight | 0.5.0 |
| tokenless | 0.4.1 |
| agent-memory | 0.1.0 |
| os-skills | 0.5.0 |
| cosh-ng | 0.1.0 (MVP) |

### 重点特性

- **agent-memory**：首次发布 v0.1.0，交付沙箱化文件系统 MCP 记忆服务器，Agent 可跨会话持久化存储并通过 BM25 检索上下文
- **tokenless**：更新到 v0.4.1，新增 Hermes Agent 插件和 Tool Ready 4 阶段预检，Agent 工具执行前自动验证环境就绪避免无效重试
- **agentsight**：更新到 v0.5.0，新增 Skill 维度 Token 指标和 Hermes 支持，用户可精确定位哪些 Skill 消耗最多 Token

### 新增组件

- **agent-memory**：首次发布 v0.1.0，构建 19 工具 MCP 服务器（命名空间隔离 + BM25 后台索引），Agent 可在沙箱化文件系统中读写/检索持久记忆
- **cosh-ng**：首次发布（MVP），完成确定性 OS 操作的生产可用功能，Agent 可获得格式可预测的结构化命令输出

### 组件更新

- **tokenless**：更新到 v0.4.1，新增 Hermes adapter runner 和 Tool Ready 机制（4 阶段环境预检集成为 cosh extension），Agent 工具调用前自动校验环境减少因环境故障浪费的 Token
- **agentsight**：更新到 v0.5.0，新增 Skill 维度 Token/调用指标和 Hermes matcher（含 SSL 支持），用户可在 Dashboard 中按 Skill 查看 Token 消耗明细
- **agent-sec-core**：更新到 v0.5.0，新增 PIIChecker（输出 PII 检测 + 脱敏引擎）和 Skill Scanner（文本/代码扫描 + 生命周期触发），Agent 输出中的敏感信息被自动拦截
- **copilot-shell**：更新到 v2.4.1，新增跨 Session 自动记忆提取和 hook reason UI 可见性，用户可看到安全 hook 拦截操作的具体原因

## [0.5] - 2026-05-28

### 组件版本

| 组件 | 版本 |
|------|------|
| copilot-shell | 2.4.0 |
| agent-sec-core | 0.4.0 |
| agentsight | 0.4.0 |
| tokenless | 0.4.0 |
| os-skills | 0.4.0 |

### 重点特性

- **tokenless**：更新到 v0.4.0，新增 Hermes 插件和 Tool Ready 环境机制，Agent 工具执行前依赖缺失被提前拦截避免 Token 浪费
- **agent-sec-core**：更新到 v0.4.0，交付 PIIChecker 和 Skill Scanner 首版，Agent 输出被扫描防止敏感信息泄露

### 组件更新

- **tokenless**：更新到 v0.4.0，开发 Hermes Agent 插件（Tool Ready 4 阶段环境预检 + History 压缩），Agent 运行时依赖在执行前被自动校验
- **agent-sec-core**：更新到 v0.4.0，新增 PIIChecker 输出 PII 检测和 Skill Scanner 基线能力，用户免受 Agent 无意泄露敏感数据的风险
- **agentsight**：更新到 v0.4.0，新增 Skill 维度指标展示，用户可按 Skill 查看 Token 消耗分组
- **os-skills**：更新到 v0.4.0，纳入 Nightly 自动化测试覆盖，Skill 质量持续验证

## [0.4] - 2026-05-13

### 组件版本

| 组件 | 版本 |
|------|------|
| copilot-shell | 2.3.0 |
| agent-sec-core | 0.4.1 |
| agentsight | 0.4.0 |
| tokenless | 0.3.0 |
| os-skills | 0.3.0 |
| ws-ckpt | 0.2.0 |

### 重点特性

- **agent-sec-core**：更新到 v0.4.1，建立 Skill 安全全生命周期管理（含 Prompt Scanner ask 策略），用户在 Agent 执行危险指令前收到确认提示
- **tokenless**：更新到 v0.3.0，搭建 4 套 Benchmark 对比基线，开发者可量化评估不同 Skill/OS 环境的 Token 消耗差异
- **ws-ckpt**：更新到 v0.2.0，扩展快照管理命令集，用户可按数量或时间维度自动清理历史快照

### 组件更新

- **agent-sec-core**：更新到 v0.4.1，集成 Prompt Scanner 至 cosh hook 和 OpenClaw 插件（ask 策略），用户在危险操作前获得交互式确认
- **tokenless**：更新到 v0.3.0，构建批量并发 Benchmark 平台并生成对比报告，开发者可一键跑分横向对比 Token 节省效果
- **agentsight**：更新到 v0.4.0，优化常驻进程内存占用，2C2G 小规格实例可稳定运行可观测服务
- **copilot-shell**：更新到 v2.3.0，适配 SWEBench 评测框架，开发者可通过 cosh 执行代码修复任务并验证通过率
- **ws-ckpt**：更新到 v0.2.0，丰富快照增删查能力，用户可按策略自动保留最近 N 份快照

## [0.3] - 2026-04-30

### 组件版本

| 组件 | 版本 |
|------|------|
| copilot-shell | 2.2.1 |
| agent-sec-core | 0.3.0 |
| agentsight | 0.3.1 |
| tokenless | 0.2.0 |
| os-skills | 0.3.0 |
| ws-ckpt | 0.1.0 |

### 重点特性

- **tokenless**：更新到 v0.2.0，交付命令重写和 TOON 上下文压缩，CLI 输出 Token 消耗降低 60–90%
- **agentsight**：更新到 v0.3.1，新增 Token 节省 Dashboard 和 Agent 异常诊断，用户可可视化节省趋势并检测 Agent 中断
- **agent-sec-core**：更新到 v0.3.0，新增 Skill Ledger 完整性追踪和 Prompt Scanner，每个 Skill 的签名链可端到端审计

### 新增组件

- **ws-ckpt**：首次发布 v0.1.0，构建基于 btrfs 的工作区快照守护进程，Agent 可毫秒级创建检查点并即时回滚文件系统状态

### 组件更新

- **tokenless**：更新到 v0.2.0，新增通过 RTK 的命令重写和 TOON 上下文压缩，Agent CLI 交互 Token 消耗减少 60–90%
- **agentsight**：更新到 v0.3.1，新增 Token 节省 Dashboard（Session/时间段统计）和 Agent 中断检测（drain 机制），用户可监控节省趋势并在 Agent 故障时收到告警
- **agent-sec-core**：更新到 v0.3.0，新增 Skill Ledger 全生命周期（check/certify/bypass/status/audit）和 Prompt Scanner 越狱检测，用户可追踪并强制执行 Skill 完整性策略
- **copilot-shell**：更新到 v2.2.1，新增 Extension 架构（command extension + system Hook + 即时激活）、Skill 市场对接和会话导出（Markdown/HTML/JSON），用户可通过插件扩展 cosh 能力并导出对话历史
- **os-skills**：更新到 v0.3.0，新增 Skill 市场上架和实用技能（xlsx/pdf-reader/image-gen/humanizer），用户可从市场发现并安装技能

## [0.2] - 2026-04-15

### 组件版本

| 组件 | 版本 |
|------|------|
| copilot-shell | 2.0.4 |
| agent-sec-core | 0.2.0 |
| agentsight | 0.2.2 |
| os-skills | 0.2.2 |
| tokenless | 0.1.0 |

### 组件更新

- **agentsight**：更新到 v0.2.2，新增 Token 消耗可观测（精确 Tokenizer 计量），用户可实时查看每条消息的 Token 明细
- **copilot-shell**：更新到 v2.0.4，新增独立鉴权（STS/ECS RAM Role）和 Skill 市场浏览，用户无需 AK/SK 即可认证并发现可用技能
- **os-skills**：更新到 v0.2.2，新增 SysAdmin 技能（Linux IO/网络/负载诊断），Agent 可独立诊断常见 OS 性能问题
- **tokenless**：首次发布 v0.1.0，构建 Skills 级 Benchmark 测试用例，开发者可跨 Skill 量化对比 Token 消耗

## [0.1] - 2026-03-30

### 组件版本

| 组件 | 版本 |
|------|------|
| copilot-shell | 2.0.1 |
| agent-sec-core | 0.1 |
| agentsight | 0.1 |
| os-skills | 0.1 |

### 新增组件

- **copilot-shell**：首次发布 v2.0.1，构建 AI 驱动终端助手（Tab 补全、/bash 模式、sudo、Hook 安全），用户开机即获得 AI 原生 CLI 交互体验
- **agent-sec-core**：首次发布 v0.1，交付 Skill 签名校验、安全沙箱和系统加固，Agent 操作在受控最小权限环境中运行
- **agentsight**：首次发布 v0.1，构建基于 eBPF 的零侵入可观测探针，用户无需修改 Agent 代码即可监控 LLM API 调用和 Token 消耗
- **os-skills**：首次发布 v0.1，整理系统管理、SysOM 运维、DevOps 和云技能库，Agent 可自主执行常见 OS 操作

### 安全

- Skill 全链路安全加密与数字签名
- 硬件级安全沙箱风险隔离
- Skill 调用身份认证与完整性校验

---

各组件详细变更日志请参阅：

**用户入口**
- [copilot-shell](src/copilot-shell/CHANGELOG.md)
- [cosh-ng](src/cosh-ng/CHANGELOG.md)
- [anolisa](src/anolisa/CHANGELOG.md)
- [os-skills](src/os-skills/CHANGELOG.md)

**Token 节省**
- [tokenless](src/tokenless/CHANGELOG.md)

**运行时**
- [agent-memory](src/agent-memory/CHANGELOG.md)
- [skillfs](src/skillfs/CHANGELOG.md)
- [ws-ckpt](src/ws-ckpt/CHANGELOG.md)

**Agent 可观测**
- [agentsight](src/agentsight/CHANGELOG.md)

**Agent 安全**
- [agent-sec-core](src/agent-sec-core/CHANGELOG.md)
