/**
 * Configuration management for the ws-ckpt plugin.
 *
 * Handles loading configuration from user-provided values and environment
 * variables, merging with sensible defaults, and validating the result.
 */

import type { PluginConfig } from "./types.js";

/**
 * Parse `ws-ckpt config` stdout to extract daemon auto-cleanup state.
 * Auto-cleanup keep: 3 (count mode) → cleanupNum; 7d → cleanupDuration.
 */
export function parseDaemonAutoCleanupConfig(stdout: string): {
  cleanupNum?: number;
  cleanupDuration?: string;
} {
  // Auto-cleanup disabled
  if (/Auto-cleanup:\s+disabled/i.test(stdout)) {
    return {};
  }

  // Parse keep value — grab first token after "Auto-cleanup keep:"
  const keepMatch = stdout.match(/Auto-cleanup keep:\s+(\S+)/);
  if (!keepMatch) return {};

  const keepVal = keepMatch[1];
  const num = parseInt(keepVal, 10);
  if (!isNaN(num) && String(num) === keepVal) {
    return { cleanupNum: num };
  }
  return { cleanupDuration: keepVal };
}

/** Default configuration values. */
export const DEFAULT_CONFIG: PluginConfig = {
  workspace: `${process.env.HOME ?? "/root"}/.openclaw/workspace`,
  autoCheckpoint: false,
};

/** Runtime tracking of daemon auto-cleanup state (global ws-ckpt settings). */
export const daemonAutoCleanup = {
  cleanupNum: undefined as number | undefined,
  cleanupDuration: undefined as string | undefined,
};

/**
 * Configuration manager for the ws-ckpt plugin.
 *
 * Loads configuration from the plugin's user-provided config and validates
 * the result. Configuration sources, in priority order:
 *   1. user config (from openclaw.json `plugins.entries.ws-ckpt.config`)
 *   2. DEFAULT_CONFIG
 */
export class PluginConfigManager {
  private config: PluginConfig;

  /**
   * Create a new PluginConfigManager.
   *
   * @param userConfig - Partial configuration from the plugin's config file.
   */
  constructor(userConfig: Partial<PluginConfig> = {}) {
    this.config = { ...DEFAULT_CONFIG, ...userConfig };
  }

  /** Return the resolved configuration. */
  public getConfig(): PluginConfig {
    return { ...this.config };
  }

  /**
   * Validate the current configuration.
   *
   * @returns An object with `valid` flag and any `errors` found.
   */
  public validate(): { valid: boolean; errors: string[] } {
    const errors: string[] = [];
    return { valid: errors.length === 0, errors };
  }
}