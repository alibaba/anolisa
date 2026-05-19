---
name: ws-ckpt
description: >
  工作区快照管理。用户说"保存一下"、"存个快照"时创建 checkpoint;
  说"回滚"、"撤销"、"恢复到之前"时 rollback;说"删掉快照"时 delete;
  说"看看快照"、"有哪些快照"时 list;说"查看快照状态"、"查看快照剩余空间"时 status。
---

# ws-ckpt 工作区快照管理

基于 btrfs COW 快照,为任意工作区提供微秒级 checkpoint/rollback。

## 工作区路径（关键 — 必须遵守）

⚠️ **绝对禁止猜测或推断工作区路径。**

ws-ckpt 的所有命令都需要 `-w <workspace>` 指定工作区路径。执行任何命令前，必须按以下顺序确定 `-w` 参数：

1. 用户在**当前消息中明确给出**了路径 → 直接使用
2. 否则 → **必须向用户询问**："请提供工作区路径（传给 `-w` 的目录）"，拿到回复后再执行

不得从环境变量、默认路径、或任何隐含上下文中猜测。

确定后，本次会话内复用同一个 workspace 路径，不要重复询问。

## 触发规则

| 用户说 | 执行命令 | 说明 |
|--------|----------|------|
| "保存一下"、"存个快照"、"checkpoint"、"备份当前状态" | `checkpoint` | 创建快照 |
| "回滚"、"撤销"、"恢复到之前"、"rollback"、"改坏了" | `rollback` | 回滚到指定快照 |
| "删掉快照"、"清理快照"、"delete snapshot" | `delete` | 删除指定快照 |
| "看看快照"、"有哪些快照"、"list"、"列一下" | `list` | 列出快照 |
| "状态"、"空间"、"status"、"工作区怎么样" | `status` | 查看工作区状态 |

## 命令用法

### checkpoint — 创建快照

```bash
ws-ckpt checkpoint -w <workspace> -i <id> [-m <message>]
```

- `-w`:工作区路径(必填)
- `-i`:快照 ID,自定义名称,同一工作区内唯一(必填)
- `-m`:快照描述(可选)

```bash
ws-ckpt checkpoint -w <path-to-workspace> -i before-refactor -m "重构前备份"
```

### rollback — 回滚到快照

```bash
ws-ckpt rollback -w <workspace> -s <snapshot>
```

- `-w`:工作区路径(快照 ID 全局唯一时可省略)
- `-s`:目标快照 ID(必填)

```bash
ws-ckpt rollback -s before-refactor
ws-ckpt rollback -w <path-to-workspacee -s before-refactor
```

### delete — 删除快照

```bash
ws-ckpt delete -s <snapshot> [--force] [-w <workspace>]
```

- `-s`:要删除的快照 ID(必填)
- `--force`:跳过确认
- `-w`:快照 ID 跨工作区重复时必须指定

```bash
ws-ckpt delete -s old-snap --force
```

### list — 列出快照

```bash
ws-ckpt list [-w <workspace>] [--format table|json]
```

- 省略 `-w` 列出所有工作区的快照

```bash
ws-ckpt list
ws-ckpt list -w <path-to-workspace
ws-ckpt list --format json
```

### status — 查看状态

```bash
ws-ckpt status [-w <workspace>]
```

- 省略 `-w` 查看全局状态

```bash
ws-ckpt status
ws-ckpt status -w <path-to-workspace
```

## 注意事项

- checkpoint 用 `-i` 指定快照 ID;rollback 和 delete 用 `-s` 指定快照 ID,不要混淆
- daemon 必须运行中(`systemctl status ws-ckpt` 确认),否则所有命令报连接错误
- 执行破坏性操作前务必先 checkpoint
