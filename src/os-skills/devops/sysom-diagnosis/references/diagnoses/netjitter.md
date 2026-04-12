# netjitter（网络抖动诊断）

> 参数说明依据 SysOM 诊断侧脚本与 OpenAPI 行为整理。

## 功能概述

- **实时**（`is_history` 为 0/false）：执行 **`sysak rtrace --jitter-unity`**，阈值 **`threshold`**（毫秒）、持续 **`duration`**（秒）。
- **历史**（`is_history` 非 0）：按 **`anomaly_start`/`anomaly_end`** 查本地表（时间窗 **≤1 小时、7 天内**）。

## 何时选用（Agent）

- **网络抖动、时延波动** 类问题。

## 与 `memory memgraph` 的互补

本专项**不**采集 socket 发送/接收队列积压、TCP 内存占用等 memgraph 侧数据。若 `netjitter` 正常但用户仍觉延迟，且 `ss` 显示 Send-Q/Recv-Q 偏大，应再跑 `memory memgraph --deep-diagnosis`。详见 [non-memory-routing.md](../non-memory-routing.md) 与 [memgraph.md](./memgraph.md)。

## `params` 字段

| 字段 | 类型 | 必填 | 含义 | 默认 | 备注 |
|------|------|------|------|------|------|
| `region` | string | 是* | 地域 | — | `--region` |
| `instance` | string | 是* | 实例 ID（去 `:port`） | — | `--instance` |
| `is_history` | int/bool | 否 | 是否历史模式 | `0` | 非 0 走历史查询 |
| `anomaly_start` | int | 条件 | 起始 Unix 秒 | `0` | 历史必填 |
| `anomaly_end` | int | 条件 | 结束 Unix 秒 | `0` | 历史必填 |
| `duration` | int | 否 | 实时采集持续 **秒** | `20` | **须 ≤60** |
| `threshold` | int | 否 | 抖动阈值 **毫秒** | `10` | 传入 `rtrace --jitter-unity -t` |

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
./shared/scripts/osops net netjitter --channel ecs \
  --region cn-hangzhou --instance i-xxx \
  --params '{"duration":30,"threshold":10}'
```
