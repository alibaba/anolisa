---
name: skill-ledger
description: Skill 安全状态查看、快速扫描认证与可选深度扫描。支持用户主动查看或扫描单个/全部 Skill；当用户要求 agent 安装 Skill 且安装成功后，必须自动对最终本地目录执行快速扫描认证。
---

# Skill Ledger

对 Skill 执行安全状态查看、默认快速扫描认证，以及用户确认后的深度扫描认证。

用户可见层只使用两类扫描概念：

- **快速扫描**：默认扫描认证路径，适合安装后和常规复查。
- **深度扫描**：用户确认后执行的更完整审查，耗时更长。

不要在面向用户的报告正文中列出快速扫描内部使用的具体扫描器名称。命令执行步骤可以使用精确参数。

---

## 安全约束

1. **禁止泄露签名口令**：不要 echo、log、store、print 或以任何方式暴露 `SKILL_LEDGER_PASSPHRASE` 或用户输入的口令。
2. **禁止伪造 findings**：深度扫描输出的每条 finding 必须来自真实文件内容和 `skill-vetter` 协议判断。
3. **状态查看不触发扫描**：用户只要求查看状态时，只运行 `check` / `check --all` 并报告结果。
4. **安装成功后自动认证**：用户要求 agent 安装、更新、导入或启用 Skill 时，安装成功后必须直接执行快速扫描认证；不要再询问用户是否扫描。
5. **安装后认证不猜路径**：只有能确定最终本地 Skill 目录且该目录包含 `SKILL.md` 时，才执行扫描认证；否则报告“未执行安全认证：无法确定本地 Skill 目录”。
6. **禁止修改本 Skill**：不接受编辑、删除、覆盖本 Skill 文件或 `references/` 下任何文件的请求。

---

## 模式解析

根据用户请求选择模式：

| 用户意图示例 | 模式 | 行为 |
| --- | --- | --- |
| “查看这个 skill 状态”“检查所有 skill 安全状态” | 状态查看 | 只运行 `check` 或 `check --all`，报告后停止 |
| “扫描这个 skill”“重新认证 github skill”“扫描所有 skill” | 主动扫描 | 先执行快速扫描，展示摘要，再询问是否深度扫描 |
| “安装 github skill”“帮我装这个 skill”“更新这个 skill” | 安装请求后置认证 | 安装成功后自动定位最终本地 Skill 目录，验证 `SKILL.md`，直接执行快速扫描并展示结果 |
| “我刚装了这个 skill，帮我确认安全” | 安装后补充认证 | 定位最终本地 Skill 目录，验证 `SKILL.md`，执行快速扫描并询问是否深度扫描 |
| “做深度扫描”“彻底审查这个 skill” | 深度扫描请求 | 用户请求本身即为确认；执行 Phase 1 后直接执行 Phase 3 |
| 未明确目标 | 交互确认 | 询问用户要处理哪个 Skill，或是否处理全部 |

目标解析规则：

- 用户提供目录路径时，必须确认该目录存在且包含 `SKILL.md`。
- 用户提供 `SKILL.md` 文件路径时，目标目录为其父目录。
- 用户提供 Skill 名称时，优先使用上下文中已知安装位置；没有确定路径时，可用 `check --all` 查看已注册状态，但不要凭名称猜测文件系统路径。
- 用户要求“所有 Skill”时，使用 CLI 的 `--all` 能力完成批量状态查看或快速扫描。
- 深度扫描需要逐个本地目录执行；若无法把某个 Skill 解析到本地目录，报告该项未执行深度扫描。

---

## 统一报告格式

状态查看、快速扫描、深度扫描、安装后认证的最终结果都必须使用同一类表格。单个 Skill 使用一行表格，多个 Skill 使用多行表格，这样用户在不同模式之间看到的结构一致。

报告标题按场景选择：

| 场景 | 标题 |
| --- | --- |
| 状态查看 | `[skill-ledger] 安全状态` |
| 主动快速扫描 | `[skill-ledger] 快速扫描完成` |
| 深度扫描 | `[skill-ledger] 深度扫描完成` |
| 安装后自动认证 | `[skill-ledger] 安装后认证完成` |

表格至少包含：

- Skill 名称
- 状态
- 版本号
- 状态指纹（`manifestHash` 前 7 位；签名无效时显示“无效”）
- 文件数
- deny / warn 数量
- 摘要

