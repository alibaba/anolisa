/**
 * Tool handler functions for the ws-ckpt OpenClaw plugin.
 *
 * Each handle* function implements the business logic for one tool.
 * They access shared state via the pluginState singleton.
 */

import { CommandExecutor } from "./commands.js";
import { mapErrorToLLMMessage } from "./btrfs-manager.js";
import type { AgentToolResult } from "../types-shim.js";
import { pluginState, UNAVAILABLE_MSG, cwdInsideWorkspace, CWD_INSIDE_WORKSPACE_REASON } from "./state.js";
import { daemonAutoCleanup } from "./config.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

export function textToolResult(
  text: string,
  isError?: boolean,
): AgentToolResult {
  return {
    content: [{ type: "text", text }],
    details: isError ? { status: "failed" } : undefined,
  };
}

// ---------------------------------------------------------------------------
// Handler functions
// ---------------------------------------------------------------------------

export async function handleCheckpoint(
  argsStr?: string,
): Promise<{ text: string; isError: boolean }> {
  if (!pluginState.manager || !pluginState.environmentReady) {
    return { text: UNAVAILABLE_MSG, isError: true };
  }
  // pin is no longer exposed to plugin users (auto-cleanup disabled).
  const args = argsStr ? JSON.parse(argsStr) : {};
  const id = args.id;
  if (!id) {
    return { text: "Missing required parameter: id", isError: true };
  }
  const message = args.message?.trim() || "manual checkpoint";
  const explicitWs = (args.workspace as string | undefined)?.trim();

  // Explicit workspace bypasses the manager (and its workspace-bound cache),
  // mirroring the handleDelete pattern.
  if (explicitWs) {
    if (cwdInsideWorkspace(explicitWs)) {
      return { text: CWD_INSIDE_WORKSPACE_REASON, isError: true };
    }
    try {
      const executor = new CommandExecutor();
      const output = await executor.checkpoint(explicitWs, id, { message });
      if (output.exitCode !== 0) {
        return { text: mapErrorToLLMMessage(output.stderr, { id }), isError: true };
      }
      if (output.stdout && (output.stdout.includes('Skipped') || output.stdout.includes('Empty workspace'))) {
        return { text: 'Empty workspace, no snapshot created.', isError: false };
      }
      return { text: `Checkpoint created: ${id}`, isError: false };
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      return { text: `Checkpoint error: ${msg}`, isError: true };
    }
  }

  const ws = pluginState.resolvedConfig?.workspace;
  if (ws && cwdInsideWorkspace(ws)) {
    return { text: CWD_INSIDE_WORKSPACE_REASON, isError: true };
  }

  const result = await pluginState.manager.createCheckpoint({
    id,
    message,
  });
  if (result.skipped) {
    return { text: result.reason ?? "Empty workspace, no snapshot created.", isError: false };
  }
  return { text: result.message, isError: !result.success };
}

export async function handleRollback(
  target?: string,
  workspace?: string,
): Promise<{ text: string; isError: boolean }> {
  if (!pluginState.manager || !pluginState.environmentReady) {
    return { text: UNAVAILABLE_MSG, isError: true };
  }
  const trimmed = target?.trim();
  if (!trimmed) {
    return {
      text: "Usage: ws-ckpt-rollback <target>\n  target: snapshot hash id",
      isError: true,
    };
  }

  const explicitWs = workspace?.trim();
  if (explicitWs) {
    if (cwdInsideWorkspace(explicitWs)) {
      return { text: CWD_INSIDE_WORKSPACE_REASON, isError: true };
    }
    try {
      const executor = new CommandExecutor();
      const output = await executor.rollback(explicitWs, trimmed);
      if (output.exitCode !== 0) {
        return { text: mapErrorToLLMMessage(output.stderr, { id: trimmed }), isError: true };
      }
      return { text: `Rolled back to ${trimmed}`, isError: false };
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      return { text: `Rollback error: ${msg}`, isError: true };
    }
  }

  const ws = pluginState.resolvedConfig?.workspace;
  if (ws && cwdInsideWorkspace(ws)) {
    return { text: CWD_INSIDE_WORKSPACE_REASON, isError: true };
  }

  const result = await pluginState.manager.rollback(trimmed);
  return { text: result.message, isError: !result.success };
}

