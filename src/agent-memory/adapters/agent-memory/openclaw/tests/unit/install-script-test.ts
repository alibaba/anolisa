/**
 * Unit tests for scripts/install.sh OpenClaw installer-flag gating.
 *
 * The fake `openclaw` below models the two CLI facts install.sh depends on:
 *
 * 1. `plugins install` is a commander subcommand that registers its options
 *    explicitly, so an option the running release does not know is a hard
 *    failure (`error: unknown option '<flag>'`, rc=1) — not a ignored extra.
 * 2. OpenClaw >= 2026.8.1 requires capability consent before committing a
 *    managed install from a source without recorded artifact integrity (this
 *    adapter always installs from a local path), and a non-interactive command
 *    cannot prompt, so it fails with rc=1 unless `--accept-capabilities` is
 *    passed.
 *
 * Together they pin the gating: append the flag iff the installer help
 * advertises it. Reverting install.sh to an unconditional
 * `--accept-capabilities` turns the "installer does not register the option"
 * case red.
 *
 * The version numbers above are context, not the contract: `plugins install`
 * registers `--accept-capabilities` from 2026.8.1 while the OpenClaw CLI
 * reference only documents it from 2026.9.1, which is exactly why install.sh
 * probes the running CLI instead of comparing versions.
 *
 * Pattern follows src/agent-sec-core/openclaw-plugin/tests/unit/deploy-script-test.ts.
 */

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, it } from "node:test";

const INSTALL_SCRIPT = resolve("scripts/install.sh");

/**
 * What the fake installer help advertises:
 * - `consent`: an OpenClaw whose installer registers `--accept-capabilities`
 *   (2026.8.1 and later).
 * - `legacy`: an OpenClaw whose installer does not register the flag and
 *   therefore rejects it as an unknown option (verified on 2026.5.7 and
 *   2026.6.10; 2026.5.22 has no occurrence of it in `dist` either).
 * - `near-miss`: only a longer, different flag is present — proves token-boundary
 *   matching instead of substring matching.
 * - `unavailable`: the help probe itself fails.
 */
type InstallHelpMode = "consent" | "legacy" | "near-miss" | "unavailable";

type InstallOptions = {
  acceptCapabilitiesEnv?: string;
  consentRequired?: boolean;
  installHelpMode?: InstallHelpMode;
  safeInstallEnv?: string;
};

type InstallResult = {
  log: string;
  rootDir: string;
  stderr: string;
  stdout: string;
  status: number | null;
};

const tempDirs: string[] = [];

afterEach(() => {
  for (const dir of tempDirs.splice(0)) {
    rmSync(dir, { recursive: true, force: true });
  }
});

function createExecutable(path: string, content: string): void {
  writeFileSync(path, content, "utf8");
  chmodSync(path, 0o755);
}

function createFakeOpenClaw(binDir: string): void {
  createExecutable(
    join(binDir, "openclaw"),
    `#!/usr/bin/env bash
set -euo pipefail

printf '%s\\n' "$*" >> "\${OPENCLAW_FAKE_LOG:?}"

mode="\${OPENCLAW_FAKE_INSTALL_HELP:-consent}"

advertised=("--force" "--dangerously-force-unsafe-install")
if [[ "$mode" == "consent" ]]; then
    advertised+=("--accept-capabilities")
elif [[ "$mode" == "near-miss" ]]; then
    advertised+=("--accept-capabilities-only")
fi

if [[ "$mode" == "unavailable" && "\${1:-}" == "plugins" && "\${2:-}" == "install" && "\${3:-}" == "--help" ]]; then
    echo "[openclaw] Could not start the CLI." >&2
    exit 1
fi

if [[ "\${1:-}" == "plugins" && "\${2:-}" == "install" && "\${3:-}" == "--help" ]]; then
    echo "Usage: openclaw plugins install <path> [options]"
    echo ""
    echo "Options:"
    for flag in "\${advertised[@]}"; do
        case "$flag" in
            --force)
                echo "  --force  Confirm non-ClawHub sources and overwrite an existing plugin or hook pack"
                ;;
            --dangerously-force-unsafe-install)
                echo "  --dangerously-force-unsafe-install  Deprecated no-op; security.installPolicy may still block"
                ;;
            --accept-capabilities)
                echo "  --accept-capabilities  Accept the plugin's declared capabilities"
                ;;
            --accept-capabilities-only)
                echo "  --accept-capabilities-only  Something else entirely"
                ;;
        esac
    done
    exit 0
fi

if [[ "\${1:-}" == "plugins" && "\${2:-}" == "install" ]]; then
    shift 2
    accepted=0
    for arg in "$@"; do
        if [[ "$arg" != --* ]]; then
            continue
        fi
        known=0
        for flag in "\${advertised[@]}"; do
            if [[ "$arg" == "$flag" ]]; then
                known=1
            fi
        done
        if [[ "$known" != "1" ]]; then
            # commander's default for an unregistered option.
            echo "error: unknown option '$arg'" >&2
            exit 1
        fi
        if [[ "$arg" == "--accept-capabilities" ]]; then
            accepted=1
        fi
    done
    if [[ "\${OPENCLAW_FAKE_CONSENT_REQUIRED:-0}" == "1" && "$accepted" != "1" ]]; then
        echo '[openclaw] Could not start the CLI.' >&2
        echo '[openclaw] Reason: Plugin "memory-anolisa" requires capability consent. Use openclaw plugins install or openclaw plugins enable with --accept-capabilities, then retry.' >&2
        exit 1
    fi
    echo "Installed plugin: memory-anolisa"
    exit 0
fi

if [[ "$*" == "config set plugins.entries.memory-anolisa.hooks.allowConversationAccess true" ]]; then
    echo "configured"
    exit 0
fi

echo "unexpected fake openclaw args: $*" >&2
exit 2
`,
  );
}

