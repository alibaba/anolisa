#!/usr/bin/env node

/**
 * @license
 * Copyright 2026 Alibaba Cloud
 * SPDX-License-Identifier: Apache-2.0
 */

/**
 * npm packaging script for anolisa-tokenless
 *
 * Builds the Rust binaries for the current (or specified) target and packages
 * them into platform-specific npm tarballs ready for `npm publish`.
 *
 * Cross-compilation from Linux to all targets (including macOS) is supported
 * via cargo-zigbuild (recommended) or cross.
 *
 * Usage:
 *   node scripts/package-npm.js                     # current platform only
 *   node scripts/package-npm.js --all               # all supported targets
 *   node scripts/package-npm.js --target x86_64     # specific arch
 *   node scripts/package-npm.js --target darwin-arm64  # specific platform-arch
 *
 * Prerequisites:
 *   - Rust toolchain with the target installed (rustup target add ...)
 *   - cargo available on PATH
 *   - just (for rtk build setup)
 *   - For cross-compilation: cargo-zigbuild (recommended) or cross
 *     Install: cargo install cargo-zigbuild && pip install ziglang
 *
 * Output:
 *   npm/dist/
 *   ├── anolisa-tokenless-<version>.tgz                    (root package)
 *   ├── anolisa-tokenless-linux-x64-<version>.tgz          (platform package)
 *   ├── anolisa-tokenless-linux-arm64-<version>.tgz        (platform package)
 *   ├── anolisa-tokenless-darwin-x64-<version>.tgz         (platform package)
 *   └── anolisa-tokenless-darwin-arm64-<version>.tgz       (platform package)
 */

import { execSync } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  copyFileSync,
  rmSync,
  cpSync,
  readdirSync,
  chmodSync,
} from 'node:fs';
import { join, dirname, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const npmDir = join(__dirname, '..');
const tokenlessRoot = join(npmDir, '..');
const distDir = join(npmDir, 'dist');

// Read version from workspace Cargo.toml
const cargoToml = readFileSync(join(tokenlessRoot, 'Cargo.toml'), 'utf-8');
const versionMatch = cargoToml.match(/\[workspace\.package\][\s\S]*?version\s*=\s*"([^"]+)"/);
const version = versionMatch ? versionMatch[1] : cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!version) {
  console.error('Error: Could not parse version from Cargo.toml');
  process.exit(1);
}

// Read TOON_VER from Makefile to avoid version drift
let toonVer = "0.5.0";
try {
  const makefileContent = readFileSync(join(tokenlessRoot, "Makefile"), "utf-8");
  const toonMatch = makefileContent.match(/^TOON_VER\s*[:?]?=\s*(.+)/m);
  if (toonMatch) toonVer = toonMatch[1].trim();
} catch {
  console.warn("Could not read TOON_VER from Makefile, using default 0.5.0");
}

const BINARIES = ['tokenless', 'rtk', 'toon'];

// npm registry every generated manifest and publish command is pinned to.
// The nested dist/* package roots do NOT inherit npm/.npmrc, so without an
// explicit publishConfig a maintainer's user-level registry would win.
const NPM_REGISTRY = 'https://registry.npmjs.org/';
const PUBLISH_CONFIG = { registry: NPM_REGISTRY, access: 'public' };

// Minimum GLIBC baseline for the *-unknown-linux-gnu targets. Zig links
// against a specific glibc version, so builds routed through cargo-zigbuild
// are portable down to this version regardless of the build host. Linux
// platform packages are glibc-only (declared via "libc": ["glibc"]).
const GLIBC_MIN = '2.17';

