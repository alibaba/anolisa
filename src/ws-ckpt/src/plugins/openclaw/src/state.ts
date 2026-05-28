/**
 * Shared plugin state singleton.
 *
 * All modules that need to read or mutate manager, environmentReady,
 * resolvedConfig, or pluginApi must import
 * from this module to avoid circular dependencies.
 */

import path from "node:path";
import type { BtrfsManager } from "./btrfs-manager.js";
import type { OpenClawPluginApi } from "../types-shim.js";
import type { PluginConfig } from "./types.js";

// ---------------------------------------------------------------------------
// Mutable state object — mutated by register() in index.ts
// ---------------------------------------------------------------------------

export const pluginState = {
  /** Singleton BtrfsManager instance — created during registration. */
  manager: null as BtrfsManager | null,

  /** Whether the environment check passed. */
  environmentReady: false,

  /** Saved reference to the plugin API for use in hooks. */
  pluginApi: null as OpenClawPluginApi | null,

  /** Resolved plugin config for inspection via ws-ckpt-config tool. */
  resolvedConfig: null as PluginConfig | null,
};

// ---------------------------------------------------------------------------
// Shared constants
// ---------------------------------------------------------------------------

export const UNAVAILABLE_MSG =
  "ws-ckpt plugin is not available. Run environment check for details.";

export const CWD_INSIDE_WORKSPACE_REASON =
  "The hosting process's cwd is inside the workspace. " +
  "ws-ckpt replaces the workspace inode during init/checkpoint/rollback, " +
  "which would invalidate the process cwd. " +
  "This is NOT retryable — do NOT call any ws-ckpt tool again in this session. " +
  "The user must launch the session from outside the workspace directory.";

export function cwdInsideWorkspace(workspace: string): boolean {
  let cwd: string;
  try {
    cwd = path.resolve(process.cwd());
  } catch {
    return false;
  }
  const ws = path.resolve(workspace);
  return cwd === ws || cwd.startsWith(ws + path.sep);
}
