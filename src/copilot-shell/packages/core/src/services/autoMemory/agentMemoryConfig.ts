/**
 * @license
 * Copyright 2026 Qwen Team
 * SPDX-License-Identifier: Apache-2.0
 */

/**
 * Shared configuration for agent-memory command hooks.
 *
 * These hooks are registered as command hooks in copilot-shell's
 * hooksConfig. They spawn the agent-memory MCP server via stdio,
 * call memory_search / memory_observe, and return results.
 */

/** agent-memory binary path (env override or default). */
export const AGENT_MEMORY_BINARY =
  process.env['AGENT_MEMORY_BINARY'] ?? 'agent-memory';

/** Hook timeout in milliseconds. */
export const HOOK_TIMEOUT_MS = 5000;

/** Max memories to recall. */
export const RECALL_TOP_K = 5;

/** Search mode: bm25 | vector | hybrid. */
export const SEARCH_MODE = 'hybrid';

/** Dedup TTL for auto-capture (ms). */
export const DEDUP_TTL_MS = 5 * 60 * 1000;

/** Max content length for auto-capture. */
export const MAX_CAPTURE_LENGTH = 2000;

/**
 * Trigger keywords for auto-capture — only capture when the assistant
 * mentions decisions, findings, preferences, or notable items.
 */
export const CAPTURE_TRIGGERS = [
  /\b(I decided|I've decided|my decision|I will remember)\b/i,
  /\b(the answer is|the solution is|I found that|it turns out)\b/i,
  /\b(user prefers|user wants|user's preference|you prefer|you want)\b/i,
  /\b(important|critical|key|notable|significant)\b/i,
  /\b(I should note|I should remember|notable observation)\b/i,
];

/**
 * HTML-escape a string for safe inclusion in a model prompt.
 * Prevents memory content from being interpreted as markup.
 */
export function htmlEscape(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

/**
 * Wrap memory search results in a <relevant-memories> block with
 * HTML-escaped content for prompt-injection safety.
 */
export function wrapMemoryResults(text: string): string {
  return `<relevant-memories>\n${htmlEscape(text)}\n</relevant-memories>`;
}