const TARGETS = [
  {
    rust_target: 'x86_64-unknown-linux-gnu',
    zig_target: 'x86_64-linux-gnu',
    npm_os: 'linux',
    npm_cpu: 'x64',
    pkg_suffix: 'linux-x64',
  },
  {
    rust_target: 'aarch64-unknown-linux-gnu',
    zig_target: 'aarch64-linux-gnu',
    npm_os: 'linux',
    npm_cpu: 'arm64',
    pkg_suffix: 'linux-arm64',
  },
  {
    rust_target: 'x86_64-apple-darwin',
    zig_target: 'x86_64-macos',
    npm_os: 'darwin',
    npm_cpu: 'x64',
    pkg_suffix: 'darwin-x64',
  },
  {
    rust_target: 'aarch64-apple-darwin',
    zig_target: 'aarch64-macos',
    npm_os: 'darwin',
    npm_cpu: 'arm64',
    pkg_suffix: 'darwin-arm64',
  },
];

async function parseArgs() {
  const args = process.argv.slice(2);
  if (args.includes('--all')) return TARGETS;
  const targetIdx = args.indexOf('--target');
  if (targetIdx !== -1 && args[targetIdx + 1]) {
    const targetArg = args[targetIdx + 1];
    const matched = TARGETS.filter(
      (t) =>
        t.rust_target.includes(targetArg) ||
        t.npm_cpu === targetArg ||
        t.pkg_suffix.includes(targetArg) ||
        t.npm_os === targetArg,
    );
    if (matched.length === 0) {
      console.error(`Unknown target: ${targetArg}`);
      console.error(`Available: ${TARGETS.map((t) => t.pkg_suffix).join(', ')}`);
      process.exit(1);
    }
    return matched;
  }
  // Default: current platform
  const { platform, arch } = await import('node:os');
  const currentKey = `${platform()}-${arch()}`;
  const current = TARGETS.find((t) => t.pkg_suffix === currentKey);
  if (!current) {
    console.error(`Unsupported host platform: ${currentKey}`);
    process.exit(1);
  }
  return [current];
}

/**
 * Detect the best available cross-compilation tool.
 * Priority: cargo-zigbuild > cross > native cargo
 */
function detectBuilder() {
  // Check cargo-zigbuild (recommended — supports all targets from Linux)
  try {
    execSync('cargo-zigbuild --version', { stdio: 'pipe' });
    return 'zigbuild';
  } catch {
    // not installed
  }

  // Check cross (Docker-based — Linux targets only from Linux)
  try {
    execSync('cross --version', { stdio: 'pipe' });
    return 'cross';
  } catch {
    // not installed
  }

  // Fallback: native cargo (no cross-compilation)
  return 'cargo';
}

/** Rust target triple with the GLIBC baseline appended for Linux gnu targets. */
function pinnedRustTarget(target) {
  return target.npm_os === 'linux' ? `${target.rust_target}.${GLIBC_MIN}` : target.rust_target;
}

/** Zig target triple with the GLIBC baseline appended for Linux gnu targets. */
function pinnedZigTarget(target) {
  return target.npm_os === 'linux' ? `${target.zig_target}.${GLIBC_MIN}` : target.zig_target;
}

function buildCommand(builder, target, isCross) {
  if (!isCross) {
    return 'cargo build --release --locked';
  }

  switch (builder) {
    case 'zigbuild':
      return `cargo zigbuild --release --locked --target ${pinnedRustTarget(target)}`;
    case 'cross':
      if (target.npm_os === 'darwin') {
        console.warn(
          `  ⚠️  cross does not support macOS targets. ` +
          `Install cargo-zigbuild for Linux→macOS cross-compilation:`,
        );
        console.warn(`     cargo install cargo-zigbuild && pip install ziglang`);
        return null;
      }
      return `cross build --release --locked --target ${target.rust_target}`;
    case 'cargo':
      console.warn(
        `  ⚠️  No cross-compilation tool available for ${target.rust_target}.`,
      );
      console.warn(`     Install cargo-zigbuild: cargo install cargo-zigbuild && pip install ziglang`);
      return null;
  }
}
/**
 * Set up cross-compilation environment for cargo install fallback.
 * Uses zig cc as the C cross-compiler and zig as the Rust linker.
 */
