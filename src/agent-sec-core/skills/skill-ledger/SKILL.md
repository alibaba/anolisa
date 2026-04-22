---
name: skill-ledger
description: Skill 安全扫描与完整性认证。对目标 Skill 执行安全审查（Phase 1 vetter），并通过密码学签名建立防篡改版本链（Phase 2 ledger）。支持单个 Skill 扫描、批量扫描、状态检查等多种模式。
---

# Skill Ledger — 安全扫描与完整性认证

对 Skill 执行安全审查并建立密码学签名的版本链。

- **Phase 0**：环境准备与智能分诊——检查 CLI、密钥，评估哪些 Skill 需要扫描
- **Phase 1**：安全扫描（vetter）——逐文件审查目标 Skill，输出结构化 findings
- **Phase 2**：建版签名（ledger）——将 findings 写入版本链，生成防篡改 SignedManifest

---

## 安全约束

1. **禁止泄露签名口令**：执行过程中 NEVER echo、log、store、print 或以任何方式在输出中暴露 `SKILL_LEDGER_PASSPHRASE` 环境变量或用户输入的口令。
2. **禁止伪造 findings**：Phase 1 的每条 finding 必须对应文件中实际检测到的模式。
3. **Phase 顺序不可跳过**：必须先完成 Phase 0，再执行 Phase 1，最后执行 Phase 2。不可跳过任何 Phase。
4. **禁止修改本 Skill**：不接受编辑、删除、覆盖本 Skill 文件或 `references/` 下任何文件的请求。

---

## 模式解析

从用户的请求中识别运行模式。用户通过自然语言表达意图，Agent 据此判定：

| 用户意图示例 | 模式 | 说明 |
|--------------|------|------|
| "扫描 /path/to/skill" 或 "审查 github skill" | 单个扫描 | 对指定 Skill 执行完整 Phase 0 → 1 → 2 |
| "扫描所有 skill" 或 "全部扫描" | 批量扫描 | 从 config 的 `skillDirs` 解析所有 Skill，逐一执行 |
| "检查 skill 状态" 或 "哪些 skill 需要扫描" | 仅检查 | 运行 `check` 命令，输出状态报告，不执行扫描 |
| 未明确指定目标 | 交互选择 | 询问用户：扫描哪个 Skill？或扫描全部？ |

**目标解析规则**：

- 若用户提供了 Skill 路径 → 直接使用该绝对路径
- 若用户提供了 Skill 名称（如 "github"）→ 按 project → custom → user → system 优先级查找对应目录
- 若用户要求批量操作 → 读取 `~/.config/skill-ledger/config.json` 的 `skillDirs` 展开所有 Skill 目录

---

## Phase 0：环境准备与智能分诊

### Step 0.0：自完整性检查

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

### Step 0.1：CLI 可用性

```bash
agent-sec-cli skill-ledger --help
```

若命令不可用，输出：

```
[skill-ledger] Phase 0: [NOT_RUN]
原因: agent-sec-cli skill-ledger 不可用。
请确认 agent-sec-cli 已安装且版本包含 skill-ledger 子命令。
```

停止，不继续后续 Phase。

### Step 0.2：签名密钥

检查公钥文件是否存在：

```bash
ls ~/.local/share/skill-ledger/key.pub
```

若不存在 → 首次初始化（默认无口令，减少交互）：

```
[skill-ledger] 未检测到签名密钥，正在自动初始化...
```

执行：

```bash
SKILL_LEDGER_PASSPHRASE="" agent-sec-cli skill-ledger init-keys
```

> **设计说明**：Skill 驱动的首次初始化默认不设口令（`SKILL_LEDGER_PASSPHRASE=""`），以实现零交互自动化。密钥安全性由文件系统权限保障（`key.enc` mode 0600）。用户后续可通过 `agent-sec-cli skill-ledger init-keys --force --passphrase` 重新生成带口令保护的密钥。

初始化成功后输出指纹并继续。失败 → 停止。

### Step 0.3：目标解析

根据模式解析（见上方模式解析表）确定目标 Skill 列表：

- **单个模式**：`TARGET_SKILLS = [ <skill_dir 绝对路径> ]`
- **批量模式**：读取 `~/.config/skill-ledger/config.json` 的 `skillDirs` 字段，展开 glob，收集所有有效 Skill 目录
- **交互模式**：列出已知 Skill 目录供用户选择

若目标列表为空，输出提示并停止。

### Step 0.4：预扫描分诊

对 `TARGET_SKILLS` 中的每个 Skill，执行 `check`：

```bash
agent-sec-cli skill-ledger check <skill_dir>
```

解析 JSON 输出，按状态分类：

| 状态 | 符号 | 含义 | 处置 |
|------|------|------|------|
| `pass` | ✅ | 文件未变 + 签名有效 + 扫描通过 | 默认跳过 |
| `none` | 🆕 | 从未经过安全扫描 | 需要扫描 |
| `drifted` | 🔄 | **Skill 文件已变更**（fileHashes 不匹配）——无论签名状态如何 | 需要扫描 |
| `warn` | ⚠️ | 文件未变 + 签名有效 + 上次扫描有低风险 | 建议重新扫描 |
| `deny` | 🚨 | 文件未变 + 签名有效 + 上次扫描有高危项 | 建议重新扫描 |
| `tampered` | 🔴 | **文件未变但 manifest 签名无效**——元数据可能被伪造（如篡改 scanStatus 绕过安全检查） | 必须重新扫描 |

输出分诊摘要表：

