/**
 * Token-Less Unified Plugin for OpenClaw v5
 *
 * Combines multiple complementary optimisation strategies into a single plugin:
 *
 *   1. RTK command rewriting  — transparently rewrites exec tool commands to
 *      their RTK equivalents (delegated to `rtk rewrite`).
 *   2. Tokenless response compression — compresses tool responses via
 *      `tokenless compress-response` (removes debug/null/empty values).
 *   3. TOON context compression — encodes JSON tool responses to TOON format
 *      via `toon -e`, reducing token usage for structured data. When both
 *      response and TOON compression are enabled, they run sequentially:
 *      Response Compression strips noise → TOON eliminates JSON format overhead.
 *
 * Stats are recorded automatically by tokenless compress-response.
 * Context passing uses environment variables (TOKENLESS_AGENT_ID,
 * TOKENLESS_SESSION_ID, TOKENLESS_TOOL_USE_ID) which are inherited by
 * child processes and read by RTK's stats patch.
 */

import { execSync, execFileSync, spawnSync } from "child_process";
import { existsSync, statSync } from "fs";

// ---- Session ID mapping --------------------------------------------------------
// OpenClaw's tool_result_persist ctx provides sessionKey ("agent:main:main")
// but NOT sessionId (UUID). We maintain a sessionKey → sessionId map built
// from session_start events so response compression can use the correct UUID.

const sessionMap: Map<string, string> = new Map();

// ---- Binary availability cache ------------------------------------------------

let rtkAvailable: boolean | null = null;
let tokenlessAvailable: boolean | null = null;
let toonAvailable: boolean | null = null;

// Resolved absolute paths — set by check*() functions so subprocess calls
// use the correct path even when the binary is not on PATH (e.g. RPM installs
// that place rtk/toon in /usr/libexec/anolisa/tokenless/).
let rtkPath: string = "rtk";
let tokenlessPath: string = "tokenless";
let toonPath: string = "toon";

const LIBEXEC_FALLBACK = "/usr/libexec/anolisa/tokenless";
const TOKENLESS_FALLBACK = "/usr/bin/tokenless";

// Check both existence and execute permission (mirrors shell `-x` test).
function isExecutable(path: string): boolean {
  try {
    return existsSync(path) && (statSync(path).mode & 0o111) !== 0;
  } catch {
    return false;
  }
}

function checkRtk(): boolean {
  if (rtkAvailable !== null) return rtkAvailable;
  try {
    const result = execSync("which rtk 2>/dev/null || echo ''", { encoding: "utf-8" }).trim();
    if (result && result !== "") {
      rtkPath = result;
      rtkAvailable = true;
    } else if (isExecutable(`${LIBEXEC_FALLBACK}/rtk`)) {
      rtkPath = `${LIBEXEC_FALLBACK}/rtk`;
      rtkAvailable = true;
    } else {
      rtkAvailable = false;
    }
  } catch {
    // which not available; check libexec directly
    if (isExecutable(`${LIBEXEC_FALLBACK}/rtk`)) {
      rtkPath = `${LIBEXEC_FALLBACK}/rtk`;
      rtkAvailable = true;
    } else {
      rtkAvailable = false;
    }
    return rtkAvailable;
  }
  return rtkAvailable;
}

function isSkillContent(message: any): boolean {
  // Skill files (.md with YAML frontmatter) must not be compressed because
  // truncation would break the skill metadata and make agent skills unusable.
  if (typeof message !== "string") return false;
  const trimmed = message.trimStart();
  if (!trimmed.startsWith("---")) return false;
  // Check the first few lines for typical skill metadata fields
  const firstLines = trimmed.split("\n", 20).join("\n");
  return /^name:/m.test(firstLines) || /^description:/m.test(firstLines);
}

function checkTokenless(): boolean {
  if (tokenlessAvailable !== null) return tokenlessAvailable;
  try {
    const result = execSync("which tokenless 2>/dev/null || echo ''", { encoding: "utf-8" }).trim();
    if (result && result !== "") {
      tokenlessPath = result;
      tokenlessAvailable = true;
    } else if (isExecutable(TOKENLESS_FALLBACK)) {
      tokenlessPath = TOKENLESS_FALLBACK;
      tokenlessAvailable = true;
    } else {
      tokenlessAvailable = false;
    }
  } catch {
    if (isExecutable(TOKENLESS_FALLBACK)) {
      tokenlessPath = TOKENLESS_FALLBACK;
      tokenlessAvailable = true;
    } else {
      tokenlessAvailable = false;
    }
    return tokenlessAvailable;
  }
  return tokenlessAvailable;
}

