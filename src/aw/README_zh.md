# AW

[English](README.md)

AW 为原生 Agent adapter、AW Core 和组件 Provider 定义版本化能力合同。这个 Rust 库提供 JSON Schema 和离线语义校验，供各层核对调用与结果。Provider 生成的候选需要经过独立的采用观测，才能确认环境用了什么。进程控制和状态操作也分别记录授权与执行结果，现有组件继续按原有路径运行。

**当前版本用于接口评审，尚未冻结。** 本次提供合同和校验器，AW Core 服务、Provider Host 及原生 adapter 的接入仍需后续实现。Ledger 存储、进程控制和状态 Provider 也不在本次交付中。测试使用合成样例检查合同是否一致，实际接入需要另行验收。

## 运行本地检查

准备 Rust 工具链及 rustfmt、Clippy。跨语言摘要测试还需要 Python 3 和 Node.js，Rust 库本身不依赖这两个运行时。在仓库根目录执行以下命令。

```bash
cd src/aw
cargo test --workspace --locked
python3 tests/check_canonical.py
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
```

这些检查可由普通用户运行，无需启动 Agent 或登录服务。构建时，Cargo 会下载尚未缓存的依赖；Schema 校验只读取随包资源，不会访问 Schema URI。

本次验证使用 Linux ARM64，工具版本为 Rust 1.97.1、Python 3.12.3 和 Node.js 24.15.0。这组版本用于复现本次检查。最低支持版本尚未核定，其他操作系统也尚未验证。

## 阅读与接入

- [PoC 基线与本次增量](docs/design/poc-schema-delta_zh.md)
- [冻结决策与分阶段验收](docs/design/interface-freeze_zh.md)
- [每个 Schema 的字段、取舍和评审边界](docs/design/schema-reference_zh.md)
- [Schema 文件](schemas/)与[完整合成样例](tests/fixtures/contracts.json)
- [校验 API](src/validation.rs)与[合同测试](tests/contracts.rs)

接入时，先用 `canonical::parse` 严格解析收到的字节，再通过 `Registry` 检查结构。Core 用 `validate_plan` 核对有序计划；每次调用都要通过 `validate_plan_invocation` 和 `validate_invocation`。计划结束后，`validate_plan_execution` 检查全部步骤及其 Receipt。

确认采用时调用 `validate_plan_adoption`。最终派发工具时调用 `validate_dispatch`，一并核对计划结果、当前执行意图和独立 OS 防护绑定。`validate_result`、`validate_adoption` 和 `validate_execution_gate` 只检查局部记录；完整准入还需要上述计划级校验。`Registry::validate` 通过也只说明结构符合合同。

原生执行器负责认证证据，并保证最终检查与实际动作之间不会插入未经检查的变更。它还要按 `event_id` 防止重复派发。Provider 调度和 OS 策略安装由后续运行时实现。

## 合同范围

当前 Registry 注册 21 个 Schema，其中包含 1 个共享定义、8 个能力输入输出，以及 12 个协调和证据资源。目录中另保留 8 份 PoC v1 原文供评审，它们未注册到当前库。能力合同使用 v2；调用双方必须协商一致的 Schema ID 和资源摘要，才能使用该版本。

文本投影只处理**一个 UTF-8 文本槽位**，外围工具结果和其他内容块保持原状。Receipt 记录 Provider 的输出。实际采用则要在 `final_tool_result`、`local_history` 或 `model_request` 对应的位置独立观测。每个位置只能证明本层发生的事，无法证明远端模型已经消费了内容。Ledger 的确认是否真实、记录是否持久保存，由存储层保证。

状态操作目前只有通用的审批与幂等状态机。快照和恢复的具体输入输出，以及恢复实现，需要单独设计和评审。

OS 防护应持续限制受控进程的系统访问。`os-protection-binding` 记录目标、策略和控制项的生效证据；必需防护缺失、过期或仅声明支持时，`validate_dispatch` 会拒绝准入。内核限制是否生效、跳过 hook 后能否继续阻断访问、子进程是否受保护，都需要后续实现与实机验证。
