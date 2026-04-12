# iodiagnose（IO 深度 / ioMonitor 诊断）

> 参数说明依据 SysOM 诊断侧脚本与 OpenAPI 行为整理。

## 功能概述

在目标实例执行 **`sysak ioMonitor`**（带固定 yaml、日志路径与诊断开关），用于 **IO 延迟、iowait、burst** 等深度采集。

## 何时选用（Agent）

- **IO 慢、iowait 高** 等需要 ioMonitor 一键采集时（在 iofsstat 大盘之后）。

## `params` 字段

| 字段 | 类型 | 必填 | 含义 | 默认 | 备注 |
|------|------|------|------|------|------|
| `region` | string | 是* | 地域 | — | `--region` |
| `instance` | string | 是* | 实例 ID | — | `--instance` |
| `timeout` | string/int | 否 | ioMonitor 采集时长（秒） | `"30"` | 解析失败或非正数时用 30；**上限 300**，超出则回退为 30 |

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
./shared/scripts/osops io iodiagnose --channel ecs \
  --region cn-hangzhou --instance i-xxx --params '{"timeout":60}'
```

长采集时同步增大 `--timeout`（轮询总超时）。
