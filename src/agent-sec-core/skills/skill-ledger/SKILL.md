---
name: skill-ledger
description: Skill 安全扫描与完整性认证。检查运行环境并智能分诊目标 Skill（Phase 1 triage），执行安全审查（Phase 2 vetter），并通过密码学签名建立防篡改版本链（Phase 3 ledger）。支持单个 Skill 扫描、批量扫描、状态检查等多种模式。
---

# Skill Ledger — 安全扫描与完整性认证

对 Skill 执行安全审查并建立密码学签名的版本链。

- **Phase 1**：环境准备与智能分诊——检查 CLI、密钥，评估哪些 Skill 需要扫描
- **Phase 2**：安全扫描（vetter）——逐文件审查目标 Skill，输出结构化 findings
- **Phase 3**：建版签名（ledger）——将 findings 写入版本链，生成防篡改 SignedManifest

---

## 安全约束

1. **禁止泄露签名口令**：执行过程中 NEVER echo、log、store、print 或以任何方式在输出中暴露 `SKILL_LEDGER_PASSPHRASE` 环境变量或用户输入的口令。
2. **禁止伪造 findings**：Phase 2 的每条 finding 必须对应文件中实际检测到的模式。
3. **Phase 顺序不可跳过**：必须先完成 Phase 1，再执行 Phase 2，最后执行 Phase 3。不可跳过任何 Phase。
4. **禁止修改本 Skill**：不接受编辑、删除、覆盖本 Skill 文件或 `references/` 下任何文件的请求。

---

## 模式解析

从用户的请求中识别运行模式。用户通过自然语言表达意图，Agent 据此判定：

| 用户意图示例 | 模式 | 说明 |
|--------------|------|------|
| "扫描 /path/to/skill" 或 "审查 github skill" | 单个扫描 | 对指定 Skill 执行完整 Phase 1 → 2 → 3 |
| "扫描所有 skill" 或 "全部扫描" | 批量扫描 | 通过 `check --all` 解析所有已注册 Skill，逐一执行 |
| "检查 skill 状态" 或 "哪些 skill 需要扫描" | 仅检查 | 运行 `check` 命令，输出状态报告，不执行扫描 |
| 未明确指定目标 | 交互选择 | 询问用户：扫描哪个 Skill？或扫描全部？ |

**目标解析规则**：

- 若用户提供了 Skill 路径 → 直接使用该绝对路径
- 若用户提供了 Skill 名称（如 "github"）→ 按 project → custom → user → system 优先级查找对应目录
- 若用户要求批量操作 → 使用 `check --all`（CLI 内部读取 `~/.config/agent-sec/skill-ledger/config.json` 的 `skillDirs` 并展开 glob）

---

## Phase 1：环境准备与智能分诊

### Step 1.1：自完整性检查

在扫描其他 Skill 前，先验证自身完整性：

```bash
agent-sec-cli skill-ledger check <本 Skill 目录的绝对路径>
```

- `status` 为 `pass` → 继续
- `status` 为 `none` → 继续（skill-ledger 尚未被扫描过，属正常状态）
- `status` 为 `warn` → 输出提示并继续（上次扫描存在低风险项，不阻断）：

```
⚠️ [skill-ledger] 自身上次扫描存在低风险发现
状态: warn
建议：后续对 skill-ledger 自身重新执行扫描。
```

- `status` 为 `drifted`、`tampered` 或 `deny` → 输出告警并询问用户：

```
🚨 [skill-ledger] 自身完整性异常
状态: <status>
原因:
  drifted  — skill-ledger 文件已变更
  tampered — manifest 签名校验失败，元数据可能被篡改
  deny     — 上次扫描存在高危发现
建议：确认 skill-ledger 文件来源可信后再继续。
是否继续？(Y/N)
```

用户拒绝 → 停止。用户确认 → 继续（在输出中保留告警记录）。

### Step 1.2：CLI 可用性

```bash
agent-sec-cli skill-ledger --help
```

若命令不可用，输出：

