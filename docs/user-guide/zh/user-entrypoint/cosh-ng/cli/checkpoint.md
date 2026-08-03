# 工作区快照

[English](../../../../en/user-entrypoint/cosh-ng/cli/checkpoint.md)

`cosh-cli checkpoint` 通过 Unix socket 调用 ws-ckpt 守护进程，管理工作区快照。
Agent 可以在高风险操作前保存状态，失败后再恢复到安全位置。

## 前置条件

checkpoint 命令需要运行中的 ws-ckpt 守护进程。如未运行，命令返回
`CheckpointDaemonUnavailable` 错误。

## 命令列表

| 命令 | 说明 |
|------|------|
| `cosh-cli checkpoint create` | 创建快照 |
| `cosh-cli checkpoint restore <id>` | 恢复到指定快照 |
| `cosh-cli checkpoint list` | 列出所有快照 |
| `cosh-cli checkpoint status` | 查看守护进程状态 |
| `cosh-cli checkpoint init` | 初始化工作区 |
| `cosh-cli checkpoint recover` | 恢复工作区 checkpoint 元数据 |
| `cosh-cli checkpoint delete` | 删除快照 |
| `cosh-cli checkpoint diff` | 对比两个快照 |
| `cosh-cli checkpoint cleanup` | 保留限定数量的快照 |

## 创建快照

创建一个带有标识和说明的快照。

```bash
cosh-cli checkpoint create --workspace /home/agent/project --id step-042 -m "before refactor"
```

输出示例

```json
{
  "ok": true,
  "data": {
    "checkpoint_id": "step-042",
    "step": 42
  },
  "meta": { "subsystem": "checkpoint", "duration_ms": 150, "distro": "alinux", "dry_run": false }
}
```

## 恢复快照

恢复工作区到指定快照状态。

```bash
cosh-cli checkpoint restore step-040 --workspace /home/agent/project
```

## 列出快照

列出工作区中的所有快照。

```bash
cosh-cli checkpoint list --workspace /home/agent/project
```

## 比较快照

对比两个快照之间的差异。

```bash
cosh-cli checkpoint diff --workspace /home/agent/project --from step-040 --to step-042
```

## 初始化工作区

初始化工作区的快照管理。

```bash
cosh-cli checkpoint init --workspace /home/agent/project
```

## 删除快照

删除指定快照。

```bash
cosh-cli checkpoint delete --snapshot step-042
```

多个工作区可能存在相同的快照 ID，此时请使用 `--workspace <path>`。后端支持时，
`--force` 可以跳过确认。

## 恢复快照元数据

恢复已初始化工作区的快照元数据。

```bash
cosh-cli checkpoint recover --workspace /home/agent/project
```

## 清理旧快照

请求守护进程只保留指定数量的快照。

```bash
cosh-cli checkpoint cleanup --workspace /home/agent/project --keep 20
```

快照修改不支持 `--dry-run`。恢复、删除或清理前，请检查工作区、快照 ID、固定状态和
守护进程策略。

## 查看守护进程状态

查看 ws-ckpt 守护进程连接状态。

```bash
cosh-cli checkpoint status
```

## IPC 协议

快照命令通过 Unix socket 与 ws-ckpt 守护进程通信，使用 bincode 序列化和 4 字节小端
长度前缀。详见开发者文档 [IPC 协议](../../../../../developer-guide/zh/cosh-ng/ipc-protocol.md)。

## 典型 Agent 工作流

```
1. cosh-cli checkpoint create --workspace /path/to/workspace --id pre-action -m "safe point"
2. 执行高风险操作（文件修改、服务重启等）
3. 验证操作结果
4. 若失败 → cosh-cli checkpoint restore pre-action --workspace /path/to/workspace
5. 若成功 → 继续下一步
```
