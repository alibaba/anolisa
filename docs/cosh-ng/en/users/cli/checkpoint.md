# Workspace Checkpoints

The `cosh-cli checkpoint` subsystem manages workspace snapshots by communicating with the ws-ckpt daemon via Unix socket. Snapshots enable Agents to save state before executing high-risk operations and quickly rollback on failure.

## Prerequisites

Checkpoint commands require a running ws-ckpt daemon. If not running, commands return `CheckpointDaemonUnavailable` error.

## Command List

| Command | Description |
|---------|-------------|
| `cosh-cli checkpoint create` | Create snapshot |
| `cosh-cli checkpoint restore <id>` | Restore to specified snapshot |
| `cosh-cli checkpoint list` | List all snapshots |
| `cosh-cli checkpoint status` | Check daemon status |
| `cosh-cli checkpoint init` | Initialize workspace |
| `cosh-cli checkpoint delete` | Delete snapshot |
| `cosh-cli checkpoint diff` | Compare two snapshots |

## create

Create a snapshot with an identifier and description.

```bash
cosh-cli checkpoint create --workspace /home/agent/project --id step-042 -m "before refactor"
```

Output:

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

Restore workspace to a specified snapshot state.

```bash
cosh-cli checkpoint restore step-040 --workspace /home/agent/project
```

## list

List all snapshots in the workspace.

```bash
cosh-cli checkpoint list --workspace /home/agent/project
```

## diff

Compare differences between two snapshots.

```bash
cosh-cli checkpoint diff --workspace /home/agent/project --from step-040 --to step-042
```

## init

Initialize checkpoint management for a workspace.

```bash
cosh-cli checkpoint init --workspace /home/agent/project
```

## delete

Delete a specified snapshot.

```bash
cosh-cli checkpoint delete --snapshot step-042
```

## status

Check ws-ckpt daemon connection status.

```bash
cosh-cli checkpoint status
```

## IPC Protocol

Checkpoint commands communicate with the ws-ckpt daemon via Unix socket, using bincode serialization + 4-byte little-endian length prefix frame format. See developer documentation [IPC Protocol](../../developers/ipc-protocol.md).

## Typical Agent Workflow

```
1. cosh-cli checkpoint create --id pre-action -m "safe point"
2. Execute high-risk operation (file modifications, service restarts, etc.)
3. Verify operation results
4. If failed → cosh-cli checkpoint restore pre-action
5. If successful → proceed to next step
```
