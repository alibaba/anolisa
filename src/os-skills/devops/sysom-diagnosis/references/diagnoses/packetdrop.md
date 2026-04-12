# packetdrop（网络丢包诊断）

> 参数说明依据 SysOM 诊断侧脚本与 OpenAPI 行为整理。

## 功能概述

- **实时**（`is_history` 为假，默认）：在实例上执行 **`sysak -g rtrace --drop-unity`**。
- **历史**（`is_history` 为真）：在服务端聚合 Prometheus 等指标做离线分析。

## 何时选用（Agent）

- **丢包、重传、网卡侧 rtrace**；或需 **历史区间** 分析时开 `is_history`。

## 与 `memory memgraph` 的互补

本专项**不**覆盖 socket 队列全景、TCP 内存等；`packetdrop` 无异常不能排除应用背压导致 Send-Q 积压或内核 TCP 内存与延迟的关联。出现此类线索时请加做 `memory memgraph --deep-diagnosis`。详见 [non-memory-routing.md](../non-memory-routing.md) 与 [memgraph.md](./memgraph.md)。

## `params` 字段

| 字段 | 类型 | 必填 | 含义 | 默认 | 备注 |
|------|------|------|------|------|------|
| `region` | string | 是* | 地域 | — | `--region` |
| `instance` | string | 是* | 实例 ID（可含 `:port`，实现取 host） | — | `--instance` |
| `is_history` | bool | 否 | 是否走历史/离线分析 | `false` | `true` 时用 `anomaly_start`/`anomaly_end` |
| `anomaly_start` | number | 条件 | 历史查询起始时间戳（秒） | `0` | |
| `anomaly_end` | number | 条件 | 历史结束时间戳（秒） | `0` | |

\* CLI `--region`/`--instance` 可合并写入 params；本机 ECS 省略时由元数据补全。

## 平台约束

| 项 | 值 |
|----|-----|
| support_channel | **ecs \| eflo** |
| support_mode | **仅 node** |
| sysak 最低 | **`3.6.0-1`** |

## 建议用法

**当前目录**：见 [agent-conventions.md](../agent-conventions.md)（在 `sysom-diagnosis/` 下使用 `./shared/scripts/osops`）。

```bash
./shared/scripts/osops net packetdrop --channel ecs \
  --region cn-hangzhou --instance i-xxx
```

历史模式加 `--params '{"is_history":true,"anomaly_start":...,"anomaly_end":...}'`。
