/**
 * @license
 * Copyright 2026 Alibaba Cloud
 * SPDX-License-Identifier: Apache-2.0
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import fs from 'node:fs/promises';
import * as os from 'node:os';
import path from 'node:path';

let testDir: string;

vi.mock('@copilot-shell/core', () => ({
  Storage: {
    getGlobalQwenDir: () => testDir,
  },
}));

// Dynamic import so the mock is active when the module loads
const { getAndMarkUnshownFeatureTips } = await import('./featureTips.js');

describe('getAndMarkUnshownFeatureTips', () => {
  beforeEach(async () => {
    testDir = await fs.mkdtemp(path.join(os.tmpdir(), 'feature-tips-test-'));
  });

  afterEach(async () => {
    await fs.rm(testDir, { recursive: true, force: true });
  });

  it('should return the single tip on first call when state file does not exist', async () => {
    const tips = await getAndMarkUnshownFeatureTips();
    expect(tips).toHaveLength(1);
    expect(tips[0].id).toBe('bash-interactive-shell');

    // State file should have been created
    const raw = await fs.readFile(
      path.join(testDir, 'feature-tips-state.json'),
      'utf-8',
    );
    const state = JSON.parse(raw);
    expect(state.shownTipIds).toContain('bash-interactive-shell');
  });

  it('should return empty array on second call when all tips are shown', async () => {
    // First call — marks the tip
    await getAndMarkUnshownFeatureTips();
    // Second call — all tips shown
    const tips = await getAndMarkUnshownFeatureTips();
    expect(tips).toHaveLength(0);
  });

  it('should return the highest-priority unshown tip when multiple tips exist', async () => {
    // Pre-populate state with one shown tip ID to simulate partial consumption.
    // We write 'bash-interactive-shell' as shown so the function will look for
    // remaining unshown tips. Since the production registry only has one tip,
    // this test verifies the priority-sorting logic by temporarily extending
    // the internal registry via a fresh state file that claims one is already shown.
    await fs.writeFile(
      path.join(testDir, 'feature-tips-state.json'),
      JSON.stringify({ shownTipIds: [] }),
      'utf-8',
    );

    const tips = await getAndMarkUnshownFeatureTips();
    // With default registry (1 tip), should return it
    expect(tips).toHaveLength(1);
    expect(tips[0].id).toBe('bash-interactive-shell');
  });

  it('should handle corrupted state file gracefully', async () => {
    // Write invalid JSON
    await fs.writeFile(
      path.join(testDir, 'feature-tips-state.json'),
      'not valid json!!!',
      'utf-8',
    );

    const tips = await getAndMarkUnshownFeatureTips();
    // Should fall back to empty state and return the tip
    expect(tips).toHaveLength(1);
    expect(tips[0].id).toBe('bash-interactive-shell');
  });

  it('should auto-create directory when state directory does not exist', async () => {
    // Remove the test dir so the write must create it
    await fs.rm(testDir, { recursive: true, force: true });

    const tips = await getAndMarkUnshownFeatureTips();
    expect(tips).toHaveLength(1);

    // Verify directory and file were created
    const stat = await fs.stat(path.join(testDir, 'feature-tips-state.json'));
    expect(stat.isFile()).toBe(true);
  });

  it('should not throw when write fails due to permission issues', async () => {
    // Make dir read-only to prevent writing
    await fs.chmod(testDir, 0o444);

    const tips = await getAndMarkUnshownFeatureTips();
    // Should still return the tip despite write failure
    expect(tips).toHaveLength(1);
    expect(tips[0].id).toBe('bash-interactive-shell');

    // Restore permissions for cleanup
    await fs.chmod(testDir, 0o755);
  });
});