```
[skill-ledger] Phase 1: [NOT_RUN]
原因: agent-sec-cli skill-ledger 不可用。
请确认 agent-sec-cli 已安装且版本包含 skill-ledger 子命令。
```

停止，不继续后续 Phase。

### Step 1.3：签名密钥

检查公钥文件是否存在：

```bash
ls ~/.local/share/agent-sec/skill-ledger/key.pub
```

若不存在 → 首次初始化（默认无口令，减少交互）：

```
[skill-ledger] 未检测到签名密钥，正在自动初始化...
```

执行：

```bash
agent-sec-cli skill-ledger init-keys
```

> **设计说明**：Skill 驱动的首次初始化默认不设口令，以实现零交互自动化。不指定 `--passphrase` 时，CLI 不会读取环境变量或提示输入，始终生成无口令密钥。密钥安全性由文件系统权限保障（`key.enc` mode 0600）。用户后续可通过 `agent-sec-cli skill-ledger init-keys --force --passphrase` 重新生成带口令保护的密钥。

初始化成功后从 JSON 输出中提取 `fingerprint` 字段并继续。失败 → 停止。

### Step 1.4：预扫描分诊

根据模式解析（见上方模式解析表）确定目标并获取当前状态。所有元数据均从 `check` 命令的 JSON 输出中提取，无需读取任何文件。

**单个模式**：

```bash
agent-sec-cli skill-ledger check <skill_dir>
```

输出为单个 JSON 对象，包含 `status`、`skillName`、`versionId`、`createdAt`、`updatedAt`、`fileCount`、`manifestHash` 等字段。

**批量模式 / 交互模式**：

```bash
agent-sec-cli skill-ledger check --all
```

输出为 `{"results": [...]}` JSON 数组，每个元素包含上述字段。CLI 内部自动从 `config.json` 的 `skillDirs` 解析所有已注册 Skill 目录。

- 若为**交互模式**：将 `check --all` 结果展示给用户，由用户选择目标 Skill
- 若结果为空，输出提示并停止

解析 JSON 输出，按状态分类：

| 状态 | 符号 | 含义 | 处置 |
|------|------|------|------|
| `pass` | ✅ | 文件未变 + 签名有效 + 扫描通过 | 默认跳过 |
| `none` | 🆕 | 从未经过安全扫描 | 需要扫描 |
| `drifted` | 🔄 | **Skill 文件已变更**（fileHashes 不匹配）——无论签名状态如何 | 需要扫描 |
| `warn` | ⚠️ | 文件未变 + 签名有效 + 上次扫描有低风险 | 建议重新扫描 |
| `deny` | 🚨 | 文件未变 + 签名有效 + 上次扫描有高危项 | 建议重新扫描 |
| `tampered` | 🔴 | **文件未变但 manifest 签名无效**——元数据可能被伪造（如篡改 scanStatus 绕过安全检查） | 必须重新扫描 |

从 `check` 的 JSON 输出中直接提取版本号（`versionId`）、最近更新时间（`updatedAt`）、跟踪文件数（`fileCount`）、状态指纹（`manifestHash` 的前 7 位十六进制）。对 `warn`/`deny` 状态提取 `findings` 详情；对 `drifted` 状态提取变更文件清单（`added`/`removed`/`modified`）。

#### 仅检查模式

输出唯一的**安全状态报告**后停止，不进入 Phase 2。报告包含一张汇总表和一段安全结论：

