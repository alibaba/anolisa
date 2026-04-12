---
name: sysom-diagnosis
version: 1.14.0
description: 面向 Linux 对内存、网络、IO、负载等问题做深度分析与诊断。内存含内存高/不足、OOM、oom-killer、Java 应用内存与内存全景大图（含 socket 队列与 TCP 内存视角）；网络含丢包与时延抖动（与 memgraph 互补）；IO 含磁盘统计与慢 IO；负载含调度延迟与系统负载。输出结构化诊断信封供自动化续跑。

layer: application
category: os-ops
lifecycle: operations

tags:
  - os-ops
  - sysom
  - diagnosis
  - ecs
  - memory
  - oom
  - oom-killer
  - 内存不足
  - 内核日志
  - io
  - network
  - load

status: beta
---

# SysOM 诊断（sysom-diagnosis）

在 **`sysom-diagnosis/`**（技能根）下使用 **`./shared/scripts/osops`** 做诊断。stdout 为 **JSON 信封**，默认**本机快速排查不调云**；远程专项由 **`--deep-diagnosis`**（内存域）或 **`io`/`net`/`load` 子命令**触发，内建环境检查。

## Agent 核心行为

1. **本机优先**：先跑 memory quick，不要默认索要 region/instance。
2. **意图路由**：
   - **内存域**：占用高/大图 → `memgraph`；OOM/oom-killer → `oom`；Java → `javamem`；不明 → `classify`。详见 [memory-routing.md](./references/memory-routing.md)。
   - **非内存域**：IO/网络/负载 → 对应 `io`/`net`/`load` 子命令，直接走远程专项。详见 [non-memory-routing.md](./references/non-memory-routing.md)。
3. **服从信封指令**：始终读 `agent.summary` 并执行 `agent.next`。quick 输出仅为信号检测，`agent.next` 有命令时必须先执行，再向用户总结。
4. **信封即结果**：诊断结论以信封 `data` 为准，无需自行采集额外信息。
5. **网络延迟 + socket 队列积压**：已跑 `net netjitter`/`net packetdrop` 且结果正常，但 `ss` 显示 Send-Q/Recv-Q 偏大时，须交叉 `memory memgraph --deep-diagnosis`。

完整约定（执行目录、凭证安全、precheck 降噪等）见 [agent-conventions.md](./references/agent-conventions.md)。

## 信封输出

CLI stdout 为 JSON 信封（`format: sysom_agent`, `version: 3.4`）。Agent 直接消费 `agent.summary`（摘要）、`agent.findings`（关键指标）、`agent.next`（下一步命令，应在技能根用 Bash 执行）；业务载荷在 `data.routing`、`data.local`、`data.remote`。详见 [output-format.md](./references/output-format.md)。

### Precheck / 认证失败

认证失败时信封含 `data.remediation`（独立 precheck）或 `data.precheck_gate.remediation`（deep-diagnosis 合并），按信封指令引导用户完成配置。详见 [agent-conventions.md](./references/agent-conventions.md)。

## 子命令速查

### 内存域

| 子命令 | 能力 | 专文 |
|--------|------|------|
| `memory memgraph` | 内存全景/大盘，含 TCP 内存与 socket 队列 | [memgraph.md](./references/diagnoses/memgraph.md) |
| `memory oom` | OOM / oom-killer 专项 | [oomcheck.md](./references/diagnoses/oomcheck.md) |
| `memory javamem` | Java 内存 | [javamem.md](./references/diagnoses/javamem.md) |
| `memory classify` | 综合归类（不明时兜底） | 路由见 [memory-routing.md](./references/memory-routing.md) |

### IO 域

| 子命令 | 能力 | 专文 |
|--------|------|------|
| `io iofsstat` | IO 大盘（磁盘统计） | [iofsstat.md](./references/diagnoses/iofsstat.md) |
| `io iodiagnose` | IO 深度（慢 IO、延迟） | [iodiagnose.md](./references/diagnoses/iodiagnose.md) |

### 网络域

| 子命令 | 能力 | 专文 |
|--------|------|------|
| `net packetdrop` | 丢包（rtrace） | [packetdrop.md](./references/diagnoses/packetdrop.md) |
| `net netjitter` | 抖动（时延波动） | [netjitter.md](./references/diagnoses/netjitter.md) |

### 负载域

| 子命令 | 能力 | 专文 |
|--------|------|------|
| `load delay` | 调度延迟（nosched） | [delay.md](./references/diagnoses/delay.md) |
| `load loadtask` | 系统负载 | [loadtask.md](./references/diagnoses/loadtask.md) |

## 快速开始

```bash
cd <sysom-diagnosis>
./shared/scripts/osops memory classify                                          # 本机归类
./shared/scripts/osops memory memgraph                                          # 本机内存大图
./shared/scripts/osops memory memgraph --deep-diagnosis --channel ecs --timeout 300  # 远程内存专项
./shared/scripts/osops io iofsstat --channel ecs --timeout 300                  # IO 大盘
./shared/scripts/osops net packetdrop --channel ecs --region cn-hangzhou --instance i-xxx  # 丢包诊断
./shared/scripts/osops load delay --channel ecs --params '{"duration":30}'      # 调度延迟
```

其它实例加 `--region <id> --instance <i-xxx>`。首次使用先 `./shared/scripts/init.sh`。

## 远程 OpenAPI 三要素

| 要素 | 说明 |
|------|------|
| 身份 | AK/SK 或实例 RAM Role |
| 策略 | `AliyunSysomFullAccess` |
| 开通与 SLR | 控制台开通 SysOM；SLR 见 [service-linked-role-subaccount.md](./shared/references/service-linked-role-subaccount.md) |

## 关键路径索引

| 需求 | 文档 |
|------|------|
| 内存意图→子命令映射 | [memory-routing.md](./references/memory-routing.md) |
| IO/网络/负载路由 | [non-memory-routing.md](./references/non-memory-routing.md) |
| 远程调用契约 / CLI 选项 / 元数据 | [invoke-diagnosis.md](./references/invoke-diagnosis.md) |
| 权限 / 凭证 / precheck | [permission-guide.md](./references/permission-guide.md) → [openapi-permission-guide.md](./shared/references/openapi-permission-guide.md) |
| 输出信封格式 | [output-format.md](./references/output-format.md) |
| Agent 行为约定 | [agent-conventions.md](./references/agent-conventions.md) |
| 各诊断 params 字段 | [diagnoses/README.md](./references/diagnoses/README.md) |
