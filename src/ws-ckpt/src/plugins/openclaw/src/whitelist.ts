/**
 * Whitelist management for the ws-ckpt OpenClaw plugin.
 *
 * Ensures all ws-ckpt tool names are present in the OpenClaw
 * `tools.alsoAllow` configuration. If any are missing, they are
 * written to openclaw.json (triggering a one-time Gateway restart).
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
 * Ensure all ws-ckpt tools are present in the OpenClaw `tools.alsoAllow`
 * whitelist. If any are missing, persist them to openclaw.json.
 *
 * Reads the current alsoAllow from disk (api.config may be a stale snapshot
 * during reload), and skips if already complete. Also guarded by a process-
 * level flag to avoid reload-loop spam.
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
    if (missing.length === 0) {
      alreadyEnsured = true;
      return;
    }

    const updated = [...currentAllow, ...missing];
    writeToolsAlsoAllow(configPath, updated);
    alreadyEnsured = true;
    console.log(
      `[ws-ckpt] Added ${missing.length} tool(s) to tools.alsoAllow: ${missing.join(", ")}. Gateway will restart.`,
    );
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    console.warn(`[ws-ckpt] Failed to update tools.alsoAllow: ${msg}`);
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

/**
 * Write the tools.alsoAllow array to openclaw.json.
 */
function writeToolsAlsoAllow(configPath: string, alsoAllow: string[]): void {
  let config: Record<string, unknown> = {};
  try {
    if (fs.existsSync(configPath)) {
      const raw = fs.readFileSync(configPath, "utf-8");
      const parsed = JSON.parse(raw);
      if (typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)) {
        config = parsed;
      }
    }
  } catch { /* start fresh */ }

  const tools = (config.tools ?? {}) as Record<string, unknown>;
  config.tools = { ...tools, alsoAllow };

  const dir = path.dirname(configPath);
  fs.mkdirSync(dir, { recursive: true });
  const tmpPath = `${configPath}.tmp.${process.pid}`;
  fs.writeFileSync(tmpPath, JSON.stringify(config, null, 2) + "\n", {
    encoding: "utf-8",
    mode: 0o600,
  });
  fs.renameSync(tmpPath, configPath);
}