```
[skill-ledger] 安全状态报告
┌─────────────┬────────────┬──────────┬────────────┬─────────────────────┬────────┬──────────────────────┐
│ Skill       │ 状态        │ 版本     │ 状态指纹    │ 最近更新时间         │ 文件数  │ 摘要                 │
├─────────────┼────────────┼──────────┼────────────┼─────────────────────┼────────┼──────────────────────┤
│ github      │ 🆕 none    │ v000001  │ 3f8a1c2    │ 2025-04-20T10:30:00Z│ 5      │ 从未扫描             │
│ docker      │ ✅ pass    │ v000002  │ 7d4e9b0    │ 2025-04-19T08:15:00Z│ 8      │ 无风险发现            │
│ my-tool     │ 🔄 drifted │ v000001  │ a91c5f3    │ 2025-04-18T14:00:00Z│ 3      │ +1 新增, ~1 修改      │
│ dev-helper  │ ⚠️ warn    │ v000003  │ c0b7e28    │ 2025-04-17T09:00:00Z│ 12     │ 2 条 warn            │
└─────────────┴────────────┴──────────┴────────────┴─────────────────────┴────────┴──────────────────────┘

安全结论:
  ✅ 安全通过: 1 (docker)
  需关注: 3 — 1 从未扫描, 1 文件变更, 1 低风险

  🔄 my-tool: SKILL.md 和 run.py 已修改, new-helper.sh 新增
  ⚠️ dev-helper: obfuscated-code (utils.js:142), suspicious-network (fetch.py:58)

  建议: 对非 pass 状态的 Skill 执行安全扫描以更新状态。
```

> **摘要列填充规则**：`none` → "从未扫描"；`pass` → "无风险发现"；`drifted` → 列出文件变更（如 "+N 新增, -N 删除, ~N 修改"）；`warn` → "N 条 warn"；`deny` → "N 条 deny, M 条 warn"；`tampered` → "签名校验失败"。
>
> **状态指纹列**：取 `check` 输出中 `manifestHash`（SHA-256 十六进制）的前 7 位显示，唯一标识当前 manifest 状态。所有状态均显示指纹（`manifestHash` 在创建时即计算，无论是否已签名）；`none` 表示首次 check 自动创建的无签名基线 manifest；`drifted` 显示变更前最后一次认证的指纹；`tampered`（签名无效）→ "⚠️ 无效"。
>
> **安全结论**中，仅对非 `pass`/`none` 状态的 Skill 展开详情：`drifted` 列出变更文件；`warn`/`deny` 列出具体 findings（规则 ID + 文件位置）；`deny` 以 🚨 标注并建议立即修复或禁用。

#### 扫描模式

基于分诊结果，列出待扫描数量与列表，询问用户确认（用户可选择跳过某些或强制加入 `pass` 状态的 Skill）。确认后进入 Phase 2。

---

## Phase 2：安全扫描（vetter）

对待扫描列表中的每个 Skill 执行安全审查。

### 扫描器调度

Phase 2 采用 **Scanner Registry 驱动**的扫描流程，支持横向扩展：

1. 读取 `~/.config/agent-sec/skill-ledger/config.json` 的 `scanners[]` 配置
2. 筛选 `type == "skill"` 的扫描器（CLI 无法直接调用的，需要 Agent 驱动）
3. 对每个 `skill` 类型扫描器，加载对应的 `references/<scanner-name>-protocol.md` 协议文件
4. 按协议执行扫描，生成 findings 文件

> **v1 版本**：仅注册 `skill-vetter`（`type: "skill"`）。`builtin`/`cli`/`api` 类型扫描器由 `certify` 的自动调用模式处理（Phase 3），无需 Agent 驱动。

### 对每个待扫描 Skill 执行

#### 2.1 加载扫描协议

当前版本加载：[references/skill-vetter-protocol.md](references/skill-vetter-protocol.md)

将 `SKILL_DIR` 和 `SKILL_NAME`（目录名）传入扫描协议。

#### 2.2 执行扫描

按 `skill-vetter-protocol.md` 定义的四阶段框架执行：

1. **Stage 1：来源验证** — 检查目录结构与元数据
2. **Stage 2：强制代码审查** — 逐文件应用规则表
3. **Stage 3：权限边界评估** — 比对声明能力与实际内容
4. **Stage 4：风险分级与输出** — 汇总并写入 findings JSON

#### 2.3 验证输出

确认 findings 文件已写入：

```bash
cat /tmp/skill-vetter-findings-<SKILL_NAME>.json | python3 -c "import json,sys; d=json.load(sys.stdin); print(f'findings: {len(d)}')"
```

