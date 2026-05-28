/**
 * ws-ckpt OpenClaw Plugin entry point.
 *
 * Exports {@link register} lifecycle function that the OpenClaw runtime
 * calls when loading the plugin via `register(api)`.
 *
 * The plugin registers:
 * - 7 tools: ws-ckpt-checkpoint, ws-ckpt-rollback,
 *   ws-ckpt-list, ws-ckpt-delete, ws-ckpt-diff, ws-ckpt-status, ws-ckpt-config
 * - 3 hooks: message_received, agent_end, session_start
 */

import { PluginConfigManager, parseDaemonAutoCleanupConfig, daemonAutoCleanup } from "./config.js";
import { EnvironmentChecker } from "./environment-check.js";
import { BtrfsManager } from "./btrfs-manager.js";
import { CommandExecutor } from "./commands.js";
import type { PluginConfig } from "./types.js";
import {
  definePluginEntry,
  type OpenClawPluginApi,
} from "../types-shim.js";
import { pluginState, cwdInsideWorkspace, CWD_INSIDE_WORKSPACE_REASON } from "./state.js";
import { registerTools } from "./tool-registry.js";
import { registerHooks } from "./hooks.js";
import { ensureToolsAlsoAllow } from "./whitelist.js";

// ---------------------------------------------------------------------------
// register() — main entry point called by OpenClaw runtime
// ---------------------------------------------------------------------------

/**
 * Register the ws-ckpt plugin.
 *
 * Called by the OpenClaw runtime when the plugin is loaded. Performs
 * configuration loading, environment checks, workspace initialization,
 * and registers tools and hooks with the OpenClaw API.
 *
 * @param api - Plugin API provided by the OpenClaw runtime.
 */
function register(api: OpenClawPluginApi): void {
  pluginState.pluginApi = api;
  const rawConfig = api.pluginConfig ?? {};

  // ------------------------------------------------------------------
  // 1. Load and validate configuration
  // ------------------------------------------------------------------
  const configManager = new PluginConfigManager(
    rawConfig as Partial<PluginConfig>,
  );
  const validation = configManager.validate();
  // Keep object identity stable across reloads so stale hook closures stay live.
  const fresh = configManager.getConfig();
  const config = pluginState.resolvedConfig
    ? Object.assign(pluginState.resolvedConfig, fresh)
    : (pluginState.resolvedConfig = fresh);

  if (!validation.valid) {
    api.logger?.warn?.(
      `Configuration errors: ${validation.errors.join(", ")}`,
    );
  }

  // ------------------------------------------------------------------
  // 3. Create BtrfsManager (environment check deferred to async init)
  // ------------------------------------------------------------------
  // Idempotent: keep manager identity stable across reloads (config ref already shared).
  pluginState.manager ??= new BtrfsManager(config);
  pluginState.manager.updateConfig(config);

  // Re-check environment on every register (daemon may start/stop between reloads).
  void (async () => {
    const checker = new EnvironmentChecker();
    const envResult = await checker.check();
    pluginState.environmentReady = envResult.passed;

    if (!envResult.passed) {
      const missing: string[] = [];
      if (!envResult.cliAvailable) missing.push("ws-ckpt CLI not found");
      if (!envResult.daemonRunning) missing.push("daemon not running");
      console.warn(
        `[ws-ckpt] Degraded mode: ${missing.join(", ")}`,
      );
      return;
    }

    if (cwdInsideWorkspace(config.workspace)) {
      pluginState.environmentReady = false;
      console.warn(`[ws-ckpt] Refusing: ${CWD_INSIDE_WORKSPACE_REASON}`);
    } else {
      try {
        const ok = await pluginState.manager!.ensureWorkspace(config.workspace);
        if (!ok) {
          pluginState.environmentReady = false;
          console.warn(
            `[ws-ckpt] Degraded mode: workspace setup failed (${config.workspace})`,
          );
        }
      } catch (err) {
        pluginState.environmentReady = false;
        console.warn(
          `[ws-ckpt] Degraded mode: workspace setup failed (${config.workspace}):`,
          err instanceof Error ? err.message : String(err),
        );
      }
    }

    // Query daemon auto-cleanup config to align in-memory state.
    const cmd = new CommandExecutor();
    const cfgResult = await cmd.config();
    if (cfgResult.exitCode === 0) {
      const { cleanupNum, cleanupDuration } = parseDaemonAutoCleanupConfig(cfgResult.stdout);
      daemonAutoCleanup.cleanupNum = cleanupNum;
      daemonAutoCleanup.cleanupDuration = cleanupDuration;
    }
  })();

  // ------------------------------------------------------------------
  // 4. Ensure ws-ckpt tools are in tools.alsoAllow whitelist
  // ------------------------------------------------------------------
  ensureToolsAlsoAllow(api);

  // ------------------------------------------------------------------
  // 5. Register tools
  // ------------------------------------------------------------------
  registerTools(api);

  // ------------------------------------------------------------------
  // 6. Register hooks
  // ------------------------------------------------------------------
  registerHooks(api, config);

  // ------------------------------------------------------------------
  // Done
  // ------------------------------------------------------------------
  // Registration complete — config available via ws-ckpt-config tool
}

// ---------------------------------------------------------------------------
// Plugin entry definition + exports
// ---------------------------------------------------------------------------

export default definePluginEntry({
  id: "ws-ckpt",
  name: "ws-ckpt",
  register,
});

export { register };

// Re-export components for external consumers
export { BtrfsManager } from "./btrfs-manager.js";
export { CommandExecutor } from "./commands.js";
export { SnapshotStore } from "./snapshot-store.js";
export { PluginConfigManager, DEFAULT_CONFIG } from "./config.js";
export { EnvironmentChecker } from "./environment-check.js";
export type {
  PluginConfig,
  SnapshotInfo,
  CheckpointResult,
  RollbackResult,
  StatusReport,
  CleanupResult,
  CommandOutput,
} from "./types.js";
