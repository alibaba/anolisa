# iofsstat（IO 流量 / 磁盘统计大盘）

> 参数说明依据 SysOM 诊断侧脚本与 OpenAPI 行为整理。

## 功能概述

在目标实例执行 **`sysak iofsstat`**，采集 IO 统计并输出 JSON，用于 **磁盘/块设备 IO 统计大盘**。

## 何时选用（Agent）

- 需要 **磁盘/块设备 IO 统计大盘**，先看 IO 概况再决定是否做 **iodiagnose**。

## `params` 字段

| 字段 | 类型 | 必填 | 含义 | 默认 | 备注 |
|------|------|------|------|------|------|
| `region` | string | 是* | 地域 | — | `--region` |
| `instance` | string | 是* | 实例 ID | — | `--instance` |
| `timeout` | string/int | 否 | 采样时长（秒） | `"15"` | ≤0 时置 15；**>30 时置 30** |
| `disk` | string | 否 | 块设备名（如 `vda`） | `""` | 非空时命令追加 `-d <disk>` |

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
./shared/scripts/osops io iofsstat --channel ecs \
  --region cn-hangzhou --instance i-xxx \
  --params '{"timeout":"20","disk":"vda"}'
```
