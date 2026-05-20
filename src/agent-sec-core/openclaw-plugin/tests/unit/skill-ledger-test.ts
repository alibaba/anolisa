import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { skillLedger } from "../../src/capabilities/skill-ledger.js";
import { _resetCliMock, _setCliMock } from "../../src/utils.js";
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
  skillLedger.register(api);
  const beforeToolCall = hooks.find((hook) => hook.hookName === "before_tool_call");
  const replyDispatch = hooks.find((hook) => hook.hookName === "reply_dispatch");
  assert.ok(beforeToolCall, "before_tool_call handler should be registered");
  return { beforeToolCall, replyDispatch, hooks, logs };
}

function registerWarningHandlers(
  pluginConfig: Record<string, any> = {},
): ReturnType<typeof registerHandlers> & { replyDispatch: RegisteredHook } {
  const handlers = registerHandlers(pluginConfig);
  assert.ok(handlers.replyDispatch, "reply_dispatch handler should be registered");
  return handlers as ReturnType<typeof registerHandlers> & {
    replyDispatch: RegisteredHook;
  };
}

let checkCallCount = 0;
let lastCheckArgs: string[] | undefined;
let lastInitArgs: string[] | undefined;

function agentSecCommandOffset(args: string[]): number {
  return args[0] === "--trace-context" ? 2 : 0;
}

function mockSkillLedgerCheck(result: CliResult): void {
  _setCliMock(async (args) => {
    const offset = agentSecCommandOffset(args);
    if (
      args[offset] === "skill-ledger" &&
      args[offset + 1] === "init" &&
      args[offset + 2] === "--no-baseline"
    ) {
      lastInitArgs = args;
      return {
        exitCode: 0,
        stdout: JSON.stringify({ fingerprint: "test-fingerprint" }),
        stderr: "",
      };
    }

    if (args[offset] === "skill-ledger" && args[offset + 1] === "check") {
      checkCallCount++;
      lastCheckArgs = args;
      return result;
    }

    return { exitCode: 0, stdout: "", stderr: "" };
  });
}

function mockSkillLedgerInitFailure(stderr: string): void {
  _setCliMock(async (args) => {
    const offset = agentSecCommandOffset(args);
    if (
      args[offset] === "skill-ledger" &&
      args[offset + 1] === "init" &&
      args[offset + 2] === "--no-baseline"
    ) {
      lastInitArgs = args;
      return {
        exitCode: 1,
        stdout: "",
        stderr,
      };
    }

    if (args[offset] === "skill-ledger" && args[offset + 1] === "check") {
      return {
        exitCode: 0,
        stdout: JSON.stringify({ status: "pass" }),
        stderr: "",
      };
    }

    return { exitCode: 0, stdout: "", stderr: "" };
  });
}

function mockSkillLedgerStatus(status: string, exitCode = 0): void {
  mockSkillLedgerCheck({
    exitCode,
    stdout: JSON.stringify({ status }),
    stderr: "",
  });
}

function readSkillEvent(path = "/skills/risky/SKILL.md", runId = "run-1") {
  return {
    toolName: "read",
    params: { file_path: path },
    runId,
  };
}

