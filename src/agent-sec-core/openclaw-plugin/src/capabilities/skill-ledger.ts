import { existsSync } from "node:fs";
import { resolve, dirname, basename } from "node:path";
import { homedir } from "node:os";
import type { SecurityCapability } from "../types.js";
import { callAgentSecCli } from "../utils.js";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type CheckResult = {
  status: string;
  skillName?: string;
  versionId?: string;
  createdAt?: string;
  updatedAt?: string;
  fileCount?: number;
  manifestHash?: string;
  [key: string]: unknown;
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const READ_TOOL_NAMES = ["read"];
const PATH_PARAM_NAMES = ["file_path", "path"];
const DEFAULT_TIMEOUT_MS = 5_000;

// ---------------------------------------------------------------------------
// Status messages and confirmation policy
// ---------------------------------------------------------------------------

const WARNING_MESSAGES: Record<string, (name: string) => string> = {
  warn: (n) => `⚠️ Skill '${n}' has low-risk findings — review recommended`,
  drifted: (n) => `⚠️ Skill '${n}' content has changed since last scan — confirm before using and run a fresh scan when possible`,
  none: (n) => `⚠️ Skill '${n}' has not been security-scanned yet — confirm before using`,
  error: (n) => `⚠️ Skill '${n}' check failed — invalid path or missing SKILL.md`,
  deny: (n) => `🚨 Skill '${n}' has high-risk findings — confirm only if you trust the skill and intend to review it`,
  tampered: (n) => `🚨 Skill '${n}' metadata signature verification failed — confirm only if you trust the skill source`,
};

const CONFIRMATION_SEVERITY: Record<string, "warning" | "critical"> = {
  none: "warning",
  drifted: "warning",
  deny: "critical",
  tampered: "critical",
};

// ---------------------------------------------------------------------------
// Key path resolution (mirrors Python's XDG_DATA_HOME / agent-sec/skill-ledger)
// ---------------------------------------------------------------------------

function getKeyPubPath(): string {
  const xdgData = process.env.XDG_DATA_HOME || resolve(homedir(), ".local", "share");
  return resolve(xdgData, "agent-sec", "skill-ledger", "key.pub");
}

function getKeyEncPath(): string {
  const xdgData = process.env.XDG_DATA_HOME || resolve(homedir(), ".local", "share");
  return resolve(xdgData, "agent-sec", "skill-ledger", "key.enc");
}

/** Return true only if both key.pub and key.enc exist (mirrors Python key_manager.keys_exist). */
function keysExist(): boolean {
  return existsSync(getKeyPubPath()) && existsSync(getKeyEncPath());
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Extract the file path from a before_tool_call event, or undefined if not a read-SKILL.md call. */
function extractSkillPath(
  event: { toolName: string; params: Record<string, unknown> },
): string | undefined {
  if (!READ_TOOL_NAMES.includes(event.toolName)) return undefined;

  let filePath: string | undefined;
  for (const paramName of PATH_PARAM_NAMES) {
    const val = event.params[paramName];
    if (typeof val === "string" && val.trim()) {
      filePath = val.trim();
      break;
    }
  }
  if (!filePath) return undefined;

  // Resolve to canonical absolute path to neutralize ".." traversal
  const resolved = resolve(filePath);

  if (!resolved.endsWith("/SKILL.md")) return undefined;

  return resolved;
}

/** Resolve skill_dir from the matched SKILL.md path. */
function resolveSkillDir(skillMdPath: string): string {
  return resolve(dirname(skillMdPath));
}

function formatSkillLedgerMessage(status: string, skillName: string): string {
  const warnFn = WARNING_MESSAGES[status];
  if (warnFn) return warnFn(skillName);
  return `⚠️ Skill '${skillName}' has unknown status '${status}'`;
}

function confirmationSeverity(status: string): "warning" | "critical" | undefined {
  return CONFIRMATION_SEVERITY[status];
}

// ---------------------------------------------------------------------------
// Capability
// ---------------------------------------------------------------------------

export const skillLedger: SecurityCapability = {
  id: "skill-ledger",
  name: "Skill Ledger",
  hooks: ["before_tool_call"],
  register(api) {
    /** Ensure signing keys exist; auto-init if missing. */
    let ensureKeysPromise: Promise<void> | null = null;

    function ensureKeys(): Promise<void> {
      if (ensureKeysPromise) return ensureKeysPromise;

      ensureKeysPromise = (async () => {
        if (keysExist()) return;

        api.logger.info("[skill-ledger] signing keys not found — running init --no-baseline");
        const result = await callAgentSecCli(
          ["skill-ledger", "init", "--no-baseline"],
          { timeout: DEFAULT_TIMEOUT_MS },
        );

        if (result.exitCode === 0) {
          api.logger.info("[skill-ledger] signing keys initialized successfully");
        } else if (!keysExist()) {
          api.logger.warn(`[skill-ledger] init --no-baseline failed: ${result.stderr}`);
          ensureKeysPromise = null; // allow retry on next call
        }
      })().catch(() => {
        ensureKeysPromise = null; // unexpected error — allow retry
      });

      return ensureKeysPromise;
    }

    // Eager key initialization (fire-and-forget from register)
    ensureKeys().catch(() => {});

    // ── Hook handler ───────────────────────────────────────────────
    api.on("before_tool_call", async (event: any, ctx: any) => {
      try {
        const skillMdPath = extractSkillPath(event);
        if (!skillMdPath) return undefined;

        const skillDir = resolveSkillDir(skillMdPath);
        const skillName = basename(skillDir);

        // Ensure keys are ready
        await ensureKeys();

        // Invoke CLI
        const result = await callAgentSecCli(
          ["skill-ledger", "check", skillDir],
          { timeout: DEFAULT_TIMEOUT_MS },
        );

        // Parse JSON output — CLI may return exit code 1 for deny/tampered states,
        // but stdout still contains valid check result with status field.
        // We should parse stdout even if exit code is non-zero.
        let checkResult: CheckResult;
        try {
          checkResult = JSON.parse(result.stdout) as CheckResult;
        } catch {
          // Only log warning if parsing fails AND exit code is non-zero
          if (result.exitCode !== 0) {
            api.logger.warn(`[skill-ledger] CLI error (exit ${result.exitCode}): ${result.stderr}`);
          } else {
            api.logger.warn(`[skill-ledger] failed to parse CLI output: ${result.stdout}`);
          }
          return undefined;
        }

        const status = checkResult.status ?? "unknown";

        // Emit warnings for non-pass statuses and require confirmation for
        // unscanned, changed, high-risk, or tampered skills.
        if (status === "pass") {
          return undefined;
        } else {
          const message = formatSkillLedgerMessage(status, skillName);
          api.logger.warn(`[skill-ledger] ${message}`);

          const severity = confirmationSeverity(status);
          if (severity) {
            return {
              requireApproval: {
                title: "Skill Ledger Security Check",
                description: message,
                severity,
              },
            };
          }
        }

        // For warn/error/unknown states, log and allow. Fail-open behavior for
        // CLI/runtime failures remains handled by the catch/parse branches.
        return undefined;
      } catch (err) {
        // Fail-open: uncaught errors must never block tool calls
        api.logger.warn(`[skill-ledger] error: ${err}`);
        return undefined;
      }
    }, { priority: 80 });
  },
};