function setupCrossEnv(target) {
  const env = { ...process.env };
  const zigTarget = pinnedZigTarget(target);

  // SDKROOT for darwin targets is validated in buildTarget() before any
  // cross build starts; it is inherited via process.env here.

  // Resolve how to invoke zig: prefer a real `zig` on PATH, otherwise use
  // cargo-zigbuild's bundled wrappers (covers `pip install ziglang` setups
  // where no standalone `zig` executable exists).
  let zigCC, zigAR, zigRANLIB;
  try {
    execSync('zig version', { stdio: 'pipe' });
    zigCC = `zig cc -target ${zigTarget}`;
    zigAR = 'zig ar';
    zigRANLIB = 'zig ranlib';
  } catch {
    zigCC = `cargo-zigbuild zig cc -- -target ${zigTarget}`;
    zigAR = 'cargo-zigbuild zig ar';
    zigRANLIB = 'cargo-zigbuild zig ranlib';
  }

  // Set CC for cc crate (used by native deps like onig_sys)
  const ccEnvKey = `CC_${target.rust_target.replace(/-/g, '_')}`;
  env[ccEnvKey] = zigCC;
  env.CC = zigCC;
  env.AR = zigAR;
  env.RANLIB = zigRANLIB;
  env.CRATE_CC_NO_DEFAULTS = '1'; // prevent cc crate from adding --target that zig doesn't understand

  // Create temporary CARGO_HOME with linker config using a wrapper script
  const cargoHomeDir = join(tokenlessRoot, 'target', '.cargo-cross');
  const cargoConfigDir = join(cargoHomeDir, 'config');
  const linkerScript = join(cargoHomeDir, 'zig-linker-' + target.pkg_suffix);
  if (!existsSync(cargoConfigDir)) mkdirSync(cargoConfigDir, { recursive: true });

  // Write linker wrapper script (cargo requires a single executable, not args)
  let linkerContent;
  if (target.npm_os === 'darwin') {
    // Use cargo-zigbuild's zig cc wrapper which handles SDKROOT for framework/system lib resolution
    linkerContent = '#!/bin/sh\nexec cargo-zigbuild zig cc -- -target ' + zigTarget + ' "$@"\n';
  } else {
    linkerContent = '#!/bin/sh\nexec ' + zigCC + ' "$@"\n';
  }
  writeFileSync(linkerScript, linkerContent);
  execSync(`chmod +x "${linkerScript}"`, { stdio: 'pipe' });

  let configContent = `[target.${target.rust_target}]
linker = "${linkerScript}"
`;

  // Merge with existing CARGO_HOME config if present
  const origCargoHome = process.env.CARGO_HOME || join(process.env.HOME || '/root', '.cargo');
  const origConfig = join(origCargoHome, 'config.toml');
  if (existsSync(origConfig)) {
    const origContent = readFileSync(origConfig, 'utf-8');
    configContent = configContent + '\n' + origContent;
  }
  writeFileSync(join(cargoConfigDir, 'config.toml'), configContent);
  env.CARGO_HOME = cargoConfigDir;

  return env;
}