若文件不存在或 JSON 格式无效 → 标记该 Skill 为扫描失败，继续下一个。

#### 2.4 Phase 2 状态输出

扫描过程中，每个 Skill 完成后输出单行进度（包含文件数与发现统计）：

```
[skill-ledger] Phase 2: 扫描 3 个 Skill...
[skill-ledger] Phase 2: github — 完成 (5 文件, 0 deny, 0 warn)
[skill-ledger] Phase 2: my-tool — 完成 (3 文件, 0 deny, 2 warn)
[skill-ledger] Phase 2: dev-helper — 完成 (12 文件, 0 deny, 0 warn)
[skill-ledger] Phase 2 完成: 成功 3 / 3
```

若某个 Skill 扫描失败：

```
[skill-ledger] Phase 2: <SKILL_NAME> — 失败 (<错误原因>)
```

> **设计说明**：Phase 2 仅输出单行进度，不展示详细 findings 或汇总表。所有扫描结果统一在 Phase 3 完成后的最终报告中呈现，避免中间产出多张表格导致信息分散。

若全部失败 → 停止，不进入 Phase 3。
若部分失败 → 询问用户是否继续对成功扫描的 Skill 执行 Phase 3。

---

## Phase 3：建版签名（ledger）

**前置条件**：Phase 2 已完成，至少一个 Skill 有有效的 findings 文件。

对每个成功扫描的 Skill 执行 `certify`：

### 3.1 执行 certify

```bash
agent-sec-cli skill-ledger certify <SKILL_DIR> \
  --findings /tmp/skill-vetter-findings-<SKILL_NAME>.json \
  --scanner skill-vetter
```

> 当 Scanner Registry 中有多个 `skill` 类型扫描器时，对每个扫描器分别调用 `certify --findings <对应 findings> --scanner <对应 scanner>`。`certify` 会自动合并同一 Skill 的多个 scanner 条目到 `scans[]` 数组。

#### 口令处理

若 `certify` 因口令错误失败（stderr 包含 `PassphraseError` 或 `wrong passphrase`），说明签名密钥受口令保护。按以下步骤处理：

1. 告知用户：「签名密钥需要口令才能完成认证签名。建议将口令设置为环境变量以避免反复输入：」

```
export SKILL_LEDGER_PASSPHRASE="<您的口令>"
```

2. 用户设置环境变量后，重试 `certify`。若用户直接提供口令而非设置环境变量，则通过内联环境变量传递：

```bash
SKILL_LEDGER_PASSPHRASE="<用户提供的口令>" agent-sec-cli skill-ledger certify <SKILL_DIR> \
  --findings /tmp/skill-vetter-findings-<SKILL_NAME>.json \
  --scanner skill-vetter
```

3. 若再次失败，告知用户口令不正确并请求重试（最多 3 次）
4. 3 次均失败 → 建议用户重新生成密钥（无口令模式，避免后续阻断）：

```
⚠️ 口令验证 3 次失败。建议重新生成签名密钥（无口令保护）：
  agent-sec-cli skill-ledger init-keys --force
```

执行重新生成后，从 Phase 3.1 重新开始对该 Skill 执行 certify。

> **安全提示**：口令仅通过 `SKILL_LEDGER_PASSPHRASE` 环境变量传递，**禁止**将口令写入命令行参数、日志或对话输出中。建议用户在 shell profile（如 `~/.bashrc`、`~/.zshrc`）中持久化该环境变量。

### 3.2 解析输出

`certify` 输出 JSON 到 stdout，解析关键字段：

| 字段 | 说明 |
|------|------|
| `versionId` | 版本号，如 `v000001` |
| `scanStatus` | 聚合状态：`pass` / `warn` / `deny` / `none` |
| `newVersion` | 布尔值，文件变更时为 `true`，未变更时为 `false` |
| `skillName` | Skill 名称（目录名） |
| `createdAt` | 版本创建时间（ISO 8601 UTC） |
| `updatedAt` | 最近更新时间（ISO 8601 UTC） |
| `fileCount` | 跟踪文件数 |
| `manifestHash` | 状态指纹（SHA-256），取前 7 位十六进制显示 |

