# ws-ckpt IPC 协议

## 概述

cosh-ng 通过 Unix Domain Socket 与 ws-ckpt 守护进程通信，实现工作空间快照管理。
通信使用 **bincode 序列化 + 4 字节小端长度前缀** 的帧格式。

## 架构

```
cosh-cli / cosh-core          ws-ckpt daemon
     │                              │
     │  Unix socket                 │
     │  /run/ws-ckpt/ws-ckpt.sock   │
     │─────────────────────────────→│
     │  [4B LE len][bincode req]    │
     │                              │
     │←─────────────────────────────│
     │  [4B LE len][bincode resp]   │
```

客户端实现位于 `crates/cosh-platform/src/checkpoint.rs`（`CkptClient`），
类型定义位于 `crates/cosh-types/src/checkpoint.rs`。

## 帧格式

每个消息由两部分组成：

```
┌──────────────────┬───────────────────────────────┐
│ 4 字节 LE u32    │ bincode 编码的枚举载荷        │
│ (payload 长度)   │ (WsCkptRequest / Response)    │
└──────────────────┴───────────────────────────────┘
```

- 长度前缀：小端序无符号 32 位整数，表示后续 bincode 载荷的字节数
- 最大响应限制：64 MiB（防止 OOM）
- 默认超时：5000ms（可通过 `CkptClient::with_timeout()` 配置）

## 请求类型（WsCkptRequest）

bincode 按枚举变体索引序列化（第一个变体 = index 0）。**变体顺序即二进制契约，不可重排。**

| 索引 | 变体 | 说明 |
|------|------|------|
| 0 | `Init { workspace }` | 初始化工作空间 |
| 1 | `Checkpoint { workspace, id, message, metadata, pin }` | 创建快照 |
| 2 | `Rollback { workspace, to }` | 回滚到指定快照 |
| 3 | `Delete { workspace, snapshot, force }` | 删除快照 |
| 4 | `List { workspace, format }` | 列出快照 |
| 5 | `Diff { workspace, from, to }` | 两个快照间的差异 |
| 6 | `Status { workspace }` | 查询状态 |
| 7 | `Cleanup { workspace, keep }` | 清理旧快照 |
| 8 | `Config` | 获取守护进程配置 |
| 9 | `ReloadConfig` | 重新加载配置 |
| 10 | `Recover { workspace }` | 恢复工作空间 |
| 11 | `HealthAdvisory` | 健康检查 |

## 响应类型（WsCkptResponse）

| 变体 | 对应请求 | 关键字段 |
|------|----------|----------|
| `InitOk { ws_id }` | Init | 工作空间 ID |
| `CheckpointOk { snapshot_id }` | Checkpoint | 快照 ID |
| `RollbackOk { from, to }` | Rollback | 回滚源和目标 |
| `DeleteOk { target }` | Delete | 被删除的快照标识 |
| `Error { code, message }` | 任意 | 错误码 + 人类可读描述 |
| `ListOk { snapshots }` | List | `Vec<SnapshotEntry>` |
| `DiffOk { changes }` | Diff | `Vec<DiffEntry>` |
| `StatusOk { report }` | Status | `StatusReport` |
| `CleanupOk { removed }` | Cleanup | 被移除的快照 ID 列表 |
| `ConfigOk { config }` | Config | `ConfigReport` |
| `ReloadConfigOk` | ReloadConfig | 无载荷 |
| `CheckpointSkipped { reason }` | Checkpoint | 跳过原因（如无变更） |
| `RecoverOk { workspace }` | Recover | 恢复的工作空间路径 |
| `HealthAdvisoryOk { ... }` | HealthAdvisory | 超限工作空间数、磁盘用量 |

## 错误码（WsCkptErrorCode）

| 索引 | 变体 | 说明 |
|------|------|------|
| 0 | `WorkspaceNotFound` | 工作空间不存在 |
| 1 | `SnapshotNotFound` | 快照不存在 |
| 2 | `AlreadyInitialized` | 工作空间已初始化 |
| 3 | `BtrfsError` | Btrfs 操作错误 |
| 4 | `IoError` | I/O 错误 |
| 5 | `InvalidPath` | 非法路径 |
| 6 | `ConfirmationRequired` | 需要确认（如删除 pinned 快照） |
| 7 | `InternalError` | 内部错误 |
| 8 | `SnapshotAlreadyExists` | 快照 ID 冲突 |
| 9 | `WriteLockConflict` | 写锁冲突 |
| 10 | `DiskSpaceInsufficient` | 磁盘空间不足 |

## 客户端使用

```rust
use cosh_platform::checkpoint::CkptClient;

// 默认路径 /run/ws-ckpt/ws-ckpt.sock
let client = CkptClient::default_path();

// 或指定路径和超时
let client = CkptClient::with_timeout("/custom/path.sock", 10000);

// 健康检查
if !client.is_available() {
    eprintln!("ws-ckpt daemon not running");
}

// 操作示例
let result = client.create("/home/user/project", "snap-001", Some("initial"), None, false)?;
let list = client.list(Some("/home/user/project"))?;
let restored = client.restore("/home/user/project", "snap-001")?;
```

## 关键约束

| 约束 | 说明 |
|------|------|
| 变体顺序不可变 | bincode 使用索引序列化枚举，重排即破坏线格式 |
| 新增只能追加 | 新的 Request/Response 变体只能在末尾添加 |
| 类型必须同步 | `cosh-types` 中的定义必须与 `ws-ckpt-common` 完全一致 |
| 超时处理 | 客户端对 read/write 设置超时，避免守护进程无响应时阻塞 |
| 长度限制 | 响应超过 64 MiB 视为异常，立即断开 |
| socket 路径 | 默认 `/run/ws-ckpt/ws-ckpt.sock`，可通过环境变量或 CLI 参数覆盖 |

## 测试验证

```bash
cd src/cosh-ng

# bincode 往返序列化测试
cargo test --locked -p cosh-types -- checkpoint

# 变体索引契约测试
cargo test --locked -p cosh-types test_request_bincode_variant_index

# CkptClient 单元测试（不需要运行的守护进程）
cargo test --locked -p cosh-platform -- checkpoint
```
