/**
 * Whitelist check for the ws-ckpt OpenClaw plugin.
 *
 * Verifies all ws-ckpt tool names are present in the OpenClaw
 * `tools.alsoAllow` configuration and warns about missing ones. The plugin
 * never writes openclaw.json itself: out-of-band writes trip the
 * OpenClaw >= 2026.9.2 config snapshot-hash guard ("config changed since
 * last load"). Entries are added by install-openclaw.sh through the
 * sanctioned `openclaw config set` mutation path.
 */

import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import type { OpenClawPluginApi } from "../types-shim.js";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/** All ws-ckpt tool names that need to be in tools.alsoAllow. */
export const WS_CKPT_TOOL_NAMES = [
  "ws-ckpt-checkpoint",
  "ws-ckpt-rollback",
  "ws-ckpt-list",
  "ws-ckpt-delete",
  "ws-ckpt-diff",
  "ws-ckpt-config",
  "ws-ckpt-status",
];

/** Once-per-process guard: avoid repeated writes during reload loops. */
let alreadyEnsured = false;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Check that all ws-ckpt tools are present in the OpenClaw `tools.alsoAllow`
 * whitelist, warning once per process if any are missing.
 *
 * Reads the current alsoAllow from disk (api.config may be a stale snapshot
 * during reload). Never writes openclaw.json: config mutations belong to the
 * installer (install-openclaw.sh), which goes through `openclaw config set`.
 */
export function ensureToolsAlsoAllow(api: OpenClawPluginApi): void {
  if (alreadyEnsured) return;
  try {
    const configPath = resolveOpenClawConfigPath();
    if (!configPath) return;

    // Prefer on-disk truth over api.config (which may be stale during reload).
    const onDisk = readAlsoAllowFromDisk(configPath);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const cfg = api.config as any;
    const fromApi: string[] = Array.isArray(cfg?.tools?.alsoAllow)
      ? [...cfg.tools.alsoAllow]
      : [];
    const currentAllow = onDisk ?? fromApi;

    const missing = WS_CKPT_TOOL_NAMES.filter((t) => !currentAllow.includes(t));
    alreadyEnsured = true;
    if (missing.length === 0) return;

    console.warn(
      `[ws-ckpt] ${missing.length} tool(s) missing from tools.alsoAllow: ${missing.join(", ")}. ` +
        `Re-run 'ws-ckpt plugin install --runtime openclaw' to add them via 'openclaw config set'.`,
    );
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    console.warn(`[ws-ckpt] Failed to check tools.alsoAllow: ${msg}`);
  }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/**
 * Resolve the openclaw.json config path (mirrors logic in openclaw-config.ts).
 */
function resolveOpenClawConfigPath(): string | null {
  try {
    const env = process.env;
    const explicitPath = env.OPENCLAW_CONFIG_PATH?.trim();
    if (explicitPath) {
      return path.resolve(explicitPath);
    }
    const stateDir =
      env.OPENCLAW_STATE_DIR?.trim() ||
      path.join(os.homedir(), ".openclaw");
    return path.join(stateDir, "openclaw.json");
  } catch {
    return null;
  }
}

/**
 * Read the existing `tools.alsoAllow` array directly from disk.
 * Returns null if the file is missing/unreadable/malformed.
 */
function readAlsoAllowFromDisk(configPath: string): string[] | null {
  try {
    if (!fs.existsSync(configPath)) return null;
    const raw = fs.readFileSync(configPath, "utf-8");
    const parsed = JSON.parse(raw);
    const allow = parsed?.tools?.alsoAllow;
    return Array.isArray(allow) ? allow.map(String) : null;
  } catch {
    return null;
  }
}