function createReplyDispatchCtx(sendBlockReply?: (payload: any) => boolean) {
  const blockReplies: any[] = [];
  const dispatcher = {
    sendToolResult: () => false,
    sendBlockReply:
      sendBlockReply ??
      ((payload: any) => {
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

describe("skill-ledger", () => {
  beforeEach(() => {
    checkCallCount = 0;
    lastCheckArgs = undefined;
    lastInitArgs = undefined;
  });

  afterEach(() => {
    _resetCliMock();
  });

  it("registers before_tool_call and reply_dispatch", () => {
    mockSkillLedgerStatus("pass");
    const { hooks } = registerWarningHandlers();

    assert.deepEqual(
      hooks.map((hook) => hook.hookName),
      ["before_tool_call", "reply_dispatch"],
    );
    assert.equal(hooks[0].priority, 80);
    assert.equal(hooks[1].priority, 0);
    assert.deepEqual(skillLedger.hooks, ["before_tool_call", "reply_dispatch"]);
  });

  it("logs key init failures without blocking registration", async () => {
    const previousXdgDataHome = process.env.XDG_DATA_HOME;
    process.env.XDG_DATA_HOME = mkdtempSync(resolve(tmpdir(), "skill-ledger-test-"));
    mockSkillLedgerInitFailure("init exploded");

    try {
      const { logs } = registerHandlers();
      await new Promise((resolvePromise) => setTimeout(resolvePromise, 300));
      assert.ok(
        logs.some((log) => log.includes("init --no-baseline failed: init exploded")),
      );
    } finally {
      if (previousXdgDataHome === undefined) {
        delete process.env.XDG_DATA_HOME;
      } else {
        process.env.XDG_DATA_HOME = previousXdgDataHome;
      }
    }
  });

  it("eager key init does not prepend trace context", async () => {
    const previousXdgDataHome = process.env.XDG_DATA_HOME;
    process.env.XDG_DATA_HOME = mkdtempSync(resolve(tmpdir(), "skill-ledger-test-"));
    mockSkillLedgerStatus("pass");

    try {
      const { api } = createMockApi();
      skillLedger.register(api);
      await new Promise((resolvePromise) => setTimeout(resolvePromise, 300));

      assert.equal(lastInitArgs?.[0], "skill-ledger");
      assert.equal(lastInitArgs?.[1], "init");
    } finally {
      if (previousXdgDataHome === undefined) {
        delete process.env.XDG_DATA_HOME;
      } else {
        process.env.XDG_DATA_HOME = previousXdgDataHome;
      }
    }
  });

  it("retries failed key init with hook trace context", async () => {
    const previousXdgDataHome = process.env.XDG_DATA_HOME;
    process.env.XDG_DATA_HOME = mkdtempSync(resolve(tmpdir(), "skill-ledger-test-"));
    let initAttempts = 0;
    _setCliMock(async (args) => {
      const offset = agentSecCommandOffset(args);
      if (
        args[offset] === "skill-ledger" &&
        args[offset + 1] === "init" &&
        args[offset + 2] === "--no-baseline"
      ) {
        initAttempts++;
        lastInitArgs = args;
        return initAttempts === 1
          ? { exitCode: 1, stdout: "", stderr: "eager init failed" }
          : {
              exitCode: 0,
              stdout: JSON.stringify({ fingerprint: "test-fingerprint" }),
              stderr: "",
            };
      }

      if (args[offset] === "skill-ledger" && args[offset + 1] === "check") {
        checkCallCount++;
        lastCheckArgs = args;
        return { exitCode: 0, stdout: JSON.stringify({ status: "pass" }), stderr: "" };
      }

      return { exitCode: 0, stdout: "", stderr: "" };
    });

    try {
      const { beforeToolCall } = registerHandlers();
      await new Promise((resolvePromise) => setTimeout(resolvePromise, 300));
      lastInitArgs = undefined;

      await beforeToolCall.handler(
        {
          toolName: "read",
          params: { file_path: "/skills/retry/SKILL.md" },
          sessionId: "session-1",
          runId: "run-1",
          toolCallId: "tool-1",
          trace: { traceId: "nested-trace-is-not-hook-input" },
        },
        {},
      );

      assert.equal(lastInitArgs?.[0], "--trace-context");
      assert.equal(
        lastInitArgs?.[1],
        JSON.stringify({
          session_id: "session-1",
          run_id: "run-1",
          tool_call_id: "tool-1",
        }),
      );
      assert.equal(lastInitArgs?.[2], "skill-ledger");
    } finally {
      if (previousXdgDataHome === undefined) {
        delete process.env.XDG_DATA_HOME;
      } else {
        process.env.XDG_DATA_HOME = previousXdgDataHome;
      }
    }
  });

  it("matches read SKILL.md calls and preserves file_path priority", async () => {
    mockSkillLedgerStatus("pass");
    const { beforeToolCall } = registerHandlers();

    await beforeToolCall.handler(
      {
        toolName: "read",
        params: {
          file_path: "/skills/alpha/SKILL.md",
          path: "/skills/beta/SKILL.md",
        },
      },
      {},
    );

    assert.equal(checkCallCount, 1);
    assert.ok(lastCheckArgs?.includes("/skills/alpha"));
  });

  it("passes hook trace context to skill-ledger check", async () => {
    mockSkillLedgerStatus("pass");
    const { beforeToolCall } = registerHandlers();

    await beforeToolCall.handler(
      {
        toolName: "read",
        params: { file_path: "/skills/traced/SKILL.md" },
        sessionId: "session-1",
        runId: "run-1",
        toolUseId: "tool-1",
        trace: { traceId: "nested-trace-is-not-hook-input" },
      },
      {},
    );

    assert.equal(lastCheckArgs?.[0], "--trace-context");
    assert.equal(
      lastCheckArgs?.[1],
      JSON.stringify({
        session_id: "session-1",
        run_id: "run-1",
        tool_call_id: "tool-1",
      }),
    );
    assert.equal(lastCheckArgs?.[2], "skill-ledger");
  });

  it("skips non-read tools and non-SKILL.md reads", async () => {
    mockSkillLedgerStatus("pass");
    const { beforeToolCall } = registerHandlers();

    await beforeToolCall.handler(
      { toolName: "exec", params: { command: "cat /skills/a/SKILL.md" } },
      {},
    );
    await beforeToolCall.handler(
      { toolName: "read", params: { file_path: "/skills/a/README.md" } },
      {},
    );

    assert.equal(checkCallCount, 0);
  });

  it("fails open on CLI errors and malformed events", async () => {
    mockSkillLedgerCheck({ exitCode: 1, stdout: "", stderr: "boom" });
    const { beforeToolCall, logs } = registerHandlers();

    assert.equal(await beforeToolCall.handler(readSkillEvent(), {}), undefined);
    assert.equal(await beforeToolCall.handler(null, {}), undefined);
    assert.equal(await beforeToolCall.handler({ toolName: "read" }, {}), undefined);

    assert.ok(logs.some((log) => log.includes("CLI error")));
    assert.ok(logs.some((log) => log.includes("[skill-ledger] error:")));
  });

  it("pass allows silently", async () => {
    mockSkillLedgerStatus("pass");
    const { beforeToolCall, replyDispatch } = registerWarningHandlers();
    const { ctx, blockReplies } = createReplyDispatchCtx();

    assert.equal(await beforeToolCall.handler(readSkillEvent(), { runId: "run-1" }), undefined);
    assert.equal(
      await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx),
      undefined,
    );

    assert.deepEqual(blockReplies, []);
  });

  for (const status of ["none", "drifted", "deny", "tampered"]) {
    it(`${status} defaults to non-blocking same-run user warning`, async () => {
      mockSkillLedgerStatus(status, status === "none" ? 0 : 1);
      const { beforeToolCall, replyDispatch } = registerWarningHandlers();
      const { ctx, blockReplies } = createReplyDispatchCtx();

      const result = await beforeToolCall.handler(
        readSkillEvent(`/skills/${status}/SKILL.md`, "run-1"),
        { runId: "run-1" },
      );
      const firstDispatch = await replyDispatch.handler(
        { runId: "run-1", sendPolicy: "allow" },
        ctx,
      );
      const secondDispatch = await replyDispatch.handler(
        { runId: "run-1", sendPolicy: "allow" },
        ctx,
      );

      assert.equal(result, undefined);
      assert.equal(firstDispatch, undefined);
      assert.equal(secondDispatch, undefined);
      assert.equal(blockReplies.length, 1);
      assert.match(blockReplies[0].text, /\[skill-ledger\]/);
      assert.match(blockReplies[0].text, new RegExp(status));
      assert.match(blockReplies[0].text, /本轮请求将继续处理/);
    });
  }

  it("skillLedgerRequireApproval=true preserves approval behavior", async () => {
    const cases: Array<[string, "warning" | "critical"]> = [
      ["none", "warning"],
      ["drifted", "warning"],
      ["deny", "critical"],
      ["tampered", "critical"],
    ];

    for (const [status, severity] of cases) {
      mockSkillLedgerStatus(status, status === "none" ? 0 : 1);
      const { beforeToolCall, replyDispatch, hooks } = registerHandlers({
        skillLedgerRequireApproval: true,
      });

      const result = await beforeToolCall.handler(
        readSkillEvent(`/skills/${status}/SKILL.md`, "run-1"),
        { runId: "run-1" },
      );

      assert.deepEqual(
        hooks.map((hook) => hook.hookName),
        ["before_tool_call"],
      );
      assert.equal(replyDispatch, undefined);
      assert.equal(result?.requireApproval?.title, "Skill Ledger Security Check");
      assert.equal(result?.requireApproval?.severity, severity);
    }
  });

  for (const status of ["warn", "error", "mystery"]) {
    it(`${status} logs only in default and approval modes`, async () => {
      for (const pluginConfig of [{}, { skillLedgerRequireApproval: true }]) {
        mockSkillLedgerStatus(status, status === "error" ? 1 : 0);
        const { beforeToolCall, replyDispatch, logs } = registerHandlers(pluginConfig);
        const { ctx, blockReplies } = createReplyDispatchCtx();

        const result = await beforeToolCall.handler(
          readSkillEvent(`/skills/${status}/SKILL.md`, "run-1"),
          { runId: "run-1" },
        );

        if (pluginConfig.skillLedgerRequireApproval === true) {
          assert.equal(replyDispatch, undefined);
        } else {
          assert.ok(replyDispatch);
          await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx);
        }

        assert.equal(result, undefined);
        assert.deepEqual(blockReplies, []);
        assert.ok(logs.some((log) => log.includes("[skill-ledger]")));
      }
    });
  }

  it("does not cache a user warning when runId is missing", async () => {
    mockSkillLedgerStatus("none");
    const { beforeToolCall, replyDispatch, logs } = registerWarningHandlers();
    const { ctx, blockReplies } = createReplyDispatchCtx();

    await beforeToolCall.handler(
      { toolName: "read", params: { file_path: "/skills/none/SKILL.md" } },
      {},
    );
    await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx);

    assert.deepEqual(blockReplies, []);
    assert.ok(logs.some((log) => log.includes("missing runId")));
  });

  it("retains warnings when sendBlockReply fails", async () => {
    mockSkillLedgerStatus("drifted", 1);
    const { beforeToolCall, replyDispatch } = registerWarningHandlers();
    const failedCtx = createReplyDispatchCtx(() => false).ctx;
    const { ctx, blockReplies } = createReplyDispatchCtx();

    await beforeToolCall.handler(readSkillEvent("/skills/drifted/SKILL.md", "run-1"), {
      runId: "run-1",
    });
    await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, failedCtx);
    await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx);

    assert.equal(blockReplies.length, 1);
    assert.match(blockReplies[0].text, /drifted/);
  });

  it("drops warnings when delivery is denied or suppressed", async () => {
    mockSkillLedgerStatus("deny", 1);
    const { beforeToolCall, replyDispatch } = registerWarningHandlers();
    const { ctx, blockReplies } = createReplyDispatchCtx();

    await beforeToolCall.handler(readSkillEvent("/skills/deny/SKILL.md", "run-1"), {
      runId: "run-1",
    });
    await replyDispatch.handler({ runId: "run-1", sendPolicy: "deny" }, ctx);
    await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx);

    await beforeToolCall.handler(readSkillEvent("/skills/deny/SKILL.md", "run-2"), {
      runId: "run-2",
    });
    await replyDispatch.handler(
      { runId: "run-2", sendPolicy: "allow", suppressUserDelivery: true },
      ctx,
    );
    await replyDispatch.handler({ runId: "run-2", sendPolicy: "allow" }, ctx);

    assert.deepEqual(blockReplies, []);
  });

  it("expires undrained warnings by TTL", async () => {
    mockSkillLedgerStatus("none");
    const { beforeToolCall, replyDispatch } = registerWarningHandlers({
      skillLedgerWarningTtlMs: 0,
    });
    const { ctx, blockReplies } = createReplyDispatchCtx();

    await beforeToolCall.handler(readSkillEvent("/skills/none/SKILL.md", "run-1"), {
      runId: "run-1",
    });
    await replyDispatch.handler({ runId: "run-1", sendPolicy: "allow" }, ctx);

    assert.deepEqual(blockReplies, []);
  });
});
