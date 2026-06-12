#!/usr/bin/env node
/**
 * @license
 * Copyright 2026 Qwen Team
 * SPDX-License-Identifier: Apache-2.0
 */

/**
 * Agent Memory Recall Hook — command hook for UserPromptSubmit event.
 *
 * Reads UserPromptSubmitInput from stdin ({ prompt: string, ... }),
 * searches agent-memory for relevant context via MCP stdio, and
 * outputs UserPromptSubmitOutput with additionalContext on stdout.
 *
 * Registered in copilot-shell hooksConfig as:
 *   "UserPromptSubmit": [{ "hooks": [{ "type": "command",
 *     "command": "node .../agentMemoryRecall.mjs", "timeout": 5000 }] }]
 */

import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { createHash } from 'node:crypto';
import {
  AGENT_MEMORY_BINARY,
  HOOK_TIMEOUT_MS,
  RECALL_TOP_K,
  SEARCH_MODE,
  wrapMemoryResults,
} from './agentMemoryConfig.js';

/**
 * Read all stdin and parse as JSON.
 */
function readStdin() {
  return new Promise((resolve, reject) => {
    let data = '';
    process.stdin.setEncoding('utf8');
    process.stdin.on('data', (chunk) => {
      data += chunk;
    });
    process.stdin.on('end', () => {
      try {
        resolve(data.trim() ? JSON.parse(data) : {});
      } catch (err) {
        reject(err);
      }
    });
    process.stdin.on('error', reject);
  });
}

/**
 * Connect to agent-memory MCP server via stdio and call memory_search.
 */
async function searchMemories(query) {
  const transport = new StdioClientTransport({
    command: AGENT_MEMORY_BINARY,
    args: [],
    env: {
      ...process.env,
      MEMORY_MOUNT_STRATEGY: 'userland',
    },
  });

  const client = new Client(
    { name: 'cosh-recall-hook', version: '1.0.0' },
    { capabilities: {} },
  );

  try {
    await client.connect(transport);

    const result = await client.callTool({
      name: 'memory_search',
      arguments: {
        query,
        top_k: RECALL_TOP_K,
        mode: SEARCH_MODE,
      },
    });

    const text = result.content?.[0]?.text ?? '';
    return text;
  } finally {
    await client.close().catch(() => {});
  }
}

/**
 * Main entry point.
 */
async function main() {
  const input = await readStdin();
  const prompt = input.prompt ?? '';

  if (!prompt || prompt.trim().length < 3) {
    process.stdout.write(JSON.stringify({}));
    return;
  }

  try {
    const searchResult = await searchMemories(prompt);
    if (searchResult && searchResult.trim()) {
      const additionalContext = wrapMemoryResults(searchResult);
      process.stdout.write(
        JSON.stringify({
          hookSpecificOutput: {
            hookEventName: 'UserPromptSubmit',
            additionalContext,
          },
        }),
      );
    } else {
      process.stdout.write(JSON.stringify({}));
    }
  } catch (err) {
    // Fire-and-forget: log to stderr, output empty JSON
    process.stderr.write(`agent-memory recall hook failed: ${err}\n`);
    process.stdout.write(JSON.stringify({}));
  }
}

main().catch((err) => {
  process.stderr.write(`agent-memory recall hook error: ${err}\n`);
  process.stdout.write(JSON.stringify({}));
  process.exit(0); // exit 0 — never block the session
});
