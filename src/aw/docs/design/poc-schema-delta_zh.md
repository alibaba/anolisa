# PoC Schema 基线与本次增量

[English](poc-schema-delta.md)

评审时可以先看 PoC 已有的 8 份 v1 能力 Schema，再看本次新增的接口合同。分支按这个顺序保留了两层改动，便于逐份对照；PoC 的运行时实现未纳入本次变更。

## 来源与范围

第一层依次重放下列四个提交中的 Schema 改动，用 `Source-commit` 标注来源。重放后的 8 份文件与 [PoC 固定版本](https://github.com/kongche-jbw/anolisa/tree/5ebfc0b3905fa2f5f74aff2da4aec2b3be639647/src/aw/crates/aw-contracts/schemas) 逐字节一致，路径和 Schema ID 也保留原样。提取范围仅限 Schema 文件，原提交中的 Core、Host 和 Ledger 实现均未带入，Provider native 协议、manifest 及部署脚本也不在其中。

四次改动按以下顺序发生。

- [投影输入输出](https://github.com/kongche-jbw/anolisa/commit/b299cdfe)，引入两份投影 Schema。
- [安全检查输入输出](https://github.com/kongche-jbw/anolisa/commit/1556e9d6)，引入六份安全 Schema。
- [合同语义修正](https://github.com/kongche-jbw/anolisa/commit/4d47593b)，收紧恢复与安全结果约束。
- [输入与输出绑定修正](https://github.com/kongche-jbw/anolisa/commit/8ecb1412)，补充媒体类型和覆盖语义。

第二层 cherry-pick [接口提交](https://github.com/kongche-jbw/anolisa/commit/6fe1d30b0235b8039ac423d1649f2bdcb96d3b63)，再补充本对照说明。根 README 和 `.github/` 保持基线状态。

## 目录与兼容性

| 位置 | 作用 | 当前库是否注册 |
| --- | --- | --- |
| `crates/aw-contracts/schemas/` | 8 份 PoC v1 原文，仅供评审比较；这里没有第二个 Rust crate | 否 |
| `schemas/` | 21 份待评审合同，包括 8 份能力 v2、1 份 common、12 份协调及证据 Schema | 是 |
| `src/`、`tests/` | 离线校验 API、跨记录约束和合成样例 | 当前库实现 |

v1 文件供评审比较，当前 Registry 只注册新合同，PoC 调用方仍需单独迁移。旧的 `/schemas/capabilities/.../v1` 与新的 `/schemas/aw/.../v2` 对应不同合同，迁移时要调整 adapter/Provider 映射，并协商输入输出 Schema 及摘要。仅改版本数字或 URI 无法完成迁移，本次也未提供自动转换。

## 八份能力 Schema 逐项增量

下列文件名在旧目录使用 `-v1.schema.json`，在新目录使用 `-v2.schema.json`。

| 文件名前缀 | PoC 已有 | 本次变化及理由 |
| --- | --- | --- |
| `context-projection-prepare-input` | artifact、边界、是否允许媒体类型重编码 | 增加 `accepted_reversibility`；调用方显式约束可接受的恢复保证 |
| `context-projection-prepare-output` | 源 ID/摘要、候选文本、变换链和恢复类别 | 增加与恢复类别配对的 `recovery`；独立回读校验恢复结果，避免把类别声明当作证明 |
| `security-content-inspect-input` | artifact、边界、低置信度报告策略 | 使用共享 artifact 定义；统一文本槽位、ID、媒体类型和整数约束 |
| `security-content-inspect-output` | verdict、findings、扫描字节、truncated | 改为 `coverage`，明确输入摘要、输入/扫描字节、完整性、规则集和语言；保留 clean 必须完整且无发现的语义 |
| `security-code-inspect-input` | artifact、边界、bash/python/auto | 使用共享 artifact；明确 auto 需要同时覆盖两种语言，由跨记录校验约束 |
| `security-code-inspect-output` | 安全结果与 `language_detected` | `coverage.languages` 表示检查范围；校验与请求一致，避免将检测到的语言当作扫描范围 |
| `security-command-inspect-input` | 命令文本/摘要/语言、pre_tool | 增加 `execution_intent_digest`，绑定完整参数、目标、cwd、环境及防护策略 |
| `security-command-inspect-output` | allow/warn/deny、理由、发现、扫描字节 | 回显意图摘要并提供 coverage；最终准入重新核对当前执行意图 |

表中列出主要变化，共享定义还调整了 ID、媒体类型、边界和数值范围。逐字段约束见 [Schema 图册](schema-reference_zh.md)，完整的数据结构以两组 JSON 文件为准。

## 新增的协调与证据合同

PoC 已经包含 Provider、Receipt 和 Ledger，也有采用记录及 Core 顺序计划的部分 Rust 实现。本次把这些概念的边界进一步写入公开 Schema，并补充缺少的约束。

| Schema | 本次明确的边界 |
| --- | --- |
| `common` | 共享 scope、artifact、coverage、meter 和证据引用 |
| `boundary-descriptor` | adapter 能观察或阻断的真实位置、输入最终性与最终 guard |
| `runtime-binding` | 运行时身份、代际与观测来源；观测不等于控制权 |
| `control-grant` | 控制主体、动作、目标代际与有效期 |
| `execution-intent` | 检查与派发之间必须保持一致的完整执行意图 |
| `provider-descriptor` | Provider 身份、能力版本、保证及合同资源绑定 |
| `capability-invocation` | scope、预算、deadline、输入及固定 plan step |
| `provider-receipt` | Provider 结果与调用、输入和计划的绑定；不代替采用证据 |
| `context-adoption` | 独立观察的采用边界、实际文本和 Ledger 确认 |
| `operation-record` | 通用效果记录的审批、幂等与不确定状态；尚未定义快照 payload |
| `capability-plan` | 固定有序步骤、Provider 选择、必需 gate 和失败规则 |
| `plan-execution` | 全部步骤及 Receipt 的顺序证据；缺步或后续 allow 不能绕过 deny |
| `os-protection-binding` | 独立 OS 防护的目标、策略、覆盖、时效与证据 |

Core 负责一次 AW 计划内的执行顺序，框架其他插件的全局顺序由原生机制管理。OS 防护由原生执行层持续实施，本库核对可信调用方提供的绑定记录。内核隔离是否建立，需要系统层另行证明。

## 查看差异与验收范围

分支从已核对的 main 基线开始，共有五个提交。前四个依次保留 PoC 的 Schema 改动，第五个加入本次接口修改。查看第五个提交的 diff，就能看到新增内容；原有 8 份 Schema 在该提交中保持不变。在仓库根目录运行以下命令即可查看。

```bash
git log --oneline HEAD~5..HEAD
git diff --stat HEAD~1 HEAD
git diff HEAD~1 HEAD -- src/aw
git diff --exit-code HEAD~1 HEAD -- src/aw/crates/aw-contracts/schemas
```

前四个提交只包含 Schema，已检查 JSON 能否解析，并逐字节比对来源文件。第五个提交提供可编译的合同库，已通过组件 README 中的 Rust 检查、28 项测试和 Python/JavaScript 摘要向量校验。这些检查覆盖离线合同；Agent、Provider 和 OS 的实际接入仍需分阶段验收，接口冻结也需要评审通过。