function buildTarget(target) {
  console.log(`\n🔨 Building for ${target.rust_target}...`);

  // Ensure the Rust target is installed
  try {
    execSync(`rustup target add ${target.rust_target}`, { stdio: 'pipe' });
  } catch {
    // target may already be installed
  }

  // Detect host target for cross-compile check
  let hostTarget;
  try {
    hostTarget = execSync('rustc -vV', { encoding: 'utf-8' }).match(/host: (.+)/)?.[1]?.trim();
  } catch {
    // ignore
  }

  const builder = detectBuilder();

  // Linux gnu targets are routed through cargo-zigbuild even on a matching
  // host, so the minimum GLIBC baseline is pinned to GLIBC_MIN instead of
  // being inherited from whatever glibc the build machine happens to run.
  const glibcPinned = target.npm_os === 'linux' && builder === 'zigbuild';
  const isCross = !hostTarget || target.rust_target !== hostTarget || glibcPinned;
  if (target.npm_os === 'linux' && !glibcPinned) {
    console.warn(
      `  ⚠️  cargo-zigbuild not available — GLIBC baseline NOT pinned for ${target.rust_target} ` +
      `(binaries will require the build host's glibc). Install cargo-zigbuild before publishing.`,
    );
  }

  // Cross-compiling to macOS requires a macOS SDK (zig only provides the C
  // toolchain, not Apple headers/libraries). Fail fast with instructions
  // instead of hitting an obscure linker error later.
  if (target.npm_os === 'darwin' && isCross) {
    if (!process.env.SDKROOT) {
      const sdkPaths = [
        '/opt/MacOSX13.3.sdk',
        '/opt/MacOSX14.0.sdk',
        '/opt/MacOSX15.0.sdk',
      ];
      for (const p of sdkPaths) {
        if (existsSync(p)) {
          process.env.SDKROOT = p;
          break;
        }
      }
    }
    if (!process.env.SDKROOT || !existsSync(process.env.SDKROOT)) {
      throw new Error(
        `macOS SDK not found for ${target.rust_target}. ` +
        `Set SDKROOT to an extracted macOS SDK (see npm/README.md ` +
        `"Cross-Compilation" for SDK download and setup steps), ` +
        `or build Apple targets natively on a macOS host.`,
      );
    }
    console.log(`  Using macOS SDK: ${process.env.SDKROOT}`);
  }

  if (isCross) {
    console.log(`  Using builder: ${builder} (host: ${hostTarget || 'unknown'})`);
  }

  const cmd = buildCommand(builder, target, isCross);
  if (!cmd) {
    throw new Error(`Cannot cross-compile to ${target.rust_target}`);
  }

  // Build tokenless CLI
  console.log(`  Building tokenless... (${cmd.split(' ').slice(0, 2).join(' ')})`);
  execSync(cmd, { stdio: 'inherit', cwd: tokenlessRoot });

  // Build rtk (third_party/rtk)
  console.log(`  Building rtk...`);
  const rtkDir = join(tokenlessRoot, 'third_party', 'rtk');
  // Ensure rtk source is available (clone + patch via justfile)
  if (!existsSync(join(rtkDir, 'Cargo.toml'))) {
    console.log(`  Setting up rtk source (just setup-rtk)...`);
    try {
      execSync('just setup-rtk', { stdio: 'inherit', cwd: tokenlessRoot });
    } catch {
      console.warn(`  ⚠️  Failed to run 'just setup-rtk'. Ensure 'just' is installed.`);
    }
  }
  if (existsSync(join(rtkDir, 'Cargo.toml'))) {
    const rtkCmd = buildCommand(builder, target, isCross);
    if (rtkCmd) {
      execSync(rtkCmd, { stdio: 'inherit', cwd: rtkDir });
    } else {
      console.warn(`  ⚠️  Cannot cross-compile rtk, skipping.`);
    }
  } else {
    console.warn(`  ⚠️  rtk source not available, skipping rtk binary.`);
  }

  // Build toon (toon-format crate from crates.io)
  // toon-format is a registry dependency, NOT a workspace member, so
  // `cargo build -p toon-format` cannot work here — the `toon` bin is
  // produced via `cargo install`. Each target installs into its own root
  // (target/npm-toon/<pkg_suffix>) which is wiped before the install, so a
  // failed install can never pick up a binary left over from another
  // target or from the host. If the install fails, the whole target fails.
  console.log(`  Building toon...`);
  const toonRoot = join(tokenlessRoot, 'target', 'npm-toon', target.pkg_suffix);
  rmSync(toonRoot, { recursive: true, force: true });
  let toonInstallCmd = `cargo install toon-format --version ${toonVer} --locked --root ${toonRoot}`;
  let toonInstallEnv = process.env;
  if (isCross) {
    toonInstallCmd += ` --target ${target.rust_target}`;
    toonInstallEnv = setupCrossEnv(target);
  }
  execSync(toonInstallCmd, { stdio: 'inherit', env: toonInstallEnv });
  const toonBinPath = join(toonRoot, 'bin', 'toon');
  if (!existsSync(toonBinPath)) {
    throw new Error(`toon binary missing at ${toonBinPath} after cargo install for ${target.rust_target}`);
  }

  // Collect binary paths
  const binaryPaths = {};
  const releaseDir = isCross
    ? join(tokenlessRoot, 'target', target.rust_target, 'release')
    : join(tokenlessRoot, 'target', 'release');
  // rtk is built in third_party/rtk, so its binary is in a separate target dir
  const rtkReleaseDir = isCross
    ? join(tokenlessRoot, 'third_party', 'rtk', 'target', target.rust_target, 'release')
    : join(tokenlessRoot, 'third_party', 'rtk', 'target', 'release');

  for (const bin of BINARIES) {
    let binPath;
    if (bin === 'rtk') {
      // rtk binary lives in third_party/rtk/target/...
      binPath = join(rtkReleaseDir, bin);
      if (!existsSync(binPath)) {
        // Fallback: some setups copy rtk to main target dir
        binPath = join(releaseDir, bin);
      }
    } else if (bin === 'toon') {
      // toon comes exclusively from this target's own install root
      // (verified to exist right after `cargo install` above) — never fall
      // back to native or other targets' artifacts.
      binPath = toonBinPath;
    } else {
      binPath = join(releaseDir, bin);
    }
    if (existsSync(binPath)) {
      binaryPaths[bin] = binPath;
    } else {
      console.warn(`  ⚠️  Binary ${bin} not found at ${binPath}`);
    }
  }

  const builtBins = Object.keys(binaryPaths);
  const missingBins = BINARIES.filter(b => !builtBins.includes(b));
  if (missingBins.length > 0) {
    throw new Error(`Missing required binaries for ${target.rust_target}: ${missingBins.join(', ')}`);
  }

  verifyGlibcBaseline(target, binaryPaths, glibcPinned);

  console.log(`  ✅ Built ${Object.keys(binaryPaths).join(', ')}`);
  return binaryPaths;
}

