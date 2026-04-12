# 非内存域：IO / 网络 / 负载子命令路由

与 [memory-routing.md](./memory-routing.md) 并列；IO/网络/负载子命令本身即远程专项入口，内建环境检查。

## 推荐入口

子命令名与 **`service_name`** 一致；`--channel`/`--params`/`--region`/`--instance`/轮询选项等与 [invoke-diagnosis.md](./invoke-diagnosis.md) 中专项约定一致。

| 子系统 | 子命令 | 专文 |
|--------|--------|------|
| **`io`** | `iofsstat` | [iofsstat.md](./diagnoses/iofsstat.md) |
| **`io`** | `iodiagnose` | [iodiagnose.md](./diagnoses/iodiagnose.md) |
| **`net`** | `packetdrop` | [packetdrop.md](./diagnoses/packetdrop.md) |
| **`net`** | `netjitter` | [netjitter.md](./diagnoses/netjitter.md) |
| **`load`** | `delay` | [delay.md](./diagnoses/delay.md) |
| **`load`** | `loadtask` | [loadtask.md](./diagnoses/loadtask.md) |

示例：

```bash
./shared/scripts/osops io iofsstat --channel ecs --timeout 300
./shared/scripts/osops net packetdrop --channel ecs --region cn-hangzhou --instance i-xxx
```

## 与内存域 `memgraph` 的交叉（延迟 / socket 队列）

`net packetdrop` / `net netjitter` 主要覆盖丢包、rtrace、抖动阈值等；**不**提供整机内存 + socket 队列 + TCP 内存占用的 SysOM memgraph 视图。

当用户描述 **网络延迟、卡顿**，且本机 `ss` 等已观察到 **Send-Q / Recv-Q 积压**（含 `127.0.0.1` 上应用间 TCP），即使 packetdrop / netjitter 远程结果正常，仍建议补充：

```bash
./shared/scripts/osops memory memgraph --deep-diagnosis --channel ecs --timeout 300
```

本机 ECS 不传 `--region`/`--instance`；规则同 [invoke-diagnosis.md](./invoke-diagnosis.md)。详见 [memgraph.md](./diagnoses/memgraph.md)。
