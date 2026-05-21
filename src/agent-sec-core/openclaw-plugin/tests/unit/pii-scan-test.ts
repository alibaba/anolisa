import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { piiScan } from "../../src/capabilities/pii-scan.js";
import { _setCliMock, _resetCliMock } from "../../src/utils.js";
import type { CliResult } from "../../src/utils.js";

type RegisteredHook = {
  hookName: string;
  handler: (event: any, ctx?: any) => Promise<any>;
  priority: number;
};

function createMockApi(pluginConfig: Record<string, any> = {}) {
  const hooks: RegisteredHook[] = [];
  const logs: string[] = [];
  const api = {
    pluginConfig,
    logger: {
      info: (msg: string) => logs.push(`[INFO] ${msg}`),
      error: (msg: string) => logs.push(`[ERROR] ${msg}`),
      warn: (msg: string) => logs.push(`[WARN] ${msg}`),
      debug: (msg: string) => logs.push(`[DEBUG] ${msg}`),
    },
    on: (hookName: string, handler: any, opts?: { priority?: number }) => {
      hooks.push({ hookName, handler, priority: opts?.priority ?? 0 });
    },
  };
  return { api: api as any, hooks, logs };
}

function registerHandlers(pluginConfig: Record<string, any> = {}) {
  const { api, hooks, logs } = createMockApi(pluginConfig);
  piiScan.register(api);
  const beforeDispatch = hooks.find((hook) => hook.hookName === "before_dispatch");
  assert.ok(beforeDispatch, "before_dispatch handler should be registered");
  return { beforeDispatch, hooks, logs };
}

function enableBlockConfig(enableBlock: boolean): Record<string, any> {
  return {
    capabilities: {
      "pii-scan-user-input": { enableBlock },
    },
  };
}

let lastCliArgs: string[] | undefined;
let lastCliOpts: { timeout?: number; stdin?: string } | undefined;

function mockCli(result: CliResult) {
  _setCliMock(async (args, opts) => {
    lastCliArgs = args;
    lastCliOpts = opts;
    return result;
  });
}

function mockCliNoCall() {
  _setCliMock(async () => {
    throw new Error("CLI should not have been called");
  });
}

function scanResult(verdict: string, findings: unknown[]) {
  return {
    exitCode: 0,
    stdout: JSON.stringify({ verdict, findings }),
    stderr: "",
  };
}

const warnFinding = {
  type: "email",
  severity: "warn",
  evidence_redacted: "a***@example.com",
  raw_evidence: "alice@example.com",
};

const denyFinding = {
  type: "credential",
  severity: "deny",
  evidence_redacted: "password=[REDACTED]",
  raw_evidence: "password=secret",
};

describe("pii-scan-user-input", () => {
  beforeEach(() => {
    lastCliArgs = undefined;
    lastCliOpts = undefined;
  });

  afterEach(() => {
    _resetCliMock();
  });

  it("registers only before_dispatch before prompt-scan priority", () => {
    const { hooks } = registerHandlers();

    assert.deepEqual(
      hooks.map((hook) => hook.hookName),
      ["before_dispatch"],
    );
    assert.deepEqual(piiScan.hooks, ["before_dispatch"]);
    assert.equal(hooks[0].priority, 200);
  });

  it("does not call CLI for empty inbound text", async () => {
    const { beforeDispatch } = registerHandlers();
    mockCliNoCall();

    const result = await beforeDispatch.handler({ content: "   ", body: "   " });

    assert.equal(result, undefined);
  });

  it("passes scan-pii args and timeout", async () => {
    const { beforeDispatch } = registerHandlers();
    mockCli(scanResult("pass", []));

    await beforeDispatch.handler({ content: "hello", body: "fallback" });

    assert.deepEqual(lastCliArgs, [
      "scan-pii",
      "--stdin",
      "--format",
      "json",
      "--source",
      "user_input",
    ]);
    assert.equal(lastCliOpts?.timeout, 10000);
    assert.equal(lastCliOpts?.stdin, "hello");
  });

  it("falls back to body when content is empty", async () => {
    const { beforeDispatch } = registerHandlers();
    mockCli(scanResult("pass", []));

    await beforeDispatch.handler({ content: "   ", body: "hello from body" });

    assert.equal(lastCliOpts?.stdin, "hello from body");
  });

  it("adds --include-low-confidence when configured", async () => {
    const { beforeDispatch } = registerHandlers({ piiIncludeLowConfidence: true });
    mockCli(scanResult("pass", []));

    await beforeDispatch.handler({ content: "hello" });

    assert.ok(lastCliArgs?.includes("--include-low-confidence"));
  });

  it("pass verdict allows silently", async () => {
    const { beforeDispatch } = registerHandlers();
    mockCli(scanResult("pass", []));

    const result = await beforeDispatch.handler({ content: "hello" });

    assert.equal(result, undefined);
  });

  for (const enableBlock of [false, true]) {
    it(`warn verdict logs and allows when enableBlock=${enableBlock}`, async () => {
      const { beforeDispatch, logs } = registerHandlers(enableBlockConfig(enableBlock));
      mockCli(scanResult("warn", [warnFinding]));

      const result = await beforeDispatch.handler({ content: "email alice@example.com" });

      assert.equal(result, undefined);
      assert.ok(logs.some((log) => log.includes("[pii-checker] WARN")));
      assert.ok(logs.some((log) => log.includes("a***@example.com")));
      assert.ok(!logs.some((log) => log.includes("alice@example.com")));
    });
  }

  it("deny verdict defaults to log and allow", async () => {
    const { beforeDispatch, logs } = registerHandlers();
    mockCli(scanResult("deny", [denyFinding]));

    const result = await beforeDispatch.handler({ content: "password=secret" });

    assert.equal(result, undefined);
    assert.ok(logs.some((log) => log.includes("[pii-checker] DENY")));
    assert.ok(logs.some((log) => log.includes("enableBlock=false")));
  });

  it("deny verdict blocks when enableBlock=true and omits raw evidence", async () => {
    const { beforeDispatch } = registerHandlers(enableBlockConfig(true));
    mockCli(scanResult("deny", [denyFinding]));

    const result = await beforeDispatch.handler({ content: "password=secret" });

    assert.equal(result?.handled, true);
    assert.match(result?.text, /\[pii-checker\]/);
    assert.match(result?.text, /高风险/);
    assert.match(result?.text, /credential/);
    assert.match(result?.text, /password=\[REDACTED\]/);
    assert.match(result?.text, /本轮请求已被阻断/);
    assert.doesNotMatch(result?.text, /password=secret/);
    assert.doesNotMatch(result?.text, /raw_evidence/);
  });

  it("CLI nonzero fails open", async () => {
    const { beforeDispatch } = registerHandlers(enableBlockConfig(true));
    mockCli({ exitCode: 1, stdout: "", stderr: "boom" });

    const result = await beforeDispatch.handler({ content: "email alice@example.com" });

    assert.equal(result, undefined);
  });

  it("invalid CLI JSON fails open", async () => {
    const { beforeDispatch } = registerHandlers(enableBlockConfig(true));
    mockCli({ exitCode: 0, stdout: "not-json", stderr: "" });

    const result = await beforeDispatch.handler({ content: "email alice@example.com" });

    assert.equal(result, undefined);
  });
});