export async function handleListCheckpoints(): Promise<{
  text: string;
  isError: boolean;
}> {
  if (!pluginState.manager || !pluginState.environmentReady) {
    return { text: UNAVAILABLE_MSG, isError: true };
  }
  const checkpoints = await pluginState.manager.listCheckpoints();
  if (checkpoints.length === 0) {
    return {
      text: "No checkpoints found. The workspace is active and daemon is responding \u2014 there are simply no snapshots yet.",
      isError: false,
    };
  }
  const header = ["ID", "Created At", "Message", "Metadata"];
  const rows = checkpoints.map((cp) => [
    cp.snapshot,
    cp.createdAt,
    cp.message ?? "",
    cp.metadata ? JSON.stringify(cp.metadata) : "",
  ]);
  const widths = header.map((h, i) =>
    Math.max(h.length, ...rows.map((r) => r[i].length)),
  );
  const fmt = (cols: string[]) =>
    cols.map((c, i) => c.padEnd(widths[i])).join("  ");
  const lines: string[] = [
    `Checkpoints (${checkpoints.length}):`,
    "",
    fmt(header),
    widths.map((w) => "-".repeat(w)).join("  "),
    ...rows.map(fmt),
  ];
  return { text: lines.join("\n"), isError: false };
}

export async function handleDelete(
  snapshot?: string,
  workspace?: string,
): Promise<{ text: string; isError: boolean }> {
  if (!pluginState.manager || !pluginState.environmentReady) {
    return { text: UNAVAILABLE_MSG, isError: true };
  }
  if (!snapshot) {
    return {
      text: "Usage: ws-ckpt-delete <snapshot> [workspace]\n  snapshot: snapshot ID to delete (required)\n  workspace: workspace path (optional, defaults to current)",
      isError: true,
    };
  }
  const ws = workspace || pluginState.resolvedConfig?.workspace;
  if (!ws) {
    return { text: "No workspace path available", isError: true };
  }
  try {
    const executor = new CommandExecutor();
    const output = await executor.delete(snapshot, { workspace: ws, force: true });
    if (output.exitCode !== 0) {
      return { text: mapErrorToLLMMessage(output.stderr, { id: snapshot }), isError: true };
    }
    pluginState.manager.getStore().remove(snapshot);
    return { text: `Snapshot ${snapshot} deleted`, isError: false };
  } catch (error) {
    const msg = error instanceof Error ? error.message : String(error);
    return { text: `Delete error: ${msg}`, isError: true };
  }
}

export async function handleDiff(
  fromArg?: string,
  toArg?: string,
): Promise<{ text: string; isError: boolean }> {
  if (!pluginState.manager || !pluginState.environmentReady) {
    return { text: UNAVAILABLE_MSG, isError: true };
  }
  if (!fromArg) {
    return {
      text: "Usage: ws-ckpt-diff <from> [<to>]\n  from: source snapshot id\n  to:   target snapshot id",
      isError: true,
    };
  }
  const result = await pluginState.manager.execDiffRaw(fromArg, toArg ?? "HEAD");
  return { text: result.text, isError: !result.success };
}

export async function handleStatus(): Promise<{ text: string; isError: boolean }> {
  if (!pluginState.manager || !pluginState.environmentReady) {
    return { text: UNAVAILABLE_MSG, isError: true };
  }
  const result = await pluginState.manager.getStatus();
  const statusText = result.success
    ? `${result.message}\n(This is the complete daemon status report.)`
    : result.message;
  return { text: statusText, isError: !result.success };
}

