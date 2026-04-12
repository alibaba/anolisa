# loadtask（系统负载诊断）

> 参数说明依据 SysOM 诊断侧脚本与 OpenAPI 行为整理。

## 功能概述

在目标实例执行 **`sysak -g loadtask`**，读取 `summary.json` 与临时日志，用于 **load average 高、CPU 排队、负载任务分析**。

## 何时选用（Agent）

- **load average 高、CPU 排队、负载任务分析**。

## `params` 字段

| 字段 | 类型 | 必填 | 含义 | 默认 | 备注 |
|------|------|------|------|------|------|
| `region` | string | 是* | 地域 | — | `--region` |
| `instance` | string | 是* | 实例 ID | — | `--instance` |

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
./shared/scripts/osops load loadtask --channel ecs \
  --region cn-hangzhou --instance i-xxx --timeout 300
```
