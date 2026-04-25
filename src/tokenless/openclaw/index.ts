/**
 * Token-Less Unified Plugin for OpenClaw
 *
 * Combines two complementary optimisation strategies into a single plugin:
 *
 *   1. RTK command rewriting  — transparently rewrites exec tool commands to
 *      their RTK equivalents (delegated to `rtk rewrite`).
 *   2. Tokenless schema / response compression — compresses tool schemas and
 *      tool responses via `tokenless compress-schema` / `tokenless compress-response`.
 *
 * Both features are thin delegates that shell out to their respective binaries.
 * If a binary is missing the corresponding feature is silently disabled.
 *
 * Design principles:
 *   - Non-blocking: every operation has a timeout guard and failures passthrough.
 *   - Lazy binary detection: `which` is called at most once per binary.
 *   - Zero runtime dependencies beyond Node built-ins.
 */

import { execSync, execFileSync, spawnSync } from "child_process";

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
    // Use spawnSync instead of execSync because rtk uses exit code 3 to signal
    // "command was rewritten" — execSync throws on any non-zero exit code,
    // which would silently discard the rewritten result.
    const result = spawnSync("rtk", ["rewrite", command], {
      encoding: "utf-8",
      timeout: 2000,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const rewritten = result.stdout?.trim();
    // Exit 0 or 3 = success (3 means rewritten), exit 1 = no rewrite needed
    if ((result.status === 0 || result.status === 3) && rewritten && rewritten !== command) {
      return rewritten;
    }
    return null;
  } catch {
    return null;
  }
}

function tryCompressSchema(schema: any): any | null {
  try {
    const input = JSON.stringify(schema);
    const result = execFileSync("tokenless", ["compress-schema"], {
      encoding: "utf-8",
      timeout: 3000,
      input,
    }).trim();
    return JSON.parse(result);
  } catch {
    return null;
  }
}

function tryCompressResponse(response: any): any | null {
  try {
    const input = JSON.stringify(response);
    const result = execFileSync("tokenless", ["compress-response"], {
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
  version: "1.0.0",
  description: "Unified RTK command rewriting + schema/response compression",
  register(api: any) {
  const pluginConfig = api.config ?? {};
  const rtkEnabled = pluginConfig.rtk_enabled !== false;
  const schemaCompressionEnabled = pluginConfig.schema_compression_enabled !== false;
  const responseCompressionEnabled = pluginConfig.response_compression_enabled !== false;
  const skipTools: Set<string> = new Set((pluginConfig.skip_tools ?? ["Read", "read_file", "Glob", "list_directory", "NotebookRead"]).map((t: string) => t.toLowerCase()));
  const verbose = pluginConfig.verbose !== false;

  // ---- 1. RTK command rewriting (before_tool_call) ----------------------------

  if (rtkEnabled && checkRtk()) {
    api.on(
      "before_tool_call",
      (event: { toolName: string; params: Record<string, unknown> }) => {
        if (event.toolName !== "exec") return;

        const command = event.params?.command;
        if (typeof command !== "string") return;

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

  // ---- 2. Schema compression (before_tool_register) ---------------------------

  if (schemaCompressionEnabled && checkTokenless()) {
    api.on(
      "before_tool_register",
      (event: { toolName: string; schema: Record<string, unknown> }) => {
        const compressed = tryCompressSchema(event.schema);
        if (!compressed) return;

        if (verbose) {
          const before = JSON.stringify(event.schema).length;
          const after = JSON.stringify(compressed).length;
          console.log(
            `[tokenless/schema] ${event.toolName}: ${before} -> ${after} chars (${Math.round((1 - after / before) * 100)}% reduction)`,
          );
        }

        return { schema: compressed };
      },
      { priority: 10 },
    );
  }

  // ---- 3. Response compression (tool_result_persist) -------------------------

  if (responseCompressionEnabled && checkTokenless()) {
    api.on(
      "tool_result_persist",
      (event: { toolName: string; toolCallId?: string; message: any }) => {
        // Skip exec tool results when RTK is enabled — RTK already produces
        // optimized output, double-compression is wasteful.
        if (rtkEnabled && rtkAvailable && event.toolName === "exec") return;

        // Skip content-retrieval tools — agent needs complete responses
        if (event.toolName && skipTools.has(event.toolName.toLowerCase())) return;

        // Skip skill content to avoid breaking YAML frontmatter metadata.
        if (isSkillContent(event.message)) return;

        const compressed = tryCompressResponse(event.message);
        if (!compressed) return;

        if (verbose) {
          const before = JSON.stringify(event.message).length;
          const after = JSON.stringify(compressed).length;
          console.log(
            `[tokenless/response] ${event.toolName}: ${before} -> ${after} chars (${Math.round((1 - after / before) * 100)}% reduction)`,
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
      schemaCompressionEnabled && tokenlessAvailable ? "schema-compression" : null,
      responseCompressionEnabled && tokenlessAvailable ? "response-compression" : null,
    ].filter(Boolean);
    console.log(`[tokenless] OpenClaw plugin registered — active features: ${features.join(", ") || "none"}`);
  }
  },
};