export async function handleConfig(
  action?: string,
  key?: string,
  value?: string,
): Promise<{ text: string; isError: boolean }> {
  if (!pluginState.resolvedConfig) {
    return { text: UNAVAILABLE_MSG, isError: true };
  }

  const act = (action ?? "view").toLowerCase();

  if (act === "view") {
    const cfg = pluginState.resolvedConfig;
    const lines: string[] = ["Current ws-ckpt configuration:\n"];
    lines.push(`  autoCheckpoint: ${cfg.autoCheckpoint}`);
    const autoCleanupDisabled = daemonAutoCleanup.cleanupNum === undefined && daemonAutoCleanup.cleanupDuration === undefined;
    lines.push(
      `  maxSnapshotsNum: ${
        daemonAutoCleanup.cleanupNum !== undefined
          ? daemonAutoCleanup.cleanupNum
          : autoCleanupDisabled ? "not set - auto-cleanup disabled" : "not set"
      }`,
    );
    lines.push(
      `  maxSnapshotsDuration: ${
        daemonAutoCleanup.cleanupDuration !== undefined
          ? daemonAutoCleanup.cleanupDuration
          : autoCleanupDisabled ? "not set - auto-cleanup disabled" : "not set"
      }`,
    );
    lines.push(`  workspace: ${cfg.workspace}`);
    lines.push("\nNote: maxSnapshotsNum / maxSnapshotsDuration are ws-ckpt global daemon settings.");
    return { text: lines.join("\n"), isError: false };
  }

  if (act === "update" || act === "set") {
    if (!key) {
      return {
        text: "Usage: ws-ckpt-config update <key> <value>\n  Available keys: autoCheckpoint, maxSnapshotsNum, maxSnapshotsDuration, workspace",
        isError: true,
      };
    }

    if (key === "maxSnapshotsNum") {
      if (value === undefined) {
        return { text: "maxSnapshotsNum requires a value (positive integer, or \"unset\" to disable auto-cleanup)", isError: true };
      }

      // --- unset path ---
      if (value === "unset") {
        daemonAutoCleanup.cleanupNum = undefined;
        const cmd = new CommandExecutor();
        const result = await cmd.config({ disableAutoCleanup: true });
        if (result.exitCode !== 0) {
          return { text: `Failed to disable auto-cleanup on daemon: ${result.stderr}`, isError: true };
        }
        return { text: "Cleared: maxSnapshotsNum unset — auto-cleanup disabled on ws-ckpt daemon.", isError: false };
      }

      // --- set path ---
      const num = parseInt(value, 10);
      if (isNaN(num) || num < 1) {
        return { text: "maxSnapshotsNum must be a positive integer", isError: true };
      }
      const cmd = new CommandExecutor();
      const result = await cmd.config({ enableAutoCleanup: true, autoCleanupKeep: String(num) });
      if (result.exitCode !== 0) {
        return { text: `Failed to configure daemon: ${result.stderr}`, isError: true };
      }
      daemonAutoCleanup.cleanupNum = num;
      daemonAutoCleanup.cleanupDuration = undefined; // mutually exclusive
      return { text: `Updated ws-ckpt global daemon config: maxSnapshotsNum = ${num} (auto-cleanup enabled, keep ${num})`, isError: false };
    }

    if (key === "maxSnapshotsDuration") {
      if (value === undefined) {
        return { text: "maxSnapshotsDuration requires a value (e.g. \"7d\", \"24h\", or \"unset\" to disable auto-cleanup)", isError: true };
      }

      // --- unset path ---
      if (value === "unset") {
        daemonAutoCleanup.cleanupDuration = undefined;
        const cmd = new CommandExecutor();
        const result = await cmd.config({ disableAutoCleanup: true });
        if (result.exitCode !== 0) {
          return { text: `Failed to disable auto-cleanup on daemon: ${result.stderr}`, isError: true };
        }
        return { text: "Cleared: maxSnapshotsDuration unset — auto-cleanup disabled on ws-ckpt daemon.", isError: false };
      }

      // --- set path ---
      const cmd = new CommandExecutor();
      const result = await cmd.config({ enableAutoCleanup: true, autoCleanupKeep: value });
      if (result.exitCode !== 0) {
        return { text: `Failed to configure daemon: ${result.stderr}`, isError: true };
      }
      daemonAutoCleanup.cleanupDuration = value;
      daemonAutoCleanup.cleanupNum = undefined; // mutually exclusive
      return { text: `Updated ws-ckpt global daemon config: maxSnapshotsDuration = ${value} (auto-cleanup enabled, keep ${value})`, isError: false };
    }

    if (key === "autoCheckpoint") {
      const coerced = value === "true";
      if (coerced) {
        const ws = pluginState.resolvedConfig.workspace;
        if (ws && cwdInsideWorkspace(ws)) {
          return { text: CWD_INSIDE_WORKSPACE_REASON, isError: true };
        }
      }
      pluginState.resolvedConfig.autoCheckpoint = coerced;
      const persistHint = coerced
        ? `\n\nNote: This change is in-memory only and will reset on Gateway restart.\nTo persist, run:\n  openclaw config set plugins.entries.ws-ckpt.config.autoCheckpoint true --strict-json\n(This will cause a Gateway restart.)`
        : `\n\nNote: This change is in-memory only and will reset on Gateway restart.\nTo persist, run:\n  openclaw config set plugins.entries.ws-ckpt.config.autoCheckpoint false --strict-json\n(This will cause a Gateway restart.)`;
      return {
        text: `Config updated: autoCheckpoint = ${coerced}${persistHint}`,
        isError: false,
      };
    }

    if (key === "workspace") {
      if (!value) {
        return { text: "workspace requires a path value", isError: true };
      }
      pluginState.resolvedConfig.workspace = value;
      return {
        text: `Config updated: workspace = ${value} (in-memory, will reset on Gateway restart)`,
        isError: false,
      };
    }

    return {
      text: `Unknown config key: ${key}. Available: autoCheckpoint, maxSnapshotsNum, maxSnapshotsDuration, workspace`,
      isError: true,
    };
  }

  return {
    text: `Unknown action: ${act}. Use "view" or "update".`,
    isError: true,
  };
}