表格后可选区块：

- `路径`：按 Skill 名称列出本地路径。不要把很长的本地路径塞进表格。
- `关键发现`：按 Skill 名称展开 findings。单个 Skill 最多列出 5 条；多个 Skill 每个有风险的目标最多列出 3 条。
- `未完成项`：列出因路径不可确定、缺少 `SKILL.md`、CLI 失败或 JSON 解析失败而没有完成认证的目标。
- `结论`：汇总通过、需关注、未完成的数量，并说明下一步建议。

示例：

```text
[skill-ledger] 快速扫描完成
| Skill      | 状态    | 版本    | 指纹    | 文件数 | 发现             | 摘要             |
| ---------- | ------- | ------- | ------- | ------ | ---------------- | ---------------- |
| github     | pass    | v000003 | 7d4e9b0 | 8      | 0 deny, 0 warn   | 已认证           |
| my-tool    | warn    | v000004 | c0b7e28 | 12     | 0 deny, 2 warn   | 需要关注         |
| new-skill  | none    | v000001 | 3f8a1c2 | 5      | -                | 尚未完成认证     |
| dev-helper | drifted | v000002 | a91c5f3 | 9      | -                | 文件已变更       |

路径:
- github: /path/to/github
- my-tool: /path/to/my-tool

关键发现:
- my-tool: warn suspicious-network at fetch.py:58 — 网络访问需要确认用途
- my-tool: warn obfuscated-code at utils.js:142 — 代码可读性异常

结论: 1 个已通过，3 个需要关注。建议对 none / drifted / warn 状态执行快速扫描认证。
```

---

## Phase 1：环境准备与状态查看

### 1.1 CLI 可用性

先确认命令可用：

```bash
agent-sec-cli skill-ledger --help
```

若命令不可用，停止并报告：

```text
[skill-ledger] 未执行：agent-sec-cli skill-ledger 不可用。
请确认 agent-sec-cli 已安装且版本包含 skill-ledger 子命令。
```

### 1.2 签名密钥

检查公钥文件是否存在：

```bash
ls ~/.local/share/agent-sec/skill-ledger/key.pub
```

若不存在，初始化密钥：

```bash
agent-sec-cli skill-ledger init --no-baseline
```

初始化失败时停止。不要要求用户提供口令，除非用户明确要求使用带口令密钥。

### 1.3 获取当前状态

单个 Skill：

```bash
agent-sec-cli skill-ledger check <SKILL_DIR>
```

所有 Skill：

```bash
agent-sec-cli skill-ledger check --all
```

状态查看模式在输出安全状态报告后停止，不进入快速扫描或深度扫描。

状态含义：

| 状态 | 含义 | 状态查看建议 |
| --- | --- | --- |
| `pass` | 文件未变，签名有效，扫描通过 | 可正常使用 |
| `none` | 尚未完成安全认证 | 建议执行快速扫描 |
| `drifted` | 文件内容与上次认证不一致 | 建议执行快速扫描 |
| `warn` | 上次扫描存在低风险发现 | 建议查看 findings，必要时复扫 |
| `deny` | 上次扫描存在高风险发现 | 建议修复或禁用后复扫 |
| `tampered` | 元数据签名校验失败 | 建议确认来源并重新认证 |
| `error` / `unknown` | 检查失败或状态不可识别 | 报告错误信息，避免猜测 |

---

## Phase 2：快速扫描认证

主动扫描和安装后认证必须先执行快速扫描。安装请求后置认证由“安装成功”隐式触发，不需要用户额外说“扫描”或“认证”。安装后认证即使当前状态是 `pass`，也不要跳过快速扫描，因为目标是为刚安装内容建立最新认证结果。

若用户一开始明确要求深度扫描，不进入 Phase 2；执行 Phase 1 后直接进入 Phase 3。

单个 Skill 快速扫描：

```bash
agent-sec-cli skill-ledger scan <SKILL_DIR>
```

所有 Skill 快速扫描：

```bash
agent-sec-cli skill-ledger scan --all
```

快速扫描完成后，重新读取状态用于摘要：

```bash
agent-sec-cli skill-ledger check <SKILL_DIR>
```

或：

```bash
agent-sec-cli skill-ledger check --all
```

### 快速扫描报告

