#!/usr/bin/env node

/**
 * @license
 * Copyright 2026 Alibaba Cloud
 * SPDX-License-Identifier: Apache-2.0
 */

/**
 * postinstall script for @anolisa/cli
 *
 * Resolves the platform-specific binary package and creates a launcher
 * script at bin/anolisa that delegates to the native binary.
 *
 * Platform packages follow the naming convention:
 *   @anolisa/cli-{os}-{arch}
 *
 * Each platform package ships a single native binary at:
 *   bin/anolisa
 */

import { existsSync, mkdirSync, symlinkSync, unlinkSync, chmodSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { platform, arch } from 'node:os';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const require = createRequire(import.meta.url);
const packageRoot = join(__dirname, '..');
const binDir = join(packageRoot, 'bin');

// Map Node.js platform/arch to package names
const PLATFORM_MAP = {
  'linux-x64': '@anolisa/cli-linux-x64',
  'linux-arm64': '@anolisa/cli-linux-arm64',
};

function resolvePackageBinary() {
  const key = `${platform()}-${arch()}`;
  const pkgName = PLATFORM_MAP[key];

  if (!pkgName) {
    console.warn(
      `@anolisa/cli: No prebuilt binary available for ${platform()}-${arch()}.`,
    );
    console.warn('You can build from source: cd src/anolisa && cargo build --release');
    process.exit(0);
  }

  // Resolve platform package using createRequire (compatible with Node 16+)
  let pkgDir;
  try {
    const resolved = require.resolve(`${pkgName}/package.json`);
    pkgDir = dirname(resolved);
  } catch {
    // Fallback: walk up to find node_modules
    let current = packageRoot;
    while (current !== dirname(current)) {
      const candidate = join(current, 'node_modules', ...pkgName.split('/'));
      if (existsSync(candidate)) {
        pkgDir = candidate;
        break;
      }
      current = dirname(current);
    }
  }

  if (!pkgDir || !existsSync(pkgDir)) {
    console.warn(
      `@anolisa/cli: Platform package ${pkgName} not found.`,
    );
    console.warn(
      'This may happen if optional dependencies were skipped during install.',
    );
    process.exit(0);
  }

  const nativeBinary = join(pkgDir, 'bin', 'anolisa');
  if (!existsSync(nativeBinary)) {
    console.error(
      `@anolisa/cli: Binary not found in ${pkgName} at ${nativeBinary}`,
    );
    process.exit(1);
  }

  return nativeBinary;
}

function main() {
  const nativeBinary = resolvePackageBinary();

  // Ensure bin/ directory exists
  if (!existsSync(binDir)) {
    mkdirSync(binDir, { recursive: true });
  }

  const linkPath = join(binDir, 'anolisa');

  // Remove existing symlink or file
  if (existsSync(linkPath)) {
    unlinkSync(linkPath);
  }

  // Create symlink to the platform-specific binary
  symlinkSync(nativeBinary, linkPath);
  chmodSync(linkPath, 0o755);

  console.log(`@anolisa/cli: Linked native binary for ${platform()}-${arch()}`);
}

main();
