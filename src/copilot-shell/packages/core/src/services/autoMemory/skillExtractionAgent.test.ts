/**
 * @license
 * Copyright 2025 Qwen Code
 * SPDX-License-Identifier: Apache-2.0
 */

import { describe, it, expect } from 'vitest';
import * as path from 'node:path';
import { createExtractionHooks } from './skillExtractionAgent.js';
import type { PostToolUsePayload } from '../../subagents/subagent-hooks.js';

const basePayload = {
  subagentId: 'sub-1',
  name: 'auto-memory-extractor',
  durationMs: 0,
  timestamp: Date.now(),
};

function makePayload(
  toolName: string,
  args: Record<string, unknown>,
  success = true,
): PostToolUsePayload {
  return { ...basePayload, toolName, args, success };
}

describe('createExtractionHooks session-read tracking', () => {
  const chatsDir = path.resolve('/tmp/qoder-chats-hook');
  const uuidA = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa';
  const uuidB = 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb';

  it('reports sessionId when read_file reads a chat session file', async () => {
    const reads: string[] = [];
    const hooks = createExtractionHooks({
      chatsDir,
      onSessionRead: (sid) => reads.push(sid),
    });
    await hooks.postToolUse?.(
      makePayload('read_file', {
        absolute_path: path.join(chatsDir, `${uuidA}.jsonl`),
      }),
    );
    expect(reads).toEqual([uuidA]);
  });

  it('reports multiple sessionIds from read_many_files paths', async () => {
    const reads: string[] = [];
    const hooks = createExtractionHooks({
      chatsDir,
      onSessionRead: (sid) => reads.push(sid),
    });
    await hooks.postToolUse?.(
      makePayload('read_many_files', {
        paths: [
          path.join(chatsDir, `${uuidA}.jsonl`),
          path.join(chatsDir, `${uuidB}.jsonl`),
          '/tmp/not-a-session.txt',
        ],
      }),
    );
    expect(reads.sort()).toEqual([uuidA, uuidB].sort());
  });

  it('ignores non-chat paths outside chatsDir', async () => {
    const reads: string[] = [];
    const hooks = createExtractionHooks({
      chatsDir,
      onSessionRead: (sid) => reads.push(sid),
    });
    await hooks.postToolUse?.(
      makePayload('read_file', { absolute_path: '/etc/passwd' }),
    );
    expect(reads).toEqual([]);
  });

  it('does not report when tool call failed', async () => {
    const reads: string[] = [];
    const hooks = createExtractionHooks({
      chatsDir,
      onSessionRead: (sid) => reads.push(sid),
    });
    await hooks.postToolUse?.(
      makePayload(
        'read_file',
        { absolute_path: path.join(chatsDir, `${uuidA}.jsonl`) },
        /* success */ false,
      ),
    );
    expect(reads).toEqual([]);
  });

  it('is a no-op on session tracking when no options are provided', async () => {
    const hooks = createExtractionHooks();
    // Should simply not throw, and not observe any side-effect.
    await hooks.postToolUse?.(
      makePayload('read_file', {
        absolute_path: path.join(chatsDir, `${uuidA}.jsonl`),
      }),
    );
  });

  it('does not report for non-read tools', async () => {
    const reads: string[] = [];
    const hooks = createExtractionHooks({
      chatsDir,
      onSessionRead: (sid) => reads.push(sid),
    });
    await hooks.postToolUse?.(
      makePayload('glob', {
        pattern: path.join(chatsDir, '*.jsonl'),
      }),
    );
    expect(reads).toEqual([]);
  });
});