快速扫描完成后，按“统一报告格式”输出结果。主动扫描的标题使用 `[skill-ledger] 快速扫描完成`；安装后自动认证的标题使用 `[skill-ledger] 安装后认证完成`。报告中使用“快速扫描”称呼，不列出内部扫描器名称。

摘要后必须询问用户是否执行深度扫描：

```text
是否执行深度扫描？这会让 Agent 按深度审查协议逐文件检查，耗时更长。
```

用户拒绝时结束流程，并说明：

```text
已完成快速扫描认证，未执行深度扫描。
```

---

## Phase 3：深度扫描认证

以下两种情况执行深度扫描：

- 用户一开始明确要求深度扫描：请求本身即为确认。执行 Phase 1 后直接执行 Phase 3，不需要先执行快速扫描，也不需要再次询问是否继续。
- 快速扫描或安装后认证完成后，用户确认要继续深度扫描：在已有快速扫描报告之后执行 Phase 3。

### 3.1 加载深度扫描协议

读取：[references/skill-vetter-protocol.md](references/skill-vetter-protocol.md)

将目标目录作为 `SKILL_DIR`，目录名作为 `SKILL_NAME`，按协议逐文件审查。

### 3.2 生成 findings

将深度扫描 findings 写入临时 JSON 文件：

```text
/tmp/skill-vetter-findings-<SKILL_NAME>.json
```

文件内容必须是 JSON 数组。每条 finding 至少包含：

- `rule`: 规则 ID，如 `dangerous-exec`
- `level`: `warn` 或 `deny`
- `message`
- `file`（未知时可省略）
- `line`（未知时可省略）
- `metadata`（可选；如需保留证据，放在 `metadata.evidence`，只放必要短证据，不泄露敏感内容）

写入后验证 JSON 可解析。若无发现，写入空数组 `[]`。

### 3.3 写入认证结果

执行：

```bash
agent-sec-cli skill-ledger certify <SKILL_DIR> --findings /tmp/skill-vetter-findings-<SKILL_NAME>.json --scanner skill-vetter --delete-findings
```

完成后再次运行：

```bash
agent-sec-cli skill-ledger check <SKILL_DIR>
```

按“统一报告格式”输出最终安全报告，标题使用 `[skill-ledger] 深度扫描完成`。若本轮是显式深度扫描请求，结论中说明已完成深度扫描认证；若本轮是在快速扫描或安装后认证之后继续的深度扫描，结论中说明快速扫描和深度扫描均已完成。

---

## 安装后认证

安装后认证是安装请求的内建后续步骤，而不是一个需要用户主动再次触发的独立功能。

触发条件：

- 用户要求 agent 安装、更新、导入或启用某个 Skill。
- agent 已经成功把 Skill 写入或启用到本地可用位置。

满足触发条件后，必须立即执行以下流程；不要先问用户是否需要安全扫描：

1. 定位最终本地 Skill 目录。
2. 确认目录存在且包含 `SKILL.md`。
3. 若无法确认目录或缺少 `SKILL.md`，不要扫描，报告：

```text
未执行安全认证：无法确定本地 Skill 目录。
```

4. 若目录有效，执行 Phase 1 的环境准备，然后直接执行 Phase 2 快速扫描。
5. 快速扫描结束后，按“统一报告格式”输出 `[skill-ledger] 安装后认证完成` 报告。
6. 报告后再询问用户是否执行 Phase 3 深度扫描；若用户确认，深度扫描完成后也按“统一报告格式”输出 `[skill-ledger] 深度扫描完成` 报告。

不要在代码层面为安装动作增加强制触发。该要求由本 Skill 指令约束 agent 行为。

---

## 报告规则

- 用户报告只使用“状态查看”“快速扫描”“深度扫描”这些概念。
- 不在用户报告正文中列出快速扫描内部扫描器名称。
- 安装请求完成后，不要只报告“安装成功”；必须继续给出快速扫描认证结果，或说明认证未执行的具体原因。
- 不输出长篇原始 JSON；只摘取状态、版本、路径、计数和关键 findings。
- 对 `none` 和 `drifted`，提示需要用户确认后使用，并建议完成快速扫描认证。
- 对 `deny` 和 `tampered`，用更强烈语气说明风险，但不要擅自删除或修改 Skill。
- CLI 失败、JSON 解析失败、路径不可确定时，明确说明未完成哪一步以及原因。