/** install.sh aborts unless the built plugin entry exists. */
function createAdapterFixture(rootDir: string): string {
  const adapterDir = join(rootDir, "adapter");
  mkdirSync(join(adapterDir, "openclaw", "dist"), { recursive: true });
  writeFileSync(join(adapterDir, "openclaw", "dist", "index.js"), "export {};\n", "utf8");
  return adapterDir;
}

function runInstall(options: InstallOptions = {}): InstallResult {
  const rootDir = mkdtempSync(join(tmpdir(), "agent-memory-openclaw-install-"));
  tempDirs.push(rootDir);
  const binDir = join(rootDir, "bin");
  const logPath = join(rootDir, "openclaw.log");
  mkdirSync(binDir, { recursive: true });
  createFakeOpenClaw(binDir);

  const adapterDir = createAdapterFixture(rootDir);
  const env: NodeJS.ProcessEnv = {
    ...process.env,
    ANOLISA_ADAPTER_DIR: adapterDir,
    OPENCLAW_FAKE_CONSENT_REQUIRED: options.consentRequired === true ? "1" : "0",
    OPENCLAW_FAKE_INSTALL_HELP: options.installHelpMode ?? "consent",
    OPENCLAW_FAKE_LOG: logPath,
    OPENCLAW_HOME: join(rootDir, "openclaw-home"),
    OPENCLAW_STATE_DIR: join(rootDir, "openclaw-home"),
    PATH: `${binDir}:${process.env.PATH ?? ""}`,
    TMPDIR: rootDir,
  };
  delete env.AGENT_MEMORY_ACCEPT_CAPABILITIES;
  delete env.AGENT_MEMORY_SAFE_INSTALL;
  if (options.acceptCapabilitiesEnv !== undefined) {
    env.AGENT_MEMORY_ACCEPT_CAPABILITIES = options.acceptCapabilitiesEnv;
  }
  if (options.safeInstallEnv !== undefined) {
    env.AGENT_MEMORY_SAFE_INSTALL = options.safeInstallEnv;
  }

  const result = spawnSync("bash", [INSTALL_SCRIPT], {
    cwd: resolve("."),
    encoding: "utf8",
    env,
  });

  return {
    log: existsSync(logPath) ? readFileSync(logPath, "utf8") : "",
    rootDir,
    stderr: result.stderr,
    stdout: result.stdout,
    status: result.status,
  };
}

/** The argv line the fake CLI recorded for the real install (not the help probe). */
function installArgvLine(log: string): string {
  const lines = log
    .split("\n")
    .filter((line) => line.startsWith("plugins install ") && !line.endsWith("--help"));
  assert.equal(lines.length, 1, `expected exactly one install invocation, got:\n${log}`);
  return lines[0];
}