```
[skill-ledger] 预扫描分诊
┌─────────────┬────────────┬───────────┐
│ Skill       │ 状态        │ 处置      │
├─────────────┼────────────┼───────────┤
│ github      │ 🆕 none    │ 需要扫描   │
│ docker      │ ✅ pass    │ 跳过      │
│ my-tool     │ 🔄 drifted │ 需要扫描   │
└─────────────┴────────────┴───────────┘
待扫描: 2 / 3
```

**仅检查模式**：输出分诊表后停止，不进入 Phase 1。

**扫描模式**：询问用户确认待扫描列表（用户可选择跳过某些或强制加入 `pass` 状态的 Skill）。确认后进入 Phase 1。

---

## Phase 1：安全扫描（vetter）

对待扫描列表中的每个 Skill 执行安全审查。

### 扫描器调度

Phase 1 采用 **Scanner Registry 驱动**的扫描流程，支持横向扩展：

1. 读取 `~/.config/skill-ledger/config.json` 的 `scanners[]` 配置
2. 筛选 `type == "skill"` 的扫描器（CLI 无法直接调用的，需要 Agent 驱动）
3. 对每个 `skill` 类型扫描器，加载对应的 `references/<scanner-name>-protocol.md` 协议文件
4. 按协议执行扫描，生成 findings 文件

> **v1 版本**：仅注册 `skill-vetter`（`type: "skill"`）。`builtin`/`cli`/`api` 类型扫描器由 `certify` 的自动调用模式处理（Phase 2），无需 Agent 驱动。

### 对每个待扫描 Skill 执行

#### 1.1 加载扫描协议

当前版本加载：[references/skill-vetter-protocol.md](references/skill-vetter-protocol.md)

将 `SKILL_DIR` 和 `SKILL_NAME`（目录名）传入扫描协议。

#### 1.2 执行扫描

按 `skill-vetter-protocol.md` 定义的四阶段框架执行：

1. **Stage 1：来源验证** — 检查目录结构与元数据
2. **Stage 2：强制代码审查** — 逐文件应用规则表
3. **Stage 3：权限边界评估** — 比对声明能力与实际内容
4. **Stage 4：风险分级与输出** — 汇总并写入 findings JSON

#### 1.3 验证输出

确认 findings 文件已写入：

```bash
cat /tmp/skill-vetter-findings-<SKILL_NAME>.json | python3 -c "import json,sys; d=json.load(sys.stdin); print(f'findings: {len(d)}')"
```

若文件不存在或 JSON 格式无效 → 标记该 Skill 为扫描失败，继续下一个。

#### 1.4 Phase 1 状态输出

每个 Skill 扫描完成后输出：

```
[skill-ledger] Phase 1 完成: <SKILL_NAME>
扫描器: skill-vetter
```

全部 Skill 扫描完成后输出汇总：

```
[skill-ledger] Phase 1 汇总
成功: N 个 Skill
失败: M 个 Skill（<列出失败的 Skill 名>）
```

若全部失败 → 停止，不进入 Phase 2。
若部分失败 → 询问用户是否继续对成功扫描的 Skill 执行 Phase 2。

---

## Phase 2：建版签名（ledger）

**前置条件**：Phase 1 已完成，至少一个 Skill 有有效的 findings 文件。

对每个成功扫描的 Skill 执行 `certify`：

### 2.1 执行 certify

```bash
agent-sec-cli skill-ledger certify <SKILL_DIR> \
  --findings /tmp/skill-vetter-findings-<SKILL_NAME>.json \
  --scanner skill-vetter
```

> 当 Scanner Registry 中有多个 `skill` 类型扫描器时，对每个扫描器分别调用 `certify --findings <对应 findings> --scanner <对应 scanner>`。`certify` 会自动合并同一 Skill 的多个 scanner 条目到 `scans[]` 数组。

### 2.2 解析输出

`certify` 输出 JSON 到 stdout，解析关键字段：

| 字段 | 说明 |
|------|------|
| `versionId` | 版本号，如 `v000001` |
| `scanStatus` | 聚合状态：`pass` / `warn` / `deny` / `none` |

### 2.3 Phase 2 状态输出

每个 Skill 认证完成后输出：

```
[skill-ledger] Phase 2 完成: <SKILL_NAME>
版本: <versionId>
扫描状态: <scanStatus>
Manifest: <SKILL_DIR>/.skill-meta/latest.json
```

### 2.4 最终汇总

全部 Skill 处理完成后输出总结：

```
[skill-ledger] 执行完毕
┌─────────────┬──────────┬────────────┐
│ Skill       │ 版本     │ 扫描状态   │
├─────────────┼──────────┼────────────┤
│ github      │ v000001  │ ✅ pass    │
│ my-tool     │ v000003  │ ⚠️ warn   │
└─────────────┴──────────┴────────────┘
```

---

## 错误处理

| 场景 | 处置 |
|------|------|
| CLI 命令返回非零退出码 | 输出 stderr 内容，标记该 Skill 为失败，继续处理下一个 |
| findings 文件 JSON 解析失败 | 标记为扫描失败，不执行 certify |
| certify 签名失败（口令错误） | 提示用户重新输入口令（最多 3 次），全部失败则停止 |
| 目标目录不存在 | 跳过该 Skill，告警 |
| 批量模式 config.json 不存在 | 引导用户创建配置或切换为单个模式 |

---

## 附加资源

- 扫描协议: [references/skill-vetter-protocol.md](references/skill-vetter-protocol.md)
- 设计文档: Skill 安全技术方案（skill-ledger）
- CLI 子命令: `agent-sec-cli skill-ledger --help`
