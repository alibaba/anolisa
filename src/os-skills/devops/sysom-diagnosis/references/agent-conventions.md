# Agent 约定（sysom-diagnosis）

本文件从 SKILL.md 迁出，详述 Agent 行为规范。主 SKILL 仅保留三条核心规则与索引链接。

## 执行目录

- `./shared/scripts/osops` 为相对路径，仅在 **技能根**（与 `SKILL.md` 同级）下有效。
- 推荐写法：`cd <技能根绝对路径> && ./shared/scripts/osops …`。
- `agent.next` 中的 `command` 同样须在技能根用 Bash 执行，不要改为让用户自行复制。

## 本地优先

- 用户只说「内存高」「OOM」等泛化症状、未明确要远程时，先跑本机 quick（按 [memory-routing.md](./memory-routing.md) 选子命令），不索要 region/instance。
- 用户明确要远程、或 `agent.next` 已给出 `--deep-diagnosis` 时，再按 [invoke-diagnosis.md](./invoke-diagnosis.md) 区分本机/远程。

## 凭证与安全

- **禁止**在对话中收集 AccessKey / Secret。
- 引导用户在本机终端于技能根执行 `./shared/scripts/osops configure`，写入 `~/.aliyun/config.json`。
- 无 PTY 时：在 COSH 中通过 `/settings` 开启「交互式 Shell（PTY）」，或用 `/bash` 进入交互式 Bash。

## Precheck 信封消费

- 按 `data.remediation`（独立 precheck）或 `data.precheck_gate.remediation`（deep-diagnosis 合并）**顺序**引导用户修复。`primary_path` 已锁定时只展示该路径，`configure_identity` 时按 `guidance.auth_path_choice` 让用户选择。
- `data.precheck_gate` 存在时以其为准，不要把 quick findings 与 precheck findings 重复全文复述。

## 检查与使用顺序

### A. 内存域

1. **快速排查**：按 [memory-routing.md](./memory-routing.md) 选本机子命令。下一步读 `agent.next`，关键发现见 `agent.findings`。
2. **（可选）`precheck`**：单独验证凭证与 SysOM 开通。
3. **确认目标**：当前实例不传 `--region`/`--instance`；其它实例须用户给出 region + instance-id。
4. **深度诊断**：按 `agent.next` 中的 `command`，或加 `--deep-diagnosis`。
5. **失败处理**：读 `error`、`data`（含 `remediation`、`precheck_gate` 等）、`agent.summary`。

### B. 非内存域（IO / 网络 / 负载）

1. **确认当前/其它实例**（同 A）→ 2. `./shared/scripts/osops <io|net|load> <子命令> …`（调用前内建环境检查）→ 读 `data.routing`/`data.remote`、`agent.findings`。
2. **网络延迟 + socket 队列积压**：已跑 `net netjitter`/`net packetdrop` 且结果正常，但 `ss` 显示 Send-Q/Recv-Q 偏大时，须交叉 `memory memgraph --deep-diagnosis`。详见 [non-memory-routing.md](./non-memory-routing.md)。
3. **失败处理**：同 A。

## 与其它 memory 技能的边界

- 本技能在 `sysom-diagnosis/` 下使用 `./shared/scripts/osops`（`memory`/`io`/`net`/`load` 配套）。
- 其它技能或父仓库里的入口可能与本目录不同；SysOM 远程专项请使用本目录内的 `osops`。
