#!/usr/bin/env node
/**
 * @license
 * Copyright 2026 Qwen Team
 * SPDX-License-Identifier: Apache-2.0
 */

/**
 * Agent Memory Capture Hook — command hook for Stop event.
 *
 * Reads StopInput from stdin ({ lastAssistantMessage: string, ... }),
 * filters for notable content (decisions, findings, preferences),
 * deduplicates via SHA-256, and calls memory_observe on agent-memory
 * via MCP stdio.
 *
 * Registered in copilot-shell hooksConfig as:
 *   "Stop": [{ "hooks": [{ "type": "command",
 *     "command": "node .../agentMemoryCapture.mjs", "timeout": 5000 }] }]
 */

import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { createHash } from 'node:crypto';
import {
  AGENT_MEMORY_BINARY,
  CAPTURE_TRIGGERS,
  DEDUP_TTL_MS,
  MAX_CAPTURE_LENGTH,
} from './agentMemoryConfig.js';

/** SHA-256 dedup cache (prevents re-capturing same content). */
const captureDedupCache = new Map();

/**
 * Check if content was recently captured.
 */
function wasRecentlyCaptured(content) {
  const hash = createHash('sha256').update(content).digest('hex').slice(0, 16);
  const now = Date.now();

  // Evict stale entries
  for (const [key, ts] of captureDedupCache) {
    if (now - ts > DEDUP_TTL_MS) {
      captureDedupCache.delete(key);
    }
  }

  if (captureDedupCache.has(hash)) {
    return true;
  }
  captureDedupCache.set(hash, now);
  return false;
}

/**
 * Check if content matches any capture trigger.
 */
function shouldCapture(content) {
  return CAPTURE_TRIGGERS.some((re) => re.test(content));
}

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
 * Connect to agent-memory MCP server via stdio and call memory_observe.
 */
async function observeMemory(content) {
  const transport = new StdioClientTransport({
    command: AGENT_MEMORY_BINARY,
    args: [],
    env: {
      ...process.env,
      MEMORY_MOUNT_STRATEGY: 'userland',
    },
  });

  const client = new Client(
    { name: 'cosh-capture-hook', version: '1.0.0' },
    { capabilities: {} },
  );

  try {
    await client.connect(transport);
    await client.callTool({
      name: 'memory_observe',
      arguments: {
        content,
        hint: 'auto-capture',
      },
    });
  } finally {
    await client.close().catch(() => {});
  }
}

/**
 * Main entry point.
 */
async function main() {
  const input = await readStdin();
  const assistantMessage = input.lastAssistantMessage ?? '';

  if (!assistantMessage || assistantMessage.length < 20) {
    process.stdout.write(JSON.stringify({}));
    return;
  }

  // Dedup check
  if (wasRecentlyCaptured(assistantMessage)) {
    process.stdout.write(JSON.stringify({}));
    return;
  }

  // Trigger check
  if (!shouldCapture(assistantMessage)) {
    process.stdout.write(JSON.stringify({}));
    return;
  }

  // Truncate
  const content = assistantMessage.slice(0, MAX_CAPTURE_LENGTH);

  try {
    await observeMemory(content);
  } catch (err) {
    // Fire-and-forget: log to stderr, don't block
    process.stderr.write(`agent-memory capture hook failed: ${err}\n`);
  }

  // Always output empty JSON — capture doesn't modify context
  process.stdout.write(JSON.stringify({}));
}

main().catch((err) => {
  process.stderr.write(`agent-memory capture hook error: ${err}\n`);
  process.stdout.write(JSON.stringify({}));
  process.exit(0); // exit 0 — never block the session
});
