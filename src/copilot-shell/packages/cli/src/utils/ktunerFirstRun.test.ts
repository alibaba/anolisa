/**
 * @license
 * Copyright 2025 Google LLC
 * SPDX-License-Identifier: Apache-2.0
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { EventEmitter } from 'node:events';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { spawn } from 'node:child_process';
import type { HistoryItem } from '../ui/types.js';

// Point the sentinel at a real temp dir (per featureTips.test.ts precedent) so
// the once-guard / retry logic is exercised against a real filesystem.
let testDir: string;
vi.mock('@copilot-shell/core', () => ({
  Storage: {
    getGlobalQwenDir: () => testDir,
  },
}));

vi.mock('node:child_process');

// Controls for the trusted-path resolver (resolveKtunerBinary). Real fs is kept
// for the sentinel logic; only realpathSync/statSync on the trusted candidates
// are stubbed so the resolver is deterministic regardless of the test host
// (CI has no /usr/bin/ktuner; a dev box might).
const resolverCtl = vi.hoisted(() => ({
  ktunerPresent: true, // does /usr/bin/ktuner realpath-resolve
  ktunerFileUid: 0, // owner uid of the ktuner file (0 = root/trusted)
}));

vi.mock('node:fs', async (importOriginal) => {
  const actual = await importOriginal<typeof import('node:fs')>();
  return {
    ...actual,
    realpathSync: (p: fs.PathLike, ...rest: unknown[]) => {
      const s = String(p);
      if (s === '/usr/bin/ktuner') {
        if (!resolverCtl.ktunerPresent) {
          throw new Error('ENOENT');
        }
        return '/usr/bin/ktuner';
      }
      if (s === '/usr/local/bin/ktuner') {
        throw new Error('ENOENT');
      }
      return (actual.realpathSync as (...a: unknown[]) => string)(p, ...rest);
    },
    statSync: (p: fs.PathLike, ...rest: unknown[]) => {
      const s = String(p);
      if (s === '/usr/bin/ktuner') {
        return {
          isFile: () => true,
          isDirectory: () => false,
          uid: resolverCtl.ktunerFileUid,
          mode: 0o755,
        } as unknown as fs.Stats;
      }
      if (s === '/usr/bin' || s === '/usr' || s === '/') {
        return {
          isFile: () => false,
          isDirectory: () => true,
          uid: 0,
          mode: 0o755,
        } as unknown as fs.Stats;
      }
      return (actual.statSync as (...a: unknown[]) => fs.Stats)(p, ...rest);
    },
  };
});

let savedPlatform: PropertyDescriptor | undefined;
function forcePlatform(value: string): void {
  if (!savedPlatform) {
    savedPlatform = Object.getOwnPropertyDescriptor(process, 'platform');
  }
  Object.defineProperty(process, 'platform', { value, configurable: true });
}
function restorePlatform(): void {
  if (savedPlatform) {
    Object.defineProperty(process, 'platform', savedPlatform);
    savedPlatform = undefined;
  }
}

// Dynamic import so the mocks are active when the module loads.
const { parseKtunerCheck, maybeRunKtunerFirstRunCheck } =
  await import('./ktunerFirstRun.js');

const mockSpawn = spawn as unknown as ReturnType<typeof vi.fn>;

const SENTINEL = '.ktuner-firstrun-checked';

/**
 * Make mockSpawn return a fake child that, on the next tick, emits the given
 * stdout then closes with `code` — or emits `error` (ktuner not on PATH).
 */
function stubSpawn(opts: { stdout?: string; code?: number; error?: Error }): {
  kill: ReturnType<typeof vi.fn>;
} {
  const child = Object.assign(new EventEmitter(), {
    stdout: new EventEmitter(),
    kill: vi.fn(),
  });
  mockSpawn.mockImplementation(() => {
    process.nextTick(() => {
      if (opts.error) {
        child.emit('error', opts.error);
        return;
      }
      if (opts.stdout !== undefined) {
        child.stdout.emit('data', Buffer.from(opts.stdout));
      }
      child.emit('close', opts.code ?? 0);
    });
    return child as unknown as ReturnType<typeof spawn>;
  });
  return child;
}

function collectItems() {
  const items: Array<Omit<HistoryItem, 'id'>> = [];
  const addItem = (item: Omit<HistoryItem, 'id'>, _ts: number) => {
    items.push(item);
  };
  return { items, addItem };
}

describe('parseKtunerCheck', () => {
  it('parses a valid ktuner check report', () => {
    const out = JSON.stringify({
      score: 30,
      recommendations: [{}, {}, {}],
      counts: { high_confidence: 2 },
    });
    expect(parseKtunerCheck(out)).toEqual({
      score: 30,
      recommendations: 3,
      highConfidence: 2,
    });
  });

  it('defaults high-confidence to 0 when counts is absent', () => {
    const out = JSON.stringify({ score: 90, recommendations: [] });
    expect(parseKtunerCheck(out)).toEqual({
      score: 90,
      recommendations: 0,
      highConfidence: 0,
    });
  });

  it('returns null on non-JSON or empty output', () => {
    expect(parseKtunerCheck('not json')).toBeNull();
    expect(parseKtunerCheck('')).toBeNull();
  });

  it('returns null when the shape is wrong', () => {
    // score must be a number
    expect(
      parseKtunerCheck(JSON.stringify({ score: 'x', recommendations: [] })),
    ).toBeNull();
    // recommendations must be an array (missing here)
    expect(parseKtunerCheck(JSON.stringify({ score: 30 }))).toBeNull();
  });
});

