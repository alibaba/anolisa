import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, it } from "node:test";

const INSTALL_SCRIPT = resolve("scripts/install.sh");

// Models the OpenClaw installer contract install.sh depends on: what
// `plugins install --help` advertises, and whether the host enforces the
// 2026.9.2 capability-consent gate. Same fake-CLI approach as
// src/agent-sec-core/openclaw-plugin/tests/unit/deploy-script-test.ts.
type InstallHelpMode =
  | "advertises-accept-capabilities"
  | "no-accept-capabilities"
  | "lookalike-only"
  | "help-unreadable";

type InstallOptions = {
  acceptCapabilitiesEnv?: string;
  installHelpMode?: InstallHelpMode;
  rejectOption?: string;
  requireConsent?: boolean;
  safeInstallEnv?: string;
};

type InstallResult = {
  argv: string[];
  log: string;
  pluginDir: string;
  status: number | null;
  stderr: string;
  stdout: string;
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
set -uo pipefail

printf '%s\\n' "$*" >> "\${OPENCLAW_FAKE_LOG:?}"

if [[ "\${1:-}" == "plugins" && "\${2:-}" == "install" && "\${3:-}" == "--help" ]]; then
    case "\${OPENCLAW_FAKE_INSTALL_HELP:-advertises-accept-capabilities}" in
        no-accept-capabilities)
            echo "Usage: openclaw plugins install <package> [--force] [--dangerously-force-unsafe-install]"
            ;;
        lookalike-only)
            echo "Usage: openclaw plugins install <package> [--force] [--accept-capabilities-file <path>]"
            ;;
        help-unreadable)
            echo "openclaw: unknown command 'plugins install --help'" >&2
            exit 1
            ;;
        *)
            echo "Usage: openclaw plugins install <package> [--force] [--dangerously-force-unsafe-install] [--accept-capabilities]"
            ;;
    esac
    exit 0
fi

if [[ "\${1:-}" == "plugins" && "\${2:-}" == "install" ]]; then
    consented=0
    for arg in "$@"; do
        if [[ -n "\${OPENCLAW_FAKE_REJECT_OPTION:-}" && "$arg" == "\${OPENCLAW_FAKE_REJECT_OPTION}" ]]; then
            echo "error: unknown option '$arg'" >&2
            exit 1
        fi
        # A host whose help does not advertise the flag rejects it as an unknown
        # option — the model both existing repo implementations assume, and the
        # reason passing it unconditionally regresses every pre-2026.9.2 host.
        if [[ "\${OPENCLAW_FAKE_INSTALL_HELP:-advertises-accept-capabilities}" != "advertises-accept-capabilities" && "$arg" == "--accept-capabilities" ]]; then
            echo "error: unknown option '$arg'" >&2
            exit 1
        fi
        if [[ "$arg" == "--accept-capabilities" ]]; then
            consented=1
        fi
    done
    if [[ "\${OPENCLAW_FAKE_REQUIRE_CONSENT:-0}" == "1" && "$consented" != "1" ]]; then
        echo "[openclaw] Could not start the CLI." >&2
        echo '[openclaw] Reason: Plugin "memory-anolisa" requires capability consent. Use openclaw plugins install or openclaw plugins enable with --accept-capabilities, then retry.' >&2
        exit 1
    fi
    echo "Installed plugin: memory-anolisa"
    exit 0
fi

if [[ "$*" == "config set plugins.entries.memory-anolisa.hooks.allowConversationAccess true" ]]; then
    echo "updated plugins.entries.memory-anolisa.hooks.allowConversationAccess"
    exit 0
fi