function checkToon(): boolean {
  if (toonAvailable !== null) return toonAvailable;
  try {
    const result = execSync("which toon 2>/dev/null || echo ''", { encoding: "utf-8" }).trim();
    if (result && result !== "") {
      toonPath = result;
      toonAvailable = true;
    } else if (isExecutable(`${LIBEXEC_FALLBACK}/toon`)) {
      toonPath = `${LIBEXEC_FALLBACK}/toon`;
      toonAvailable = true;
    } else {
      toonAvailable = false;
    }
  } catch {
    if (isExecutable(`${LIBEXEC_FALLBACK}/toon`)) {
      toonPath = `${LIBEXEC_FALLBACK}/toon`;
      toonAvailable = true;
    } else {
      toonAvailable = false;
    }
    return toonAvailable;
  }
  return toonAvailable;
}

// ---- Subprocess helpers -------------------------------------------------------

function tryRtkRewrite(command: string): string | null {
  try {
    const result = spawnSync(rtkPath, ["rewrite", command], {
      encoding: "utf-8",
      timeout: 2000,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const rewritten = result.stdout?.trim();
    if ((result.status === 0 || result.status === 3) && rewritten && rewritten !== command) {
      return rewritten;
    }
    return null;
  } catch {
    return null;
  }
}

function tryCompressResponse(response: any, sessionId?: string, toolCallId?: string): any | null {
  try {
    const input = JSON.stringify(response);
    const args = ["compress-response", "--agent-id", "openclaw"];
    if (sessionId) args.push("--session-id", sessionId);
    if (toolCallId) args.push("--tool-use-id", toolCallId);
    const result = execFileSync(tokenlessPath, args, {
      encoding: "utf-8",
      timeout: 3000,
      input,
    }).trim();

    // Only return the compressed result if it differs from the input
    if (result === input) {
      return null; // No actual compression occurred
    }

    return JSON.parse(result);
  } catch {
    return null;
  }
}

function tryCompressToon(response: any, sessionId?: string, toolCallId?: string): { toonText: string; savingsPct: number } | null {
  try {
    const input = JSON.stringify(response);
    const beforeChars = input.length;
    const args = ["compress-toon", "--agent-id", "openclaw"];
    if (sessionId) args.push("--session-id", sessionId);
    if (toolCallId) args.push("--tool-use-id", toolCallId);
    const toonText = execFileSync(tokenlessPath, args, {
      encoding: "utf-8",
      timeout: 3000,
      input,
    }).trim();
    if (!toonText || toonText === input) return null;
    if (toonText.length > beforeChars) return null;

    const afterChars = toonText.length;
    const savingsPct = beforeChars > 0 ? Math.round(((beforeChars - afterChars) / beforeChars) * 100) : 0;
    return { toonText, savingsPct };
  } catch {
    return null;
  }
}

function tryEnvCheck(toolName: string): { status: string; diagnostic: string } | null {
  try {
    const result = execFileSync(tokenlessPath, ["env-check", "--tool", toolName, "--json"], {
      encoding: "utf-8",
      timeout: 3000,
    }).trim();
    const parsed = JSON.parse(result);
    const status: string = parsed.status || "UNKNOWN";

    // Phase 1+2: UNKNOWN (not in dict) or READY → skip silently
    if (status === "UNKNOWN" || status === "READY") return null;

    // Phase 3: NOT_READY → attempt auto-fix
    const fixResult = execFileSync(tokenlessPath, ["env-check", "--tool", toolName, "--fix", "--json"], {
      encoding: "utf-8",
      timeout: 10000,
    }).trim();
    const fixParsed = JSON.parse(fixResult);
    const postStatus: string = fixParsed.status || "NOT_READY";

    // Phase 3 success: fix worked → continue silently
    if (postStatus === "READY") return null;

    // Phase 4: Fix failed → feedback to Agent
    const diagnostic: string = fixParsed.diagnostic
      || `[tokenless tool-ready] ${toolName}: NOT_READY — environment issue. Skip retry.`;
    return { status: postStatus, diagnostic };
  } catch {
    return null;
  }
}

// ---- Plugin entry point -------------------------------------------------------

export default {
  id: "tokenless-openclaw",
  name: "Token-Less",
  version: "1.0.0",
  description: "Unified RTK command rewriting + response/TOON compression + Tool Ready",
  register(api: any) {
  const pluginConfig = api.config ?? {};
  const rtkEnabled = pluginConfig.rtk_enabled !== false;
  const responseCompressionEnabled = pluginConfig.response_compression_enabled !== false;
  const toonCompressionEnabled = pluginConfig.toon_compression_enabled !== false;
  const toolReadyEnabled = pluginConfig.tool_ready_enabled !== false;
  const skipTools: Set<string> = new Set((pluginConfig.skip_tools ?? ["Read", "read_file", "Glob", "list_directory", "NotebookRead"]).map((t: string) => t.toLowerCase()));
  const verbose = pluginConfig.verbose !== false;

  // ---- 0. Session mapping (sessionKey → sessionId) ---------------------------

  api.on(
    "session_start",
    (event: { sessionId: string; sessionKey?: string; resumedFrom?: string }) => {
      if (event.sessionKey && event.sessionId) {
        sessionMap.set(event.sessionKey, event.sessionId);
      }
      // Also store in env var for RTK (exec) path
      process.env.TOKENLESS_SESSION_ID = event.sessionId;
    },
  );

  // ---- 1. Tool Ready environment pre-check (before_tool_call) -----------------

  if (toolReadyEnabled && checkTokenless()) {
    api.on(
      "before_tool_call",
      (event: { toolName: string; params: Record<string, unknown> }, ctx: { sessionId?: string; sessionKey?: string; agentId?: string; toolCallId?: string; runId?: string }) => {
        // Full 4-phase flow: Lookup → Check → Fix → Feedback
        // Returns null for UNKNOWN/READY/post-fix-success (continue silently).
        // Returns diagnostic only when fix fails (feedback to Agent).
        const result = tryEnvCheck(event.toolName);
        if (!result) return;

        if (verbose) {
          console.log(`[tokenless/tool-ready] ${event.toolName}: ${result.status} — tool not available`);
        }
        return { contextPrefix: result.diagnostic };
      },
      { priority: 5 },
    );
  }

  // ---- 2. RTK command rewriting (before_tool_call) ----------------------------

  if (rtkEnabled && checkRtk()) {
    api.on(
      "before_tool_call",
      (event: { toolName: string; params: Record<string, unknown> }, ctx: { sessionId?: string; sessionKey?: string; agentId?: string; toolCallId?: string; runId?: string }) => {
        if (event.toolName !== "exec") return;

        const command = event.params?.command;
        if (typeof command !== "string") return;

        // Set env vars so RTK and response compression can read agent/session/tool IDs
        process.env.TOKENLESS_AGENT_ID = "openclaw";
        if (ctx?.sessionId) process.env.TOKENLESS_SESSION_ID = ctx.sessionId;
        if (ctx?.toolCallId) process.env.TOKENLESS_TOOL_USE_ID = ctx.toolCallId;

        const rewritten = tryRtkRewrite(command);
        if (!rewritten) return;

        if (verbose) {
          console.log(`[tokenless/rtk] rewrite: ${command} -> ${rewritten}`);
        }

        return { params: { ...event.params, command: rewritten } };
      },
      { priority: 10 },
    );
  }

  // ---- 3. Response / TOON compression (tool_result_persist) -------------------
  // Pipeline: Response Compression → TOON (sequential, not mutually exclusive)
  //   1. Strip debug/nulls/empty, truncate long strings/arrays
  //   2. If result is still valid JSON and TOON is enabled, encode to TOON format

  if (checkTokenless() && (responseCompressionEnabled || toonCompressionEnabled)) {
    api.on(
      "tool_result_persist",
      (event: { toolName?: string; toolCallId?: string; message: any; isSynthetic?: boolean }, ctx: { agentId?: string; sessionId?: string; sessionKey?: string; toolName?: string; toolCallId?: string }) => {
        const beforeJson = JSON.stringify(event.message);
        // Skip small responses
        if (beforeJson.length < 200) return;

        // Skip content-retrieval tools — agent needs complete responses
        if (event.toolName && skipTools.has(event.toolName.toLowerCase())) return;

        // Skip skill content to avoid breaking YAML frontmatter metadata.
        if (isSkillContent(event.message)) return;

        const toolCallId = ctx?.toolCallId || event.toolCallId;

        // Resolve sessionId with 4-level priority:
        //   1. ctx.sessionId   — direct from OpenClaw (newer versions)
        //   2. sessionMap[sessionKey] — from session_start mapping
        //   3. TOKENLESS_SESSION_ID   — env var (set by session_start / before_tool_call)
        //   4. ctx.sessionKey  — always available ("agent:main:main"), best-effort fallback
        const sessionId = ctx?.sessionId
          || (ctx?.sessionKey && sessionMap.get(ctx.sessionKey))
          || process.env.TOKENLESS_SESSION_ID
          || ctx?.sessionKey;

        // Step 1: Response Compression
        let currentMessage: any = event.message;
        let usedResponseCompression = false;

        if (responseCompressionEnabled) {
          const compressed = tryCompressResponse(currentMessage, sessionId, toolCallId);
          if (compressed) {
            currentMessage = compressed;
            usedResponseCompression = true;
          }
        }

        // Step 2: TOON Encoding (if compressed result is JSON-serializable)
        let usedToon = false;
        let toonText = "";

        if (toonCompressionEnabled && checkToon()) {
          const result = tryCompressToon(currentMessage, sessionId, toolCallId);
          if (result) {
            toonText = result.toonText;
            usedToon = true;
          }
        }

        // Nothing was compressed — pass through unchanged
        if (!usedResponseCompression && !usedToon) return;

        // Build the final output
        let finalMessage: any;
        let savingsLabel: string;
        let totalSavingsPct: number;

        if (usedToon) {
          const before = JSON.stringify(event.message).length;
          const after = toonText.length;
          totalSavingsPct = before > 0 ? Math.round(((before - after) / before) * 100) : 0;
          savingsLabel = usedResponseCompression
            ? "response compressed + TOON encoded"
            : "TOON encoded";
          // Wrap TOON text in the original tool result message structure.
          // If we return a raw string, OpenClaw's tool_result_persist hook
          // replaces the entire message body, dropping role/toolCallId/toolName.
          // That causes session-transcript-repair to inject a synthetic
          // "missing tool result" error on the next run, breaking the session.
          const toonWrapped = `[TOON format, ${totalSavingsPct}% token savings]\n${toonText}`;
          if (typeof event.message === "object" && event.message?.role === "toolResult") {
            finalMessage = {
              ...event.message,
              content: [{ type: "text" as const, text: toonWrapped }],
            };
          } else {
            finalMessage = toonWrapped;
          }
        } else {
          const before = JSON.stringify(event.message).length;
          const after = JSON.stringify(currentMessage).length;
          totalSavingsPct = before > 0 ? Math.round(((before - after) / before) * 100) : 0;
          savingsLabel = "response compressed";
          finalMessage = currentMessage;
        }

        if (verbose) {
          const before = JSON.stringify(event.message).length;
          const after = usedToon ? toonText.length : JSON.stringify(finalMessage).length;
          console.log(
            `[tokenless/${savingsLabel}] ${event.toolName}: ${before} -> ${after} chars (${totalSavingsPct}% reduction)`,
          );
        }

        return { message: finalMessage };
      },
      { priority: 10 },
    );
  }

  // ---- Done -------------------------------------------------------------------

  if (verbose) {
    checkToon(); // populate toonAvailable cache before logging
    const features = [
      rtkEnabled && rtkAvailable ? "rtk-rewrite" : null,
      toolReadyEnabled && tokenlessAvailable ? "tool-ready" : null,
      responseCompressionEnabled && tokenlessAvailable ? "response-compression" : null,
      toonCompressionEnabled && toonAvailable ? "toon-compression" : null,
    ].filter(Boolean);
    console.log(`[tokenless] OpenClaw plugin registered — active features: ${features.join(", ") || "none"}`);
  }
  },
};
