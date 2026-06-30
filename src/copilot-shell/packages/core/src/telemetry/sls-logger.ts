/**
 * @license
 * Copyright 2026 Alibaba Cloud
 * SPDX-License-Identifier: Apache-2.0
 */

import fs from 'node:fs';
import path from 'node:path';

/**
 * Path to the SLS JSONL log file.
 * SLS agent will collect and upload logs from this file.
 */
const SLS_LOG_PATH = '/var/log/anolisa/sls/ops/cosh.jsonl';

/**
 * Append a single JSON record as one line to the SLS log file.
 * Uses O_WRONLY | O_APPEND | O_CREAT so the file is created if it does
 * not exist yet. The parent directory is also ensured via mkdirSync.
 * Open→write→close on each call to support logrotate rename-by-path.
 */
export function appendSlsLog(record: Record<string, unknown>): void {
  try {
    const line = JSON.stringify(record) + '\n';
    // Ensure parent directory exists
    const dir = path.dirname(SLS_LOG_PATH);
    fs.mkdirSync(dir, { recursive: true });
    const fd = fs.openSync(
      SLS_LOG_PATH,
      fs.constants.O_WRONLY | fs.constants.O_APPEND | fs.constants.O_CREAT,
      0o644,
    );
    try {
      fs.writeSync(fd, line);
    } finally {
      fs.closeSync(fd);
    }
  } catch {
    // Silently fail - SLS logging should never break the main process
  }
}