/**
 * Verify that Linux binaries do not require GLIBC symbols newer than the
 * GLIBC_MIN baseline. When the build was routed through zigbuild with a
 * pinned baseline, exceeding it is a hard error; for unpinned native builds
 * (no zigbuild) the observed requirement is only reported, since it will
 * legitimately reflect the host glibc.
 */
function verifyGlibcBaseline(target, binaryPaths, pinned) {
  if (target.npm_os !== 'linux') return;

  try {
    execSync('readelf --version', { stdio: 'pipe' });
  } catch {
    if (pinned) {
      throw new Error(
        `readelf not found — cannot verify GLIBC baseline for ${target.rust_target}. ` +
        `Install binutils before packaging Linux targets.`,
      );
    }
    console.warn('  ⚠️  readelf not found — skipping GLIBC baseline verification.');
    return;
  }

  const [baseMajor, baseMinor] = GLIBC_MIN.split('.').map(Number);
  for (const [bin, binPath] of Object.entries(binaryPaths)) {
    const out = execSync(`readelf --dyn-syms --wide "${binPath}"`, {
      encoding: 'utf-8',
      maxBuffer: 64 * 1024 * 1024,
    });
    let max = null;
    for (const m of out.matchAll(/GLIBC_(\d+)\.(\d+)/g)) {
      const ver = [Number(m[1]), Number(m[2])];
      if (!max || ver[0] > max[0] || (ver[0] === max[0] && ver[1] > max[1])) max = ver;
    }
    const maxLabel = max ? `GLIBC_${max.join('.')}` : 'none';
    const exceeds = max && (max[0] > baseMajor || (max[0] === baseMajor && max[1] > baseMinor));
    if (exceeds && pinned) {
      throw new Error(
        `${bin} requires ${maxLabel}, which exceeds the pinned baseline GLIBC_${GLIBC_MIN} ` +
        `for ${target.rust_target}`,
      );
    }
    if (exceeds) {
      console.warn(`  ⚠️  ${bin}: max required ${maxLabel} > baseline GLIBC_${GLIBC_MIN} (unpinned native build)`);
    } else {
      console.log(`  GLIBC check: ${bin} max required ${maxLabel} (baseline ${GLIBC_MIN})`);
    }
  }
}

