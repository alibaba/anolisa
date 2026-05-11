# Agent Sec Core 设计文档索引

本文档目录用于描述 `agent-sec-core` 的安全能力设计。各组件按职责独立成文；体系类文档只说明组件关系，不替代组件详细设计。

## Skill 安全相关文档

| 目标 | 建议阅读 |
|---|---|
| 快速了解 Skill 安全整体结构 | [SKILL_SECURITY_ARCHITECTURE_CN.md](SKILL_SECURITY_ARCHITECTURE_CN.md) |
| 了解 Skill 账本、签名、版本链和状态检查 | [SKILL_LEDGER_CN.md](SKILL_LEDGER_CN.md) |
| 了解 Skill 扫描器类型和扫描结果归一化 | [SKILL_SCANNER_CN.md](SKILL_SCANNER_CN.md) |

## 独立安全组件文档

| 目标 | 建议阅读 |
|---|---|
| 了解 Prompt 注入/越狱检测组件 | [PROMPT_SCANNER.md](PROMPT_SCANNER.md) |
| 了解隐私/敏感信息检测组件 | [PII_CHECKER_CN.md](PII_CHECKER_CN.md) |

## 文档简介

`SKILL_SECURITY_ARCHITECTURE_CN.md` 是 Skill 安全体系入口，负责说明 Skill 安全内部的模块关系和数据流。

`SKILL_LEDGER_CN.md` 是 `skill-ledger` 组件详细设计，聚焦 manifest、Ed25519 签名、版本链、`check` / `certify` / `status` / `audit` 以及 Scanner Registry。

`SKILL_SCANNER_CN.md` 是 Skill 扫描器设计，说明 `skill-vetter`、Skill 代码扫描 adapter 和 `NormalizedFinding` 归一化约定。

`PROMPT_SCANNER.md` 描述 Prompt 注入与越狱检测组件，覆盖预处理、规则检测、ML 分类、输出 schema 和审计日志。

`PII_CHECKER_CN.md` 描述隐私与敏感信息检测组件，覆盖检测范围、脱敏输出、CLI/API、检测器结构和测试要求。
