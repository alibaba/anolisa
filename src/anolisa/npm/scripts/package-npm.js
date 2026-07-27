#!/usr/bin/env node

/**
 * @license
 * Copyright 2026 Alibaba Cloud
 * SPDX-License-Identifier: Apache-2.0
 */

/**
 * npm packaging script for @anolisa/cli
 *
 * Builds the Rust binary for the current (or specified) target and packages
 * it into platform-specific npm tarballs ready for `npm publish`.
 *
 * Usage:
 *   node scripts/package-npm.js                     # current platform only
 *   node scripts/package-npm.js --all               # all supported targets
 *   node scripts/package-npm.js --target x86_64     # specific arch
 *
 * Prerequisites:
 *   - Rust toolchain with the target installed (rustup target add ...)
 *   - cargo available on PATH
 *
 * Output:
 *   npm/dist/
 *   ├── anolisa-cli-<version>.tgz              (root package)
 *   ├── anolisa-cli-linux-x64-<version>.tgz    (platform package)
 *   └── anolisa-cli-linux-arm64-<version>.tgz  (platform package)
 */

import { execSync } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  copyFileSync,
  rmSync,
} from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const npmDir = join(__dirname, '..');
const workspaceRoot = join(npmDir, '..');
const distDir = join(npmDir, 'dist');

// Read version from Cargo.toml
const cargoToml = readFileSync(join(workspaceRoot, 'Cargo.toml'), 'utf-8');
const versionMatch = cargoToml.match(/^version\s*=\s*"([^"]+)"/m);
if (!versionMatch) {
  console.error('Error: Could not parse version from Cargo.toml');
  process.exit(1);
}
const version = versionMatch[1];

const TARGETS = [
  {
    rust_target: 'x86_64-unknown-linux-gnu',
    npm_os: 'linux',
    npm_cpu: 'x64',
    pkg_suffix: 'linux-x64',
  },
  {
    rust_target: 'aarch64-unknown-linux-gnu',
    npm_os: 'linux',
    npm_cpu: 'arm64',
    pkg_suffix: 'linux-arm64',
  },
];

async function parseArgs() {
  const args = process.argv.slice(2);
  if (args.includes('--all')) return TARGETS;
  const targetIdx = args.indexOf('--target');
  if (targetIdx !== -1 && args[targetIdx + 1]) {
    const archArg = args[targetIdx + 1];
    const matched = TARGETS.filter(
      (t) => t.rust_target.includes(archArg) || t.npm_cpu === archArg || t.pkg_suffix.includes(archArg),
    );
    if (matched.length === 0) {
      console.error(`Error: Unknown target "${archArg}". Available: ${TARGETS.map((t) => t.pkg_suffix).join(', ')}`);
      process.exit(1);
    }
    return matched;
  }
  // Default: detect current platform
  const os = await import('node:os');
  const currentPlatform = os.platform();
  const currentArch = os.arch();
  const current = TARGETS.find((t) => t.npm_os === currentPlatform && t.npm_cpu === currentArch);
  if (!current) {
    console.error(`Error: No target configuration for ${currentPlatform}-${currentArch}`);
    process.exit(1);
  }
  return [current];
}

function buildTarget(target) {
  console.log(`\n🔨 Building for ${target.rust_target}...`);
  const hostTarget = execSync('rustc -vV', { encoding: 'utf-8' }).match(/host: (.+)/)?.[1]?.trim();
  const crossCompile = !hostTarget || target.rust_target !== hostTarget;

  const buildCmd = crossCompile
    ? `cross build --release --locked -p anolisa-cli --target ${target.rust_target}`
    : `cargo build --release --locked -p anolisa-cli`;

  execSync(buildCmd, { stdio: 'inherit', cwd: workspaceRoot });

  const binaryPath = crossCompile
    ? join(workspaceRoot, 'target', target.rust_target, 'release', 'anolisa')
    : join(workspaceRoot, 'target', 'release', 'anolisa');

  if (!existsSync(binaryPath)) {
    console.error(`Error: Binary not found at ${binaryPath}`);
    process.exit(1);
  }

  return binaryPath;
}