function packagePlatform(target, binaryPaths) {
  const pkgName = `@anolisa/tokenless-${target.pkg_suffix}`;
  const pkgDir = join(distDir, `tokenless-${target.pkg_suffix}`);

  console.log(`\n📦 Packaging ${pkgName}@${version}...`);

  // Clean and create package directory
  if (existsSync(pkgDir)) rmSync(pkgDir, { recursive: true });
  mkdirSync(join(pkgDir, 'bin'), { recursive: true });

  // Copy binaries
  for (const [bin, binPath] of Object.entries(binaryPaths)) {
    copyFileSync(binPath, join(pkgDir, 'bin', bin));
    execSync(`chmod 755 "${join(pkgDir, 'bin', bin)}"`, { stdio: 'pipe' });
  }

  // Build bin map for package.json
  const binMap = {};
  for (const bin of Object.keys(binaryPaths)) {
    binMap[bin] = `bin/${bin}`;
  }

  // Write package.json
  const archLabel = target.npm_cpu === 'x64' ? 'x86_64' : 'aarch64';
  const pkgJson = {
    name: pkgName,
    version,
    description: `Token-Less native binaries for ${target.npm_os} ${archLabel}`,
    license: 'Apache-2.0',
    repository: {
      type: 'git',
      url: 'git+https://github.com/alibaba/anolisa.git',
      directory: 'src/tokenless',
    },
    os: [target.npm_os],
    cpu: [target.npm_cpu],
    // Binaries target *-unknown-linux-gnu — keep musl (Alpine) installs from
    // matching a package whose ELF they cannot run.
    ...(target.npm_os === 'linux' ? { libc: ['glibc'] } : {}),
    bin: binMap,
    files: ['bin/'],
    preferUnplugged: true,
    publishConfig: PUBLISH_CONFIG,
  };
  writeFileSync(join(pkgDir, 'package.json'), JSON.stringify(pkgJson, null, 2) + '\n');

  // Create tarball
  execSync(`npm pack`, { stdio: 'pipe', cwd: pkgDir });
  console.log(`  ✅ ${pkgName}@${version} packaged`);

  return pkgDir;
}

/** Recursively invoke cb(path) for every regular file under dir. */
function walkFiles(dir, cb) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, entry.name);
    if (entry.isDirectory()) walkFiles(p, cb);
    else cb(p);
  }
}

/**
 * Build adapter payloads that are not plain source files. The OpenClaw plugin
 * is TypeScript and must be compiled to dist/index.js before it can be
 * installed by openclaw plugins install; a clean Git checkout only contains
 * index.ts. This mirrors the Makefile's build-openclaw-plugin target.
 */
function buildAdapters() {
  const openclawDir = join(tokenlessRoot, 'adapters', 'tokenless', 'openclaw');

  // Always rebuild: the plugin's `npm run build` cleans dist/ first, so a
  // stale or hand-edited dist/index.js left in the (gitignored) work tree can
  // never leak into a published tarball.
  console.log('  Building OpenClaw plugin (TypeScript -> dist/index.js)...');
  try {
    execSync('make build-openclaw-plugin', {
      stdio: 'inherit',
      cwd: tokenlessRoot,
    });
  } catch (err) {
    throw new Error(
      `OpenClaw plugin build failed. Ensure npm and TypeScript are available. ` +
      `Build manually with: make -C src/tokenless build-openclaw-plugin`,
    );
  }

  if (!existsSync(join(openclawDir, 'dist', 'index.js'))) {
    throw new Error(
      `OpenClaw plugin build did not produce adapters/tokenless/openclaw/dist/index.js`,
    );
  }
}