describe('maybeRunKtunerFirstRunCheck', () => {
  beforeEach(() => {
    testDir = fs.mkdtempSync(path.join(os.tmpdir(), 'ktuner-firstrun-test-'));
    mockSpawn.mockReset();
    // Default: ktuner is present at a trusted root-owned path on Linux, so the
    // resolver succeeds and the spawn-based cases below exercise runKtunerCheck.
    resolverCtl.ktunerPresent = true;
    resolverCtl.ktunerFileUid = 0;
    forcePlatform('linux');
  });

  afterEach(() => {
    fs.rmSync(testDir, { recursive: true, force: true });
    restorePlatform();
  });

  const sentinel = () => path.join(testDir, SENTINEL);

  it('surfaces one report with correct score/count/high and writes the sentinel', async () => {
    stubSpawn({
      stdout: JSON.stringify({
        score: 42,
        recommendations: [{}, {}, {}, {}, {}],
        counts: { high_confidence: 3 },
      }),
      code: 1, // ktuner check exits 1 when it has recommendations — must be ignored
    });
    const { items, addItem } = collectItems();

    await maybeRunKtunerFirstRunCheck(addItem);

    expect(items).toHaveLength(1);
    const text = items[0].text ?? '';
    // Distinct numbers guard against a count<->high param swap.
    expect(text).toContain('42/100');
    expect(text).toContain('5 tuning suggestion');
    expect(text).toContain('3 high-confidence');
    expect(fs.existsSync(sentinel())).toBe(true);
  });

  it('does not nag when the system is already optimal (0 recommendations)', async () => {
    stubSpawn({
      stdout: JSON.stringify({ score: 100, recommendations: [] }),
      code: 0,
    });
    const { items, addItem } = collectItems();

    await maybeRunKtunerFirstRunCheck(addItem);

    expect(items).toHaveLength(0);
    // Ran successfully, so the sentinel IS written (don't re-check every auth).
    expect(fs.existsSync(sentinel())).toBe(true);
  });

  it('is idempotent: a second call short-circuits on the sentinel', async () => {
    stubSpawn({
      stdout: JSON.stringify({
        score: 30,
        recommendations: [{}],
        counts: { high_confidence: 0 },
      }),
    });
    const { items, addItem } = collectItems();

    await maybeRunKtunerFirstRunCheck(addItem);
    await maybeRunKtunerFirstRunCheck(addItem);

    expect(items).toHaveLength(1);
    // Second call must not even spawn ktuner again.
    expect(mockSpawn).toHaveBeenCalledTimes(1);
  });

  it('when ktuner is absent, emits nothing and does NOT write the sentinel (retry later)', async () => {
    stubSpawn({ error: new Error('spawn ktuner ENOENT') });
    const { items, addItem } = collectItems();

    await maybeRunKtunerFirstRunCheck(addItem);

    expect(items).toHaveLength(0);
    expect(fs.existsSync(sentinel())).toBe(false);
  });

  it('when ktuner prints garbage, emits nothing and does NOT write the sentinel', async () => {
    stubSpawn({ stdout: 'not json at all', code: 0 });
    const { items, addItem } = collectItems();

    await maybeRunKtunerFirstRunCheck(addItem);

    expect(items).toHaveLength(0);
    expect(fs.existsSync(sentinel())).toBe(false);
  });

  it('does not resolve/run ktuner on a non-Linux platform', async () => {
    forcePlatform('darwin');
    stubSpawn({
      stdout: JSON.stringify({ score: 10, recommendations: [{}] }),
    });
    const { items, addItem } = collectItems();

    await maybeRunKtunerFirstRunCheck(addItem);

    // Resolver returns null on non-Linux → ktuner is never spawned.
    expect(mockSpawn).not.toHaveBeenCalled();
    expect(items).toHaveLength(0);
    expect(fs.existsSync(sentinel())).toBe(false);
  });

  it('does not run a non-root-owned ktuner binary (PATH-shadow / tampering guard)', async () => {
    resolverCtl.ktunerFileUid = 1000; // not root-owned → untrusted
    stubSpawn({
      stdout: JSON.stringify({ score: 10, recommendations: [{}] }),
    });
    const { items, addItem } = collectItems();

    await maybeRunKtunerFirstRunCheck(addItem);

    expect(mockSpawn).not.toHaveBeenCalled();
    expect(items).toHaveLength(0);
    expect(fs.existsSync(sentinel())).toBe(false);
  });
});