function packagePlatform(target, binaryPath) {
  const pkgName = `@anolisa/cli-${target.pkg_suffix}`;
  const pkgDir = join(distDir, `cli-${target.pkg_suffix}`);

  console.log(`📦 Packaging ${pkgName}@${version}...`);

  // Clean and create package directory
  if (existsSync(pkgDir)) rmSync(pkgDir, { recursive: true });
  mkdirSync(join(pkgDir, 'bin'), { recursive: true });

  // Copy binary
  copyFileSync(binaryPath, join(pkgDir, 'bin', 'anolisa'));
  execSync(`chmod 755 "${join(pkgDir, 'bin', 'anolisa')}"`, { stdio: 'pipe' });

  // Write package.json
  const pkgJson = {
    name: pkgName,
    version,
    description: `ANOLISA CLI native binary for Linux ${target.npm_cpu === 'x64' ? 'x86_64' : 'aarch64'}`,
    license: 'Apache-2.0',
    repository: {
      type: 'git',
      url: 'git+https://github.com/alibaba/anolisa.git',
      directory: 'src/anolisa',
    },
    os: [target.npm_os],
    cpu: [target.npm_cpu],
    bin: { anolisa: 'bin/anolisa' },
    files: ['bin/'],
    preferUnplugged: true,
  };
  writeFileSync(join(pkgDir, 'package.json'), JSON.stringify(pkgJson, null, 2) + '\n');

  // Create tarball
  execSync(`npm pack`, { stdio: 'pipe', cwd: pkgDir });
  console.log(`  ✅ ${pkgName}@${version} packaged`);

  return pkgDir;
}

function packageRoot(targets) {
  const rootPkgDir = join(distDir, 'cli');
  console.log(`\n📦 Packaging @anolisa/cli@${version} (root)...`);

  if (existsSync(rootPkgDir)) rmSync(rootPkgDir, { recursive: true });
  mkdirSync(join(rootPkgDir, 'bin'), { recursive: true });
  mkdirSync(join(rootPkgDir, 'scripts'), { recursive: true });

  // Write a stub bin/anolisa that postinstall will replace with a symlink
  const stubScript = `#!/usr/bin/env node
console.error('@anolisa/cli: postinstall has not run yet. Run "npm rebuild @anolisa/cli" to fix.');
process.exit(1);
`;
  writeFileSync(join(rootPkgDir, 'bin', 'anolisa'), stubScript);
  execSync(`chmod 755 "${join(rootPkgDir, 'bin', 'anolisa')}"`, { stdio: 'pipe' });

  // Copy postinstall script
  copyFileSync(
    join(npmDir, 'scripts', 'postinstall.js'),
    join(rootPkgDir, 'scripts', 'postinstall.js'),
  );

  // Copy README and LICENSE
  for (const file of ['README.md', 'LICENSE']) {
    const src = join(workspaceRoot, file);
    if (existsSync(src)) copyFileSync(src, join(rootPkgDir, file));
  }

  // Build optionalDependencies from target list
  const optionalDeps = {};
  for (const t of targets) {
    optionalDeps[`@anolisa/cli-${t.pkg_suffix}`] = version;
  }

  // Write root package.json
  const rootPkgJson = {
    name: '@anolisa/cli',
    version,
    description: 'ANOLISA CLI — Agentic OS component lifecycle manager',
    license: 'Apache-2.0',
    repository: {
      type: 'git',
      url: 'git+https://github.com/alibaba/anolisa.git',
      directory: 'src/anolisa',
    },
    homepage: 'https://github.com/alibaba/anolisa',
    keywords: ['anolisa', 'agentic-os', 'cli', 'agent', 'ai'],
    bin: { anolisa: 'bin/anolisa' },
    files: ['bin/', 'scripts/', 'README.md', 'LICENSE'],
    scripts: { postinstall: 'node scripts/postinstall.js' },
    engines: { node: '>=16.0.0' },
    os: ['linux'],
    optionalDependencies: optionalDeps,
  };
  writeFileSync(join(rootPkgDir, 'package.json'), JSON.stringify(rootPkgJson, null, 2) + '\n');

  execSync(`npm pack`, { stdio: 'pipe', cwd: rootPkgDir });
  console.log(`  ✅ @anolisa/cli@${version} packaged`);

  return rootPkgDir;
}

async function main() {
  console.log(`\n🚀 ANOLISA CLI npm packaging (v${version})\n`);

  // Clean dist
  if (existsSync(distDir)) rmSync(distDir, { recursive: true });
  mkdirSync(distDir, { recursive: true });

  const targets = await parseArgs();
  console.log(`Targets: ${targets.map((t) => t.pkg_suffix).join(', ')}`);

  // Build and package each platform
  for (const target of targets) {
    const binary = buildTarget(target);
    packagePlatform(target, binary);
  }

  // Package root
  packageRoot(TARGETS);

  console.log(`\n✅ All packages ready in: ${distDir}/`);
  console.log('\nTo publish:');
  console.log('  cd npm/dist/cli && npm publish --access public');
  for (const t of targets) {
    console.log(`  cd npm/dist/cli-${t.pkg_suffix} && npm publish --access public`);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