/**
 * Copy the adapter tree into the root package, mirroring what the Makefile's
 * install-adapter-resources does for FHS installs: strip build-only
 * artifacts, stamp @VERSION@ templates, and mark scripts executable.
 */
function copyAdapters(rootPkgDir) {
  const adaptersSrc = join(tokenlessRoot, 'adapters', 'tokenless');
  const adaptersDest = join(rootPkgDir, 'adapters', 'tokenless');

  console.log('  Bundling framework adapters...');
  cpSync(adaptersSrc, adaptersDest, {
    recursive: true,
    filter: (src) =>
      !src.split(sep).includes('node_modules') &&
      !src.endsWith(`${sep}package-lock.json`) &&
      !src.endsWith(`${sep}.gitignore`),
  });

  // Stamp *.in templates with the release version (Makefile does this via
  // stamp-adapter-templates for RPM installs) and drop the raw templates.
  walkFiles(adaptersDest, (p) => {
    if (p.endsWith('.in')) {
      const stamped = readFileSync(p, 'utf-8').replaceAll('@VERSION@', version);
      writeFileSync(p.slice(0, -3), stamped);
      rmSync(p);
    }
  });

  walkFiles(adaptersDest, (p) => {
    if (p.endsWith('.sh') || p.endsWith('.py')) chmodSync(p, 0o755);
  });
}

function packageRoot(targets) {
  const rootPkgDir = join(distDir, 'tokenless');
  console.log(`\n📦 Packaging anolisa-tokenless@${version} (root)...`);

  if (existsSync(rootPkgDir)) rmSync(rootPkgDir, { recursive: true });
  mkdirSync(join(rootPkgDir, 'bin'), { recursive: true });
  mkdirSync(join(rootPkgDir, 'scripts'), { recursive: true });

  // Write stub bin scripts that postinstall will replace with symlinks
  for (const bin of BINARIES) {
    const stubScript = `#!/usr/bin/env node
console.error('anolisa-tokenless: postinstall has not run yet. Run "npm rebuild anolisa-tokenless" to fix.');
process.exit(1);
`;
    writeFileSync(join(rootPkgDir, 'bin', bin), stubScript);
    execSync(`chmod 755 "${join(rootPkgDir, 'bin', bin)}"`, { stdio: 'pipe' });
  }

  // Copy postinstall script
  copyFileSync(
    join(npmDir, 'scripts', 'postinstall.js'),
    join(rootPkgDir, 'scripts', 'postinstall.js'),
  );

  // Build and bundle framework adapters (hook scripts are plain bash/python
  // — OS and architecture independent), so npm installs get adapter
  // integration on macOS and Linux alike. postinstall copies them to the
  // user-level data dir that run-hook.sh already searches
  // (~/.local/share/anolisa/...).
  buildAdapters();
  copyAdapters(rootPkgDir);

  // Copy README and LICENSE
  const readmeSrc = join(tokenlessRoot, 'README.md');
  if (existsSync(readmeSrc)) copyFileSync(readmeSrc, join(rootPkgDir, 'README.md'));

  const licenseSrc = join(tokenlessRoot, 'LICENSE');
  if (existsSync(licenseSrc)) copyFileSync(licenseSrc, join(rootPkgDir, 'LICENSE'));

  // Build optionalDependencies from target list
  const optionalDeps = {};
  for (const t of targets) {
    optionalDeps[`@anolisa/tokenless-${t.pkg_suffix}`] = version;
  }

  // Determine os and cpu arrays from targets
  const osSet = [...new Set(targets.map((t) => t.npm_os))];
  const cpuSet = [...new Set(targets.map((t) => t.npm_cpu))];

  // Build bin map
  const binMap = {};
  for (const bin of BINARIES) {
    binMap[bin] = `bin/${bin}`;
  }

  // Write root package.json
  const rootPkgJson = {
    name: 'anolisa-tokenless',
    type: 'module',
    version,
    description: 'Token-Less — LLM token optimization toolkit (schema/response compression, command rewriting, tool readiness)',
    license: 'Apache-2.0',
    repository: {
      type: 'git',
      url: 'git+https://github.com/alibaba/anolisa.git',
      directory: 'src/tokenless',
    },
    homepage: 'https://github.com/alibaba/anolisa/tree/main/src/tokenless',
    keywords: ['anolisa', 'tokenless', 'llm', 'token-optimization', 'compression', 'cli'],
    bin: binMap,
    files: ['bin/', 'scripts/', 'adapters/', 'README.md', 'LICENSE'],
    scripts: { postinstall: 'node scripts/postinstall.js' },
    engines: { node: '>=16.0.0' },
    os: osSet,
    cpu: cpuSet,
    optionalDependencies: optionalDeps,
    publishConfig: PUBLISH_CONFIG,
  };
  writeFileSync(join(rootPkgDir, 'package.json'), JSON.stringify(rootPkgJson, null, 2) + '\n');

  execSync(`npm pack`, { stdio: 'pipe', cwd: rootPkgDir });
  console.log(`  ✅ anolisa-tokenless@${version} packaged`);

  return rootPkgDir;
}

