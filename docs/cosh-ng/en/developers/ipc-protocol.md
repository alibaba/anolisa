# ws-ckpt IPC Protocol

## Overview

cosh-ng communicates with the ws-ckpt daemon via Unix Domain Socket to manage workspace snapshots.
Communication uses a frame format of **bincode serialization + 4-byte little-endian length prefix**.

## Architecture

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

The client implementation is in `crates/cosh-platform/src/checkpoint.rs` (`CkptClient`),
and type definitions are in `crates/cosh-types/src/checkpoint.rs`.

## Frame Format

Each message consists of two parts:

```
┌──────────────────┬───────────────────────────────┐
│ 4-byte LE u32    │ bincode-encoded enum payload   │
│ (payload length) │ (WsCkptRequest / Response)     │
└──────────────────┴───────────────────────────────┘
```

- Length prefix: little-endian unsigned 32-bit integer representing the byte count of the subsequent bincode payload
- Maximum response limit: 64 MiB (prevents OOM)
- Default timeout: 5000ms (configurable via `CkptClient::with_timeout()`)

## Request Types (WsCkptRequest)

bincode serializes enums by variant index (first variant = index 0). **Variant order is the binary contract and must not be reordered.**

| Index | Variant | Description |
|-------|---------|-------------|
| 0 | `Init { workspace }` | Initialize a workspace |
| 1 | `Checkpoint { workspace, id, message, metadata, pin }` | Create a snapshot |
| 2 | `Rollback { workspace, to }` | Rollback to a specified snapshot |
| 3 | `Delete { workspace, snapshot, force }` | Delete a snapshot |
| 4 | `List { workspace, format }` | List snapshots |
| 5 | `Diff { workspace, from, to }` | Diff between two snapshots |
| 6 | `Status { workspace }` | Query status |
| 7 | `Cleanup { workspace, keep }` | Clean up old snapshots |
| 8 | `Config` | Get daemon configuration |
| 9 | `ReloadConfig` | Reload configuration |
| 10 | `Recover { workspace }` | Recover a workspace |
| 11 | `HealthAdvisory` | Health check |

## Response Types (WsCkptResponse)

| Variant | Corresponding Request | Key Fields |
|---------|----------------------|------------|
| `InitOk { ws_id }` | Init | Workspace ID |
| `CheckpointOk { snapshot_id }` | Checkpoint | Snapshot ID |
| `RollbackOk { from, to }` | Rollback | Rollback source and target |
| `DeleteOk { target }` | Delete | Deleted snapshot identifier |
| `Error { code, message }` | Any | Error code + human-readable description |
| `ListOk { snapshots }` | List | `Vec<SnapshotEntry>` |
| `DiffOk { changes }` | Diff | `Vec<DiffEntry>` |
| `StatusOk { report }` | Status | `StatusReport` |
| `CleanupOk { removed }` | Cleanup | List of removed snapshot IDs |
| `ConfigOk { config }` | Config | `ConfigReport` |
| `ReloadConfigOk` | ReloadConfig | No payload |
| `CheckpointSkipped { reason }` | Checkpoint | Skip reason (e.g., no changes) |
| `RecoverOk { workspace }` | Recover | Recovered workspace path |
| `HealthAdvisoryOk { ... }` | HealthAdvisory | Over-limit workspace count, disk usage |

## Error Codes (WsCkptErrorCode)

| Index | Variant | Description |
|-------|---------|-------------|
| 0 | `WorkspaceNotFound` | Workspace does not exist |
| 1 | `SnapshotNotFound` | Snapshot does not exist |
| 2 | `AlreadyInitialized` | Workspace already initialized |
| 3 | `BtrfsError` | Btrfs operation error |
| 4 | `IoError` | I/O error |
| 5 | `InvalidPath` | Invalid path |
| 6 | `ConfirmationRequired` | Confirmation needed (e.g., deleting a pinned snapshot) |
| 7 | `InternalError` | Internal error |
| 8 | `SnapshotAlreadyExists` | Snapshot ID conflict |
| 9 | `WriteLockConflict` | Write lock conflict |
| 10 | `DiskSpaceInsufficient` | Insufficient disk space |

## Client Usage

```rust
use cosh_platform::checkpoint::CkptClient;

// Default path /run/ws-ckpt/ws-ckpt.sock
let client = CkptClient::default_path();

// Or specify path and timeout
let client = CkptClient::with_timeout("/custom/path.sock", 10000);

// Health check
if !client.is_available() {
    eprintln!("ws-ckpt daemon not running");
}

// Operation examples
let result = client.create("/home/user/project", "snap-001", Some("initial"), None, false)?;
let list = client.list(Some("/home/user/project"))?;
let restored = client.restore("/home/user/project", "snap-001")?;
```

## Key Constraints

| Constraint | Description |
|------------|-------------|
| Variant order is immutable | bincode serializes enums by index; reordering breaks the wire format |
| New additions append only | New Request/Response variants can only be added at the end |
| Types must stay in sync | Definitions in `cosh-types` must exactly match `ws-ckpt-common` |
| Timeout handling | Client sets read/write timeouts to avoid blocking when daemon is unresponsive |
| Length limit | Responses exceeding 64 MiB are treated as anomalous; connection is dropped immediately |
| Socket path | Default `/run/ws-ckpt/ws-ckpt.sock`; overridable via environment variable or CLI argument |

## Test Verification

```bash
cd src/cosh-ng

# bincode round-trip serialization tests
cargo test --locked -p cosh-types -- checkpoint

# Variant index contract tests
cargo test --locked -p cosh-types test_request_bincode_variant_index

# CkptClient unit tests (no running daemon required)
cargo test --locked -p cosh-platform -- checkpoint
```
