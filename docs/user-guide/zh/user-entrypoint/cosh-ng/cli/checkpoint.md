# 工作区快照

`cosh-cli checkpoint` 子系统管理工作区快照，通过 Unix socket 与 ws-ckpt 守护
进程通信。快照使 Agent 可以在执行高风险操作前保存状态，失败时快速回滚。

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
| `cosh-cli checkpoint delete` | 删除快照 |
| `cosh-cli checkpoint diff` | 对比两个快照 |

## create

创建一个带有标识和说明的快照。

```bash
cosh-cli checkpoint create --workspace /home/agent/project --id step-042 -m "before refactor"
```

输出：

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

## restore

恢复工作区到指定快照状态。

```bash
cosh-cli checkpoint restore step-040 --workspace /home/agent/project
```

## list

列出工作区中的所有快照。

```bash
cosh-cli checkpoint list --workspace /home/agent/project
```

## diff

对比两个快照之间的差异。

```bash
cosh-cli checkpoint diff --workspace /home/agent/project --from step-040 --to step-042
```

## init

初始化工作区的快照管理。

```bash
cosh-cli checkpoint init --workspace /home/agent/project
```

## delete

删除指定快照。

```bash
cosh-cli checkpoint delete --snapshot step-042
```

## status

查看 ws-ckpt 守护进程连接状态。

```bash
cosh-cli checkpoint status
```

## IPC 协议

checkpoint 命令通过 Unix socket 与 ws-ckpt 守护进程通信，使用 bincode 序列化 +
4 字节小端长度前缀帧格式。详见开发者文档 [IPC 协议](../../../../../developer-guide/zh/cosh-ng/ipc-protocol.md)。

## 典型 Agent 工作流

```
1. cosh-cli checkpoint create --id pre-action -m "safe point"
2. 执行高风险操作（文件修改、服务重启等）
3. 验证操作结果
4. 若失败 → cosh-cli checkpoint restore pre-action
5. 若成功 → 继续下一步
```
