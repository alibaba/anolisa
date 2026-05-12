/**
 * Shared plugin state singleton.
 *
 * All modules that need to read or mutate manager, environmentReady,
 * resolvedConfig, or pluginApi must import
 * from this module to avoid circular dependencies.
 */

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