async function main() {
  console.log(`\n🚀 Token-Less npm packaging (v${version})\n`);

  // Clean dist
  if (existsSync(distDir)) rmSync(distDir, { recursive: true });
  mkdirSync(distDir, { recursive: true });

  const targets = await parseArgs();
  console.log(`Targets: ${targets.map((t) => t.pkg_suffix).join(', ')}`);

  // Detect builder once and report
  const builder = detectBuilder();
  console.log(`Cross-compilation tool: ${builder}`);
  if (builder === 'cargo' && targets.some((t) => t.npm_os !== process.platform || t.npm_cpu !== process.arch)) {
    console.warn('\n⚠️  No cross-compilation tool detected.');
    console.warn('   For Linux→all targets (including macOS), install cargo-zigbuild:');
    console.warn('     cargo install cargo-zigbuild');
    console.warn('     pip install ziglang   # or: apt install zig / brew install zig');
    console.warn('');
  }

  // Build and package each platform (skip failed targets in --all mode)
  const succeeded = [];
  const failed = [];
  for (const target of targets) {
    try {
      const binaryPaths = buildTarget(target);
      packagePlatform(target, binaryPaths);
      succeeded.push(target.pkg_suffix);
    } catch (err) {
      console.error(`\n  ❌ Failed to build ${target.pkg_suffix}: ${err.message}`);
      failed.push(target.pkg_suffix);
    }
  }
  if (failed.length > 0) {
    console.error(`\n❌ Build failed: ${failed.length} of ${targets.length} target(s) failed: ${failed.join(', ')}`);
    console.error('   Cannot publish incomplete platform packages.');
    process.exit(1);
  }

  // Package root
  packageRoot(TARGETS);

  console.log(`\n✅ All packages ready in: ${distDir}/`);
  console.log('\nTo publish (platform packages first, root last; registry pinned to npmjs):');
  console.log('  make npm-publish');
  console.log('or manually:');
  for (const t of TARGETS) {
    console.log(`  cd npm/dist/tokenless-${t.pkg_suffix} && npm publish --access public --registry=${NPM_REGISTRY}`);
  }
  console.log(`  cd npm/dist/tokenless && npm publish --access public --registry=${NPM_REGISTRY}`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
