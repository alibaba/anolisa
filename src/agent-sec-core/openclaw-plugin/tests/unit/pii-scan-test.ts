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
  const replyDispatch = hooks.find((hook) => hook.hookName === "reply_dispatch");
  assert.ok(beforePromptBuild, "before_prompt_build handler should be registered");
  assert.ok(replyDispatch, "reply_dispatch handler should be registered");
  return { beforePromptBuild, replyDispatch, hooks, logs };
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

function createReplyDispatchCtx(sendBlockReply?: (payload: any) => boolean) {
  const blockReplies: any[] = [];
  const dispatcher = {
    sendToolResult: () => false,
    sendBlockReply: sendBlockReply ?? ((payload: any) => {
      blockReplies.push(payload);
      return true;
    }),
    sendFinalReply: () => false,
    waitForIdle: async () => {},
    getQueuedCounts: () => ({ tool: 0, block: blockReplies.length, final: 0 }),
    getFailedCounts: () => ({ tool: 0, block: 0, final: 0 }),
    markComplete: () => {},
  };
  return { ctx: { dispatcher }, blockReplies };
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

  it("registers before_prompt_build and reply_dispatch", () => {
    const { hooks } = registerHandlers();

    assert.deepEqual(
      hooks.map((hook) => hook.hookName),
      ["before_prompt_build", "reply_dispatch"],
    );
    assert.deepEqual(piiScan.hooks, ["before_prompt_build", "reply_dispatch"]);
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
    const { beforePromptBuild, replyDispatch } = registerHandlers();
    const { ctx, blockReplies } = createReplyDispatchCtx();
    mockCli(scanResult("pass", []));

    await beforePromptBuild.handler({ prompt: "hello", runId: "run-1" }, { runId: "run-1" });
    const result = await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx);

    assert.equal(result, undefined);
    assert.deepEqual(blockReplies, []);
  });

  it("warn verdict queues a same-run block reply once and omits raw evidence", async () => {
    const { beforePromptBuild, replyDispatch } = registerHandlers();
    const { ctx, blockReplies } = createReplyDispatchCtx();
    mockCli(scanResult("warn", [warnFinding]));

    await beforePromptBuild.handler({ prompt: "email alice@example.com", runId: "run-1" }, { runId: "run-1" });
    const first = await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx);
    const second = await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx);

    assert.equal(first, undefined);
    assert.equal(second, undefined);
    assert.equal(blockReplies.length, 1);
    assert.match(blockReplies[0].text, /\[pii-checker\]/);
    assert.match(blockReplies[0].text, /email/);
    assert.match(blockReplies[0].text, /a\*\*\*@example\.com/);
    assert.doesNotMatch(blockReplies[0].text, /alice@example\.com/);
    assert.doesNotMatch(blockReplies[0].text, /raw_evidence/);
    assert.match(blockReplies[0].text, /本轮请求将继续处理/);
  });

  it("keeps warning cached when reply_dispatch cannot queue the block reply", async () => {
    const { beforePromptBuild, replyDispatch } = registerHandlers();
    const failedCtx = createReplyDispatchCtx(() => false).ctx;
    const { ctx, blockReplies } = createReplyDispatchCtx();
    mockCli(scanResult("warn", [warnFinding]));

    await beforePromptBuild.handler({ prompt: "email alice@example.com", runId: "run-1" }, { runId: "run-1" });
    await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, failedCtx);
    await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx);

    assert.equal(blockReplies.length, 1);
    assert.match(blockReplies[0].text, /\[pii-checker\]/);
  });

  it("deny verdict queues a high-risk warning", async () => {
    const { beforePromptBuild, replyDispatch } = registerHandlers();
    const { ctx, blockReplies } = createReplyDispatchCtx();
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
    const result = await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx);

    assert.equal(result, undefined);
    assert.equal(blockReplies.length, 1);
    assert.match(blockReplies[0].text, /高风险/);
    assert.match(blockReplies[0].text, /credential/);
  });

  it("uses event.runId when ctx.runId is missing", async () => {
    const { beforePromptBuild, replyDispatch } = registerHandlers();
    const { ctx, blockReplies } = createReplyDispatchCtx();
    mockCli(scanResult("warn", [warnFinding]));

    await beforePromptBuild.handler({ prompt: "email alice@example.com", runId: "run-event" }, {});
    const result = await replyDispatch.handler({ runId: "run-event", sendPolicy: "allow" }, ctx);

    assert.equal(result, undefined);
    assert.equal(blockReplies.length, 1);
    assert.match(blockReplies[0].text, /\[pii-checker\]/);
  });

  it("does not cache warning when runId is missing", async () => {
    const { beforePromptBuild, replyDispatch, logs } = registerHandlers();
    const { ctx, blockReplies } = createReplyDispatchCtx();
    mockCli(scanResult("warn", [warnFinding]));

    await beforePromptBuild.handler({ prompt: "email alice@example.com" }, { sessionKey: "session-1" });
    const result = await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx);

    assert.equal(result, undefined);
    assert.deepEqual(blockReplies, []);
    assert.ok(logs.some((log) => log.includes("missing runId")));
  });

  it("CLI nonzero fails open", async () => {
    const { beforePromptBuild, replyDispatch } = registerHandlers();
    const { ctx, blockReplies } = createReplyDispatchCtx();
    mockCli({ exitCode: 1, stdout: "", stderr: "boom" });

    await beforePromptBuild.handler({ prompt: "email alice@example.com", runId: "run-1" }, { runId: "run-1" });
    const result = await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx);

    assert.equal(result, undefined);
    assert.deepEqual(blockReplies, []);
  });

  it("invalid CLI JSON fails open", async () => {
    const { beforePromptBuild, replyDispatch } = registerHandlers();
    const { ctx, blockReplies } = createReplyDispatchCtx();
    mockCli({ exitCode: 0, stdout: "not-json", stderr: "" });

    await beforePromptBuild.handler({ prompt: "email alice@example.com", runId: "run-1" }, { runId: "run-1" });
    const result = await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx);

    assert.equal(result, undefined);
    assert.deepEqual(blockReplies, []);
  });

  it("expires undrained warnings by TTL", async () => {
    const { beforePromptBuild, replyDispatch } = registerHandlers({ piiWarningTtlMs: 0 });
    const { ctx, blockReplies } = createReplyDispatchCtx();
    mockCli(scanResult("warn", [warnFinding]));

    await beforePromptBuild.handler({ prompt: "email alice@example.com", runId: "run-1" }, { runId: "run-1" });
    const result = await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx);

    assert.equal(result, undefined);
    assert.deepEqual(blockReplies, []);
  });

  it("drops warnings without display when user delivery is suppressed or denied", async () => {
    const { beforePromptBuild, replyDispatch } = registerHandlers();
    const { ctx, blockReplies } = createReplyDispatchCtx();
    mockCli(scanResult("warn", [warnFinding]));

    await beforePromptBuild.handler({ prompt: "email alice@example.com", runId: "run-1" }, { runId: "run-1" });
    await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow", suppressUserDelivery: true }, ctx);
    await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx);

    await beforePromptBuild.handler({ prompt: "email alice@example.com", runId: "run-2" }, { runId: "run-2" });
    await replyDispatch.handler({ runId: "run-2", sendPolicy: "deny" }, ctx);
    await replyDispatch.handler({ runId: "run-2", sendPolicy: "allow" }, ctx);

    assert.deepEqual(blockReplies, []);
  });
});
