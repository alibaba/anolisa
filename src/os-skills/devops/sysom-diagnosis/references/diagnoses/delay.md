# delay（调度延迟 / nosched 诊断）

> 参数说明依据 SysOM 诊断侧脚本与 OpenAPI 行为整理。OpenAPI **`service_name` 为 `delay`**；实现使用 **`sysak -g nosched`**（调度延迟，非 ICMP RTT）。

## 功能概述

- **实时**（`is_history` 为 0）：执行 **`sysak -g nosched`**，阈值 **`threshold`**（毫秒）、持续 **`duration`**（秒）。
- **历史**（`is_history` 非 0）：按时间戳查询（**≤1 小时、7 天内**）。

## 何时选用（Agent）

- **调度延迟、runqueue、nosched 可观测的卡顿**。
- **不要**当作「ICMP 网络时延」唯一手段（产品配置注释可能写「网络延迟」，以本实现为准）。

## `params` 字段

| 字段 | 类型 | 必填 | 含义 | 默认 | 备注 |
|------|------|------|------|------|------|
| `region` | string | 是* | 地域 | — | `--region` |
| `instance` | string | 是* | 实例 ID（去 `:port`） | — | `--instance` |
| `is_history` | int | 否 | 是否历史 | `0` | 非 0 走历史查询 |
| `anomaly_start` / `anomaly_end` | int | 条件 | Unix 秒 | `0` | 历史模式必填 |
| `duration` | int | 否 | 实时采集秒数 | `20` | **须 ≤60** |
| `threshold` | int | 否 | 阈值 **毫秒** | `20` | 传给 `nosched -t` |

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
./shared/scripts/osops load delay --channel ecs \
  --region cn-hangzhou --instance i-xxx \
  --params '{"duration":30,"threshold":20}'
```
