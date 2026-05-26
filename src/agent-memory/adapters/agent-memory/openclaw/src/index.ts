/**
 * agent-memory OpenClaw plugin entry point.
 *
 * Registers 4 memory tools (memory_search, memory_get, memory_observe,
 * memory_get_context) backed by the agent-memory MCP server running as
 * a stdio subprocess. The plugin is a memory-slot candidate: setting
 * `plugins.slots.memory: "agent-memory"` makes OpenClaw use these
 * tools for active-memory recall.
 */

import { definePluginEntry, type OpenClawPluginApi } from "openclaw/plugin-sdk/plugin-entry";
import { Type } from "typebox";
import { McpStdioClient } from "./mcp-client.js";
import { resolveConfig, type AgentMemoryConfig } from "./config.js";

// Module-scoped singleton client. OpenClaw may call register() again
// during a plugin hot-reload without firing gateway_stop for the old
// instance, which previously left an orphan agent-memory subprocess
// holding the sqlite/git locks. Re-register tears the prior one down
// first (fire-and-forget — the new client must not wait on stale
// shutdown for its lazy-start to begin).
let activeClient: McpStdioClient | null = null;

export default definePluginEntry({
  id: "memory-anolisa",
  name: "Anolisa Memory",
  description:
    "Persistent memory backed by the agent-memory MCP server with namespace isolation and openat2 sandbox.",
  kind: "memory",
  register(api: OpenClawPluginApi) {
    const config: AgentMemoryConfig = resolveConfig(api);

    if (activeClient) {
      const stale = activeClient;
      api.logger.warn?.(
        "agent-memory: previous client still active during register() — tearing it down (hot-reload?)",
      );
      stale.stop().catch((err: unknown) => {
        api.logger.warn?.(
          `agent-memory: stale-client teardown failed (${err instanceof Error ? err.message : String(err)})`,
        );
      });
    }

    const client = new McpStdioClient(config);
    activeClient = client;

    api.logger.info(
      `agent-memory: plugin registered (binary=${config.binaryPath}, uid=${config.userId}, profile=${config.profile}, session=${config.sessionId})`,
    );

    // Register memory capability so this plugin can own the memory slot.
    api.registerMemoryCapability?.({
      publicArtifacts: {
        async listArtifacts() {
          return [];
        },
      },
    });

    // ---- memory_search ----
    api.registerTool(
      {
        name: "memory_search",
        label: "Memory Search (agent-memory)",
        description:
          "BM25 search across indexed memory files. Returns ranked snippets as JSON. Prefer this over mem_grep for large stores.",
        parameters: Type.Object({
          query: Type.String({ description: "Search query" }),
          top_k: Type.Optional(
            Type.Integer({ minimum: 1, description: "Max results (default: 5)" }),
          ),
        }),
        async execute(_toolCallId: string, params: Record<string, unknown>) {
          try {
            // callTool throws on isError:true, so reaching this line
            // means the server returned a real result. The payload is
            // a JSON array of hits; count the entries directly rather
            // than guessing from line breaks.
            const text = await client.callTool("memory_search", params);
            let count = 0;
            try {
              const parsed = JSON.parse(text);
              if (Array.isArray(parsed)) count = parsed.length;
            } catch {
              // Server returned a non-JSON string (e.g. when the index
              // is disabled). Leave count at 0 rather than guess.
            }
            return {
              content: [{ type: "text", text }],
              details: {
                debug: {
                  backend: "agent-memory",
                  effectiveMode: "bm25",
                },
                count,
              },
            };
          } catch (err) {
            const msg = err instanceof Error ? err.message : String(err);
            return {
              content: [{ type: "text", text: `Search error: ${msg}` }],
              details: {
                error: msg,
                debug: {
                  backend: "agent-memory",
                  effectiveMode: "bm25",
                },
              },
            };
          }
        },
      },
      { names: ["memory_search"] },
    );

    // ---- memory_get ----
    api.registerTool(
      {
        name: "memory_get",
        label: "Memory Get (agent-memory)",
        description:
          "Read a memory file by path. Returns full UTF-8 content. Path is relative to the mount root.",
        parameters: Type.Object({
          path: Type.String({ description: "File path relative to memory mount root" }),
        }),
        async execute(_toolCallId: string, params: Record<string, unknown>) {
          try {
            // OpenClaw "memory_get" maps to agent-memory "mem_read".
            const text = await client.callTool("memory_get", params);
            return {
              content: [{ type: "text", text }],
              details: { path: params.path as string },
            };
          } catch (err) {
            const msg = err instanceof Error ? err.message : String(err);
            return {
              content: [{ type: "text", text: `Read error: ${msg}` }],
              details: { error: msg },
            };
          }
        },
      },
      { names: ["memory_get"] },
    );

    // ---- memory_observe ----
    api.registerTool(
      {
        name: "memory_observe",
        label: "Memory Observe (agent-memory)",
        description:
          "Record an observation. The OS picks notes/observed/<ulid>.md, writes frontmatter + body. Returns the relative path.",
        parameters: Type.Object({
          content: Type.String({ description: "Observation content to record" }),
          hint: Type.Optional(Type.String({ description: "Optional path hint" })),
        }),
        async execute(_toolCallId: string, params: Record<string, unknown>) {
          try {
            const text = await client.callTool("memory_observe", params);
            // Parse the server's text reply robustly; agent-memory's
            // current shape is `observed at <relpath>` but we anchor on
            // a regex so a wording tweak in the server doesn't silently
            // poison `details.path`.
            const match = /^observed at (.+)$/.exec(text.trim());
            return {
              content: [{ type: "text", text }],
              details: { action: "observed", path: match ? match[1] : undefined },
            };
          } catch (err) {
            const msg = err instanceof Error ? err.message : String(err);
            return {
              content: [{ type: "text", text: `Observe error: ${msg}` }],
              details: { error: msg },
            };
          }
        },
      },
      { names: ["memory_observe"] },
    );

    // ---- memory_get_context ----
    api.registerTool(
      {
        name: "memory_get_context",
        label: "Memory Get Context (agent-memory)",
        description:
          "Assemble a token-bounded context from recently modified memory files. Returns markdown with previews, capped at roughly max_tokens*4 bytes.",
        parameters: Type.Object({
          max_tokens: Type.Optional(
            Type.Integer({ minimum: 1, description: "Token budget (default: 2048)" }),
          ),
        }),
        async execute(_toolCallId: string, params: Record<string, unknown>) {
          try {
            const text = await client.callTool("memory_get_context", params);
            return {
              content: [{ type: "text", text }],
              details: { tokenBudget: (params.max_tokens as number) ?? 2048 },
            };
          } catch (err) {
            const msg = err instanceof Error ? err.message : String(err);
            return {
              content: [{ type: "text", text: `Context error: ${msg}` }],
              details: { error: msg },
            };
          }
        },
      },
      { names: ["memory_get_context"] },
    );

    // Clean up the subprocess when the gateway shuts down. The
    // handler is declared async and **returns** the stop() promise so
    // an OpenClaw runtime that awaits its lifecycle hooks blocks
    // until the SIGTERM/SIGKILL grace window completes; otherwise
    // the child would survive as a kernel orphan past gateway exit.
    api.on("gateway_stop", async () => {
      try {
        await client.stop();
      } catch (err: unknown) {
        api.logger.warn?.(
          `agent-memory: gateway_stop cleanup error (${err instanceof Error ? err.message : String(err)})`,
        );
      } finally {
        if (activeClient === client) {
          activeClient = null;
        }
      }
    });
  },
});