describe("install.sh capability-consent gating", () => {
  it("passes --accept-capabilities when the installer advertises it (OpenClaw >= 2026.8.1)", () => {
    const result = runInstall({ consentRequired: true, installHelpMode: "consent" });

    assert.equal(result.status, 0, result.stderr);
    assert.match(result.log, /plugins install --help/);
    assert.equal(
      installArgvLine(result.log),
      `plugins install ${join(result.rootDir, "adapter", "openclaw")} --force --dangerously-force-unsafe-install --accept-capabilities`,
    );
    assert.match(result.stdout, /Installed plugin: memory-anolisa/);
    assert.match(result.stdout, /plugin installed via openclaw CLI/);
    assert.match(result.log, /config set plugins\.entries\.memory-anolisa\.hooks\.allowConversationAccess true/);
  });

  it("omits --accept-capabilities when the installer does not register it and would reject the unknown option", () => {
    const result = runInstall({ consentRequired: false, installHelpMode: "legacy" });

    assert.equal(result.status, 0, result.stderr);
    assert.match(result.log, /plugins install --help/);
    assert.equal(
      installArgvLine(result.log),
      `plugins install ${join(result.rootDir, "adapter", "openclaw")} --force --dangerously-force-unsafe-install`,
    );
    assert.match(result.stderr, /does not advertise --accept-capabilities \(not listed in its --help\)/);
  });

  it("matches the flag as a whole option token, not as a substring", () => {
    const result = runInstall({ consentRequired: false, installHelpMode: "near-miss" });

    assert.equal(result.status, 0, result.stderr);
    assert.doesNotMatch(installArgvLine(result.log), /--accept-capabilities( |$)/);
    assert.match(result.stderr, /does not advertise --accept-capabilities/);
  });

  it("keeps consent on both install paths when AGENT_MEMORY_SAFE_INSTALL=1", () => {
    const result = runInstall({
      consentRequired: true,
      installHelpMode: "consent",
      safeInstallEnv: "1",
    });

    assert.equal(result.status, 0, result.stderr);
    assert.equal(
      installArgvLine(result.log),
      `plugins install ${join(result.rootDir, "adapter", "openclaw")} --force --accept-capabilities`,
    );
    assert.match(result.stderr, /AGENT_MEMORY_SAFE_INSTALL=1/);
  });

  it("honours AGENT_MEMORY_ACCEPT_CAPABILITIES=0 and says why the install failed", () => {
    const result = runInstall({
      acceptCapabilitiesEnv: "0",
      consentRequired: true,
      installHelpMode: "consent",
    });

    assert.equal(result.status, 1);
    assert.doesNotMatch(installArgvLine(result.log), /--accept-capabilities/);
    assert.match(result.stderr, /AGENT_MEMORY_ACCEPT_CAPABILITIES=0: withholding --accept-capabilities/);
    assert.match(result.stderr, /requires capability consent/);
    assert.match(result.stderr, /was withheld by AGENT_MEMORY_ACCEPT_CAPABILITIES=0/);
    assert.match(
      result.stderr,
      /or consent yourself: openclaw plugins install .* --force --dangerously-force-unsafe-install --accept-capabilities/,
    );
  });

  it("echoes the safe-install argv when the manual consent hint is printed under AGENT_MEMORY_SAFE_INSTALL=1", () => {
    const result = runInstall({
      acceptCapabilitiesEnv: "0",
      consentRequired: true,
      installHelpMode: "consent",
      safeInstallEnv: "1",
    });

    assert.equal(result.status, 1);
    assert.equal(
      installArgvLine(result.log),
      `plugins install ${join(result.rootDir, "adapter", "openclaw")} --force`,
    );
    // The hint must be copy-pasteable: same argv the script just used, so the
    // unsafe-install flag is absent here exactly as it is absent above.
    assert.match(
      result.stderr,
      /or consent yourself: openclaw plugins install .* --force --accept-capabilities/,
    );
    assert.doesNotMatch(result.stderr, /consent yourself: .*--dangerously-force-unsafe-install/);
  });

  it("does not claim to withhold the flag on an installer that never advertised it", () => {
    const result = runInstall({
      acceptCapabilitiesEnv: "0",
      consentRequired: false,
      installHelpMode: "legacy",
    });

    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /AGENT_MEMORY_ACCEPT_CAPABILITIES=0: noted/);
    assert.match(result.stderr, /nothing to withhold/);
    assert.doesNotMatch(result.stderr, /withholding --accept-capabilities/);
    assert.doesNotMatch(installArgvLine(result.log), /--accept-capabilities( |$)/);
  });

  it("survives an unreadable installer help and still installs on a host without the gate", () => {
    const result = runInstall({ consentRequired: false, installHelpMode: "unavailable" });

    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /could not read 'openclaw plugins install --help'/);
    assert.doesNotMatch(installArgvLine(result.log), /--accept-capabilities/);
  });

  it("points at the OpenClaw upgrade, not at a bogus version floor, when a legacy host hits the gate", () => {
    const result = runInstall({ consentRequired: true, installHelpMode: "legacy" });

    assert.equal(result.status, 1);
    assert.match(result.stderr, /does not list --accept-capabilities in 'plugins install --help'/);
    assert.match(result.stderr, /OpenClaw registers it from 2026\.8\.1/);
    assert.match(result.stderr, /upgrade OpenClaw to >= 2026\.8\.1 and re-run/);
  });

  it("blames the failed help probe, not the OpenClaw version, when the probe returned nothing", () => {
    // A 2026.8.x host whose probe fails falls back to the base flags and then
    // hits the consent gate. Telling that operator to upgrade is useless — the
    // CLI already registers the flag — so the hint must point at the probe.
    const result = runInstall({ consentRequired: true, installHelpMode: "unavailable" });

    assert.equal(result.status, 1);
    assert.match(result.stderr, /could not read 'openclaw plugins install --help'/);
    assert.match(result.stderr, /probe returned nothing/);
    assert.match(result.stderr, /may well support the flag \(registered since 2026\.8\.1\)/);
    assert.doesNotMatch(result.stderr, /upgrade OpenClaw to/);
  });
});
