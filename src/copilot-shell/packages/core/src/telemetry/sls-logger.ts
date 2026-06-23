/**
 * @license
 * Copyright 2026 Alibaba Cloud
 * SPDX-License-Identifier: Apache-2.0
 */

import fs from 'node:fs';

/**
 * Path to the SLS JSONL log file.
 * SLS agent will collect and upload logs from this file.
 */
const SLS_LOG_PATH = '/var/log/anolisa/sls/ops/cosh.jsonl';

/**
 * Append a single JSON record as one line to the SLS log file.
 * Uses open→write→close pattern to support logrotate rename.
 * Silently fails if the file cannot be written (e.g., directory does not exist).
 */
export function appendSlsLog(record: Record<string, unknown>): void {
  try {
    const line = JSON.stringify(record) + '\n';
    const fd = fs.openSync(SLS_LOG_PATH, 'a');
    try {
      fs.writeSync(fd, line);
    } finally {
      fs.closeSync(fd);
    }
  } catch {
    // Silently fail - SLS logging should never break the main process
  }
}