echo "unexpected fake openclaw args: $*" >&2
exit 2
`,
  );
}

function runInstall(options: InstallOptions = {}): InstallResult {
  const rootDir = mkdtempSync(join(tmpdir(), "agent-memory-openclaw-install-"));
  tempDirs.push(rootDir);
  const binDir = join(rootDir, "bin");
  const logPath = join(rootDir, "openclaw.log");
  const pluginDir = join(rootDir, "adapter", "openclaw");
  mkdirSync(binDir, { recursive: true });
  mkdirSync(join(pluginDir, "dist"), { recursive: true });
  writeFileSync(join(pluginDir, "dist", "index.js"), "export {};\n", "utf8");
  createFakeOpenClaw(binDir);

  const env: NodeJS.ProcessEnv = {
    ...process.env,
    ANOLISA_ADAPTER_DIR: join(rootDir, "adapter"),
    HOME: rootDir,
    OPENCLAW_BIN: "openclaw",
    OPENCLAW_FAKE_INSTALL_HELP: options.installHelpMode ?? "advertises-accept-capabilities",
    OPENCLAW_FAKE_LOG: logPath,
    OPENCLAW_FAKE_REJECT_OPTION: options.rejectOption ?? "",
    OPENCLAW_FAKE_REQUIRE_CONSENT: options.requireConsent === true ? "1" : "0",
    PATH: `${binDir}:${process.env.PATH ?? ""}`,
    TMPDIR: rootDir,
  };
  delete env.OPENCLAW_HOME;
  delete env.OPENCLAW_STATE_DIR;
  if (options.acceptCapabilitiesEnv !== undefined) {
    env.AGENT_MEMORY_ACCEPT_CAPABILITIES = options.acceptCapabilitiesEnv;
  } else {
    delete env.AGENT_MEMORY_ACCEPT_CAPABILITIES;
  }
  if (options.safeInstallEnv !== undefined) {
    env.AGENT_MEMORY_SAFE_INSTALL = options.safeInstallEnv;
  } else {
    delete env.AGENT_MEMORY_SAFE_INSTALL;
  }

  const result = spawnSync("bash", [INSTALL_SCRIPT], { cwd: resolve("."), encoding: "utf8", env });
  const log = existsSync(logPath) ? readFileSync(logPath, "utf8") : "";
  const installLine = log
    .split("\n")
    .find((line) => line.startsWith("plugins install ") && !line.endsWith("--help"));
  assert.ok(installLine !== undefined, `no plugins install invocation logged:\n${log}`);

  return {
    argv: installLine.split(" "),
    log,
    pluginDir,
    status: result.status,
    stderr: result.stderr,
    stdout: result.stdout,
  };
}

describe("openclaw install.sh capability-consent gating", () => {
  it("passes --accept-capabilities when the installed CLI advertises it", () => {
    const result = runInstall({ installHelpMode: "advertises-accept-capabilities", requireConsent: true });

    assert.equal(result.status, 0, result.stderr);
    assert.ok(result.argv.includes("--accept-capabilities"), result.argv.join(" "));
    // The pre-existing unsafe-install bypass must stay untouched.
    assert.ok(result.argv.includes("--dangerously-force-unsafe-install"), result.argv.join(" "));
    assert.match(result.stdout, /Installed plugin: memory-anolisa/);
    // Hook opt-in (#1460) still happens after a successful install.
    assert.match(result.log, /config set plugins\.entries\.memory-anolisa\.hooks\.allowConversationAccess true/);
  });

  it("still installs on hosts whose CLI does not advertise the flag (#3099 regression guard)", () => {
    const result = runInstall({ installHelpMode: "no-accept-capabilities" });

    // Passing the flag unconditionally makes the installer exit 1 on every
    // pre-2026.9.2 host, which manifest.json still declares supported.
    assert.equal(result.status, 0, result.stderr);
    assert.ok(!result.argv.includes("--accept-capabilities"), result.argv.join(" "));
    assert.match(result.stderr, /does not advertise --accept-capabilities/);
    assert.match(result.stdout, /Installed plugin: memory-anolisa/);
  });

  it("does not treat a longer lookalike option as advertised", () => {
    const result = runInstall({ installHelpMode: "lookalike-only" });

    assert.equal(result.status, 0, result.stderr);
    assert.ok(!result.argv.includes("--accept-capabilities"), result.argv.join(" "));
  });

  it("survives an installer whose plugins install --help cannot be read", () => {
    const result = runInstall({ installHelpMode: "help-unreadable" });

    assert.equal(result.status, 0, result.stderr);
    assert.ok(!result.argv.includes("--accept-capabilities"), result.argv.join(" "));
    assert.match(result.stderr, /does not advertise --accept-capabilities/);
  });

  it("AGENT_MEMORY_ACCEPT_CAPABILITIES=0 keeps the consent gate closed", () => {
    const result = runInstall({
      acceptCapabilitiesEnv: "0",
      installHelpMode: "advertises-accept-capabilities",
      requireConsent: true,
    });

    assert.equal(result.status, 1);
    assert.ok(!result.argv.includes("--accept-capabilities"), result.argv.join(" "));
    assert.match(result.stderr, /not consenting to the plugin capabilities/);
    // The failure must be diagnosable as consent, not as a version problem.
    assert.match(result.stderr, /OpenClaw refused the plugin capabilities/);
    assert.match(result.stderr, /--accept-capabilities/);
    assert.doesNotMatch(result.stderr, /rejected one of the options/);
  });

  it("keeps the safe-install path working and consenting", () => {
    const result = runInstall({
      installHelpMode: "advertises-accept-capabilities",
      safeInstallEnv: "1",
    });

    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.argv, [
      "plugins",
      "install",
      result.pluginDir,
      "--force",
      "--accept-capabilities",
    ]);
  });

  it("distinguishes an unknown-option failure from a consent failure", () => {
    const result = runInstall({
      installHelpMode: "advertises-accept-capabilities",
      rejectOption: "--dangerously-force-unsafe-install",
    });

    assert.equal(result.status, 1);
    assert.match(result.stderr, /rejected one of the options/);
    assert.match(result.stderr, /AGENT_MEMORY_SAFE_INSTALL=1/);
    assert.doesNotMatch(result.stderr, /OpenClaw refused the plugin capabilities/);
  });
});
