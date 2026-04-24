/**
 * Token-Less Unified Plugin for OpenClaw v5
 *
 * Combines two complementary optimisation strategies into a single plugin:
 *
 *   1. RTK command rewriting  — transparently rewrites exec tool commands to
 *      their RTK equivalents (delegated to `rtk rewrite`).
 *   2. Response compression   — compresses API/tool responses via
 *      `tokenless compress-response`.
 *
 * Stats are recorded automatically by tokenless compress-response / compress-schema.
 * Context passing uses environment variables (TOKENLESS_AGENT_ID,
 * TOKENLESS_SESSION_ID, TOKENLESS_TOOL_USE_ID) which are inherited by
 * child processes and read by RTK's stats patch.
 */

import { execSync, execFileSync, spawnSync } from "child_process";

// ---- Session ID mapping --------------------------------------------------------
// OpenClaw's tool_result_persist ctx provides sessionKey ("agent:main:main")
// but NOT sessionId (UUID). We maintain a sessionKey → sessionId map built
// from session_start events so response compression can use the correct UUID.

const sessionMap: Map<string, string> = new Map();

// ---- Binary availability cache ------------------------------------------------

let rtkAvailable: boolean | null = null;
let tokenlessAvailable: boolean | null = null;

function checkRtk(): boolean {
  if (rtkAvailable !== null) return rtkAvailable;
  try {
    execSync("which rtk", { stdio: "ignore" });
    rtkAvailable = true;
  } catch {
    rtkAvailable = false;
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
    execSync("which tokenless", { stdio: "ignore" });
    tokenlessAvailable = true;
  } catch {
    tokenlessAvailable = false;
  }
  return tokenlessAvailable;
}

// ---- Subprocess helpers -------------------------------------------------------

function tryRtkRewrite(command: string): string | null {
  try {
    const result = spawnSync("rtk", ["rewrite", command], {
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
    const result = execFileSync("tokenless", args, {
      encoding: "utf-8",
      timeout: 3000,
      input,
    }).trim();
    return JSON.parse(result);
  } catch {
    return null;
  }
}

// ---- Plugin entry point -------------------------------------------------------

export default {
  id: "tokenless-openclaw",
  name: "Token-Less",
  version: "5.0.0",
  description: "Unified RTK command rewriting + response compression",
  register(api: any) {
  const pluginConfig = api.config ?? {};
  const rtkEnabled = pluginConfig.rtk_enabled !== false;
  const responseCompressionEnabled = pluginConfig.response_compression_enabled !== false;
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

  // ---- 1. RTK command rewriting (before_tool_call) ----------------------------

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

  // ---- 2. Response compression (tool_result_persist) --------------------------

  if (responseCompressionEnabled && checkTokenless()) {
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

        const compressed = tryCompressResponse(event.message, sessionId, toolCallId);
        if (!compressed) return;

        if (verbose) {
          const beforeChars = JSON.stringify(event.message).length;
          const afterChars = JSON.stringify(compressed).length;
          console.log(
            `[tokenless/response] ${event.toolName}: ${beforeChars} -> ${afterChars} chars (${Math.round((1 - afterChars / beforeChars) * 100)}% reduction)`,
          );
        }

        return { message: compressed };
      },
      { priority: 10 },
    );
  }

  // ---- Done -------------------------------------------------------------------

  if (verbose) {
    const features = [
      rtkEnabled && rtkAvailable ? "rtk-rewrite" : null,
      responseCompressionEnabled && tokenlessAvailable ? "response-compression" : null,
    ].filter(Boolean);
    console.log(`[tokenless] OpenClaw plugin v5 registered — active features: ${features.join(", ") || "none"}`);
  }
  },
};
