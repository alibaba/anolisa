# Skill 安全体系设计

## 1. 文档定位

本文是 Skill 安全体系的总览文档，说明 `agent-sec-core` 中与 Skill 安全相关的模块边界、数据流和核心状态模型。组件内部机制分别见：

- [SKILL_LEDGER_CN.md](SKILL_LEDGER_CN.md)：Skill 账本、签名、版本链和状态检查。
- [SKILL_SCANNER_CN.md](SKILL_SCANNER_CN.md)：Skill 扫描器、扫描结果归一化和扫描器接入。

## 2. 背景

Skill 通常由 `SKILL.md`、辅助脚本、配置、样例和元数据组成。Agent 加载 Skill 后，会根据其中的指令和脚本扩展自身能力，因此需要对 Skill 的来源、内容、变更和扫描结果建立可验证记录。

Skill 安全体系关注三类问题：

| 问题 | 说明 |
|---|---|
| 完整性 | Skill 文件是否被新增、删除或修改 |
| 可信状态 | Skill 的安全状态是否由本机可信密钥签名 |
| 扫描结论 | Skill 内容审查结果是否可归档、可追溯、可聚合 |

## 3. 模块划分

| 模块 | 职责 | 详细设计 |
|---|---|---|
| `skill-ledger` | 维护 SignedManifest、文件哈希、版本链、扫描结果和聚合状态 | [SKILL_LEDGER_CN.md](SKILL_LEDGER_CN.md) |
| Skill Scanner | 产生 `NormalizedFinding[]`，供 `skill-ledger certify` 写入 `scans[]` | [SKILL_SCANNER_CN.md](SKILL_SCANNER_CN.md) |
| Hook 集成 | 在 Skill 加载路径调用 `skill-ledger check`，根据状态输出提示 | [SKILL_LEDGER_CN.md](SKILL_LEDGER_CN.md) |
| 状态报告 | 通过 `skill-ledger status` 汇总已发现 Skill 的整体状态 | [SKILL_LEDGER_CN.md](SKILL_LEDGER_CN.md) |

## 4. 数据流

```text
Skill directory
      |
      v
skill-ledger check
      |
      +--> compute file hashes
      +--> verify SignedManifest
      +--> compare scanStatus
      |
      v
pass / none / drifted / warn / deny / tampered


Skill scanner
      |
      v
NormalizedFinding[]
      |
      v
skill-ledger certify
      |
      +--> build ScanEntry
      +--> aggregate scanStatus
      +--> sign SignedManifest
      |
      v
.skill-meta/latest.json
```

## 5. 状态模型

| 状态 | 含义 | 典型来源 |
|---|---|---|
| `pass` | 文件未变，签名有效，扫描通过 | `scanStatus=pass` |
| `none` | 已建立基线，但尚无有效扫描结论 | 首次 check 或尚未 certify findings |
| `drifted` | 文件哈希与 manifest 不一致 | Skill 文件新增、删除或修改 |
| `warn` | 扫描存在低风险发现 | 任一扫描器输出 `warn` |
| `deny` | 扫描存在高风险发现 | 任一扫描器输出 `deny` |
| `tampered` | manifest 签名或版本链校验失败 | `.skill-meta/` 被篡改或密钥不匹配 |

`skill-ledger` 只负责状态计算和签名归档。具体扫描规则由 Skill Scanner 提供。

## 6. Scanner Registry

Scanner Registry 是 `skill-ledger` 接收扫描结果的扩展点。扫描器按照调用方式分为：

| 类型 | 调用方式 | 示例 |
|---|---|---|
| `skill` | Agent 按协议执行，用户或 Agent 提供 findings 文件 | `skill-vetter` |
| `builtin` | CLI 进程内调用 | Skill 代码扫描 adapter |
| `cli` | 子进程调用外部工具 | 预留 |
| `api` | 调用远端服务 | 预留 |

所有扫描器最终都需要输出或被转换为 `NormalizedFinding[]`。归一化约定见 [SKILL_SCANNER_CN.md](SKILL_SCANNER_CN.md)。

## 7. 宿主集成

Skill 安全体系面向 OpenClaw 和 copilot-shell 提供一致语义：

```text
Skill load
  -> resolve skill_dir
  -> agent-sec-cli skill-ledger check <skill_dir>
  -> pass: silent allow
  -> non-pass: allow with warning
```

当前策略以提示为主，不改变 Skill 的执行可用性。Hook 层只解释 `skill-ledger` 状态，不改变 SignedManifest 数据结构。