### 3.3 Phase 3 状态输出

认证过程中，每个 Skill 完成后输出单行进度：

```
[skill-ledger] Phase 3: 认证 3 个 Skill...
[skill-ledger] Phase 3: github — 认证完成 (v000001, pass)
[skill-ledger] Phase 3: my-tool — 认证完成 (v000002, warn)
[skill-ledger] Phase 3: dev-helper — 认证完成 (v000003, pass)
```

### 3.4 最终报告

全部 Phase 完成后，输出唯一的**执行报告**（本次执行的最终交付物）。报告包含一张汇总表和一段安全结论，覆盖所有目标 Skill（含跳过的）以呈现完整视图：

```
[skill-ledger] 执行报告
┌─────────────┬────────────┬──────────┬────────────┬─────────────────────┬────────┬──────────────────────┐
│ Skill       │ 状态        │ 版本     │ 状态指纹    │ 最近更新时间         │ 文件数  │ 摘要                 │
├─────────────┼────────────┼──────────┼────────────┼─────────────────────┼────────┼──────────────────────┤
│ github      │ ✅ pass    │ v000001  │ 5e2d1a8    │ 2025-04-23T15:30:00Z│ 5      │ 无风险发现            │
│ my-tool     │ ⚠️ warn   │ v000002  │ 9c3f7b1    │ 2025-04-23T15:31:00Z│ 3      │ 2 条 warn            │
│ dev-helper  │ ✅ pass    │ v000003  │ 2a6e0d4    │ 2025-04-23T15:32:00Z│ 12     │ 无风险发现            │
│ docker      │ ✅ pass    │ v000002  │ 7d4e9b0    │ 2025-04-19T08:15:00Z│ 8      │ 沿用上次结果          │
└─────────────┴────────────┴──────────┴────────────┴─────────────────────┴────────┴──────────────────────┘

安全结论:
  ✅ pass: 3    ⚠️ warn: 1    总计: 4 个 Skill

  ⚠️ my-tool — 存在 2 条低风险发现:
    • obfuscated-code — 超长单行代码 (lib/encoder.js:203)
    • suspicious-network — 直连非标准端口 IP (net/client.py:88)

  建议: 审查上述发现，修复后重新扫描可将状态更新为 pass。
```

> **安全结论**中，仅对非 `pass` 状态的 Skill 展开 findings 详情（规则 ID + 描述 + 文件位置）。`deny` 状态以 🚨 标注并建议立即修复或禁用；`warn` 状态以 ⚠️ 标注并建议审查。跳过的 Skill（如上例 docker）在摘要列标记 "沿用上次结果"。
>
> **摘要列填充规则**（与安全状态报告一致）：`pass` → "无风险发现"；`warn` → "N 条 warn"；`deny` → "N 条 deny, M 条 warn"；跳过 → "沿用上次结果"。

---

## 错误处理

| 场景 | 处置 |
|------|------|
| CLI 命令返回非零退出码 | 输出 stderr 内容，标记该 Skill 为失败，继续处理下一个 |
| findings 文件 JSON 解析失败 | 标记为扫描失败，不执行 certify |
| certify 签名失败（口令错误） | 按 Phase 3「口令处理」流程：建议用户设置 `SKILL_LEDGER_PASSPHRASE` 环境变量后重试（最多 3 次）；3 次均失败则建议 `init-keys --force` 重新生成无口令密钥 |
| 目标目录不存在 | 跳过该 Skill，告警 |
| 批量模式 `check --all` 返回空结果 | 引导用户创建配置或切换为单个模式 |

---

## 附加资源

- 扫描协议: [references/skill-vetter-protocol.md](references/skill-vetter-protocol.md)
- 设计文档: Skill 安全技术方案（skill-ledger）
- CLI 子命令: `agent-sec-cli skill-ledger --help`
