import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { piiScan } from "../../src/capabilities/pii-scan.js";
import { _setCliMock, _resetCliMock } from "../../src/utils.js";
import type { CliResult } from "../../src/utils.js";

type RegisteredHook = {
  hookName: string;
  handler: (event: any, ctx: any) => Promise<any>;
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
  const beforePromptBuild = hooks.find((hook) => hook.hookName === "before_prompt_build");
  const messageSending = hooks.find((hook) => hook.hookName === "message_sending");
  assert.ok(beforePromptBuild, "before_prompt_build handler should be registered");
  assert.ok(messageSending, "message_sending handler should be registered");
  return { beforePromptBuild, messageSending, hooks, logs };
}

let lastCliArgs: string[] | undefined;
let lastCliOpts: { timeout?: number } | undefined;

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

describe("pii-scan-user-input", () => {
  beforeEach(() => {
    lastCliArgs = undefined;
    lastCliOpts = undefined;
  });

  afterEach(() => {
    _resetCliMock();
  });

  it("registers before_prompt_build and message_sending", () => {
    const { hooks } = registerHandlers();

    assert.deepEqual(
      hooks.map((hook) => hook.hookName),
      ["before_prompt_build", "message_sending"],
    );
    assert.deepEqual(piiScan.hooks, ["before_prompt_build", "message_sending"]);
  });

  it("does not call CLI for empty prompt", async () => {
    const { beforePromptBuild } = registerHandlers();
    mockCliNoCall();

    const result = await beforePromptBuild.handler({ prompt: "   ", runId: "run-1" }, { runId: "run-1" });

    assert.equal(result, undefined);
  });

  it("passes scan-pii args and timeout", async () => {
    const { beforePromptBuild } = registerHandlers();
    mockCli(scanResult("pass", []));

    await beforePromptBuild.handler({ prompt: "hello", runId: "run-1" }, { runId: "run-1" });

    assert.deepEqual(lastCliArgs, [
      "scan-pii",
      "--text",
      "hello",
      "--format",
      "json",
      "--source",
      "user_input",
    ]);
    assert.equal(lastCliOpts?.timeout, 10000);
  });

  it("adds --include-low-confidence when configured", async () => {
    const { beforePromptBuild } = registerHandlers({ piiIncludeLowConfidence: true });
    mockCli(scanResult("pass", []));

    await beforePromptBuild.handler({ prompt: "hello", runId: "run-1" }, { runId: "run-1" });

    assert.ok(lastCliArgs?.includes("--include-low-confidence"));
  });

  it("pass verdict does not cache a warning", async () => {
    const { beforePromptBuild, messageSending } = registerHandlers();
    mockCli(scanResult("pass", []));

    await beforePromptBuild.handler({ prompt: "hello", runId: "run-1" }, { runId: "run-1" });
    const result = await messageSending.handler({ content: "Hello.", runId: "run-1" }, { runId: "run-1" });

    assert.equal(result, undefined);
  });

  it("warn verdict prefixes same-run reply once and omits raw evidence", async () => {
    const { beforePromptBuild, messageSending } = registerHandlers();
    mockCli(scanResult("warn", [warnFinding]));

    await beforePromptBuild.handler({ prompt: "email alice@example.com", runId: "run-1" }, { runId: "run-1" });
    const first = await messageSending.handler({ content: "Hello.", runId: "run-1" }, { runId: "run-1" });
    const second = await messageSending.handler({ content: "Hello again.", runId: "run-1" }, { runId: "run-1" });

    assert.equal(typeof first?.content, "string");
    assert.match(first.content, /\[pii-checker\]/);
    assert.match(first.content, /email/);
    assert.match(first.content, /a\*\*\*@example\.com/);
    assert.doesNotMatch(first.content, /alice@example\.com/);
    assert.doesNotMatch(first.content, /raw_evidence/);
    assert.ok(first.content.endsWith("\n\nHello."));
    assert.equal(second, undefined);
  });

  it("deny verdict prefixes a high-risk warning", async () => {
    const { beforePromptBuild, messageSending } = registerHandlers();
    mockCli(
      scanResult("deny", [
        {
          type: "credential",
          severity: "deny",
          evidence_redacted: "password=[REDACTED]",
        },
      ]),
    );

    await beforePromptBuild.handler({ prompt: "password=secret", runId: "run-1" }, { runId: "run-1" });
    const result = await messageSending.handler({ content: "Done.", runId: "run-1" }, { runId: "run-1" });

    assert.match(result.content, /高风险/);
    assert.match(result.content, /credential/);
    assert.match(result.content, /Done\./);
  });

  it("uses event.runId when ctx.runId is missing", async () => {
    const { beforePromptBuild, messageSending } = registerHandlers();
    mockCli(scanResult("warn", [warnFinding]));

    await beforePromptBuild.handler({ prompt: "email alice@example.com", runId: "run-event" }, {});
    const result = await messageSending.handler({ content: "Hello.", runId: "run-event" }, {});

    assert.match(result.content, /\[pii-checker\]/);
  });

  it("does not cache warning when runId is missing", async () => {
    const { beforePromptBuild, messageSending, logs } = registerHandlers();
    mockCli(scanResult("warn", [warnFinding]));

    await beforePromptBuild.handler({ prompt: "email alice@example.com" }, { sessionKey: "session-1" });
    const result = await messageSending.handler({ content: "Hello.", runId: "run-1" }, { runId: "run-1" });

    assert.equal(result, undefined);
    assert.ok(logs.some((log) => log.includes("missing runId")));
  });

  it("CLI nonzero fails open", async () => {
    const { beforePromptBuild, messageSending } = registerHandlers();
    mockCli({ exitCode: 1, stdout: "", stderr: "boom" });

    await beforePromptBuild.handler({ prompt: "email alice@example.com", runId: "run-1" }, { runId: "run-1" });
    const result = await messageSending.handler({ content: "Hello.", runId: "run-1" }, { runId: "run-1" });

    assert.equal(result, undefined);
  });

  it("invalid CLI JSON fails open", async () => {
    const { beforePromptBuild, messageSending } = registerHandlers();
    mockCli({ exitCode: 0, stdout: "not-json", stderr: "" });

    await beforePromptBuild.handler({ prompt: "email alice@example.com", runId: "run-1" }, { runId: "run-1" });
    const result = await messageSending.handler({ content: "Hello.", runId: "run-1" }, { runId: "run-1" });

    assert.equal(result, undefined);
  });

  it("expires undrained warnings by TTL", async () => {
    const { beforePromptBuild, messageSending } = registerHandlers({ piiWarningTtlMs: 0 });
    mockCli(scanResult("warn", [warnFinding]));

    await beforePromptBuild.handler({ prompt: "email alice@example.com", runId: "run-1" }, { runId: "run-1" });
    const result = await messageSending.handler({ content: "Hello.", runId: "run-1" }, { runId: "run-1" });

    assert.equal(result, undefined);
  });
});
