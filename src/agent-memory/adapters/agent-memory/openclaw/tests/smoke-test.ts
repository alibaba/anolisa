/**
 * Smoke test: verifies the agent-memory MCP server can be started
 * and responds to tool calls via the McpStdioClient.
 *
 * Requires `agent-memory` binary to be available on PATH or at a
 * known location. If the binary is not found, the test is skipped.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { execSync } from "node:child_process";
import { McpStdioClient } from "../src/mcp-client.js";

function findBinary(): string | null {
  try {
    const result = execSync("which agent-memory 2>/dev/null", {
      encoding: "utf8",
      timeout: 5000,
    }).trim();
    if (result && fs.existsSync(result)) {
      return result;
    }
  } catch {
    // Not on PATH.
  }

  const candidates = [
    "/usr/local/bin/agent-memory",
    "/usr/bin/agent-memory",
  ];
  for (const loc of candidates) {
    if (fs.existsSync(loc)) {
      try {
        fs.accessSync(loc, fs.constants.X_OK);
        return loc;
      } catch {
        continue;
      }
    }
  }
  return null;
}

const binaryPath = findBinary();

// Skip entire suite if binary is not available.
const skip = binaryPath === null;

describe("agent-memory MCP smoke test", { skip }, () => {
  const client = new McpStdioClient({
    binaryPath: binaryPath!,
    userId: String(process.getuid?.() ?? 0),
    profile: "advanced",
    maxReadBytes: 1_048_576,
    maxWriteBytes: 16_777_216,
  });

  it("calls memory_search and returns a result", async () => {
    const result = await client.callTool("memory_search", { query: "test query", top_k: 3 });
    assert.ok(typeof result === "string");
    assert.ok(result.length > 0);
  });

  it("calls memory_get (mem_read) and returns a result", async () => {
    const result = await client.callTool("memory_get", { path: "README.md" });
    // mem_read may return file content or an error string; both are valid responses.
    assert.ok(typeof result === "string");
  });

  it("calls memory_observe and returns a result", async () => {
    const result = await client.callTool("memory_observe", {
      content: "smoke test observation",
      hint: "smoke",
    });
    assert.ok(typeof result === "string");
    assert.ok(result.includes("observed"));
  });

  it("calls memory_get_context and returns a result", async () => {
    const result = await client.callTool("memory_get_context", { max_tokens: 100 });
    assert.ok(typeof result === "string");
  });

  it("stops cleanly", async () => {
    await client.stop();
    // Second stop should also be safe.
    await client.stop();
  });
});