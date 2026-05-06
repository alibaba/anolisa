// tests/unit/code-scan.test.ts
import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { codeScan } from "../../src/capabilities/code-scan.js";
import { _setCliMock, _resetCliMock } from "../../src/utils.js";
import type { CliResult } from "../../src/utils.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

type RegisteredHook = {
  hookName: string;
  handler: (event: any, ctx: any) => Promise<any>;
  priority: number;
};

/** Create a minimal mock OpenClaw API and capture hook registrations. */
function createMockApi() {
  const hooks: RegisteredHook[] = [];
  const logs: string[] = [];
  const api = {
    pluginConfig: {},
    logger: {
      info: (msg: string) => logs.push(msg),
      error: (msg: string) => logs.push(msg),
      warn: (msg: string) => logs.push(msg),
      debug: (msg: string) => logs.push(msg),
    },
    on: (hookName: string, handler: any, opts?: { priority?: number }) => {
      hooks.push({ hookName, handler, priority: opts?.priority ?? 0 });
    },
  };
  return { api: api as any, hooks, logs };
}

/** Register scan-code and return the single captured handler. */
function registerAndGetHandler() {
  const { api, hooks, logs } = createMockApi();
  codeScan.register(api);
  assert.equal(hooks.length, 1, "scan-code should register exactly 1 hook");
  return { handler: hooks[0].handler, hooks, logs };
}

/** Standard exec event factory. */
function execEvent(command: string) {
  return { toolName: "exec", params: { command } };
}

/** Captured CLI args from last mock call. */
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("scan-code", () => {
  beforeEach(() => {
    lastCliArgs = undefined;
    lastCliOpts = undefined;
  });

  afterEach(() => {
    _resetCliMock();
  });

  // =========================================================================
  // Dimension 2: Hook Registration Correctness
  // =========================================================================
  describe("hook registration", () => {
    it("registers exactly 1 handler on before_tool_call", () => {
      const { hooks } = registerAndGetHandler();
      assert.equal(hooks[0].hookName, "before_tool_call");
    });

    it("cap.hooks array matches registered hook name", () => {
      const { hooks } = registerAndGetHandler();
      assert.deepEqual(codeScan.hooks, [hooks[0].hookName]);
    });

    it("uses default priority (0)", () => {
      const { hooks } = registerAndGetHandler();
      assert.equal(hooks[0].priority, 0);
    });
  });

  // =========================================================================
  // Dimension 3a: Event-to-CLI Parameter Transformation
  // =========================================================================
  describe("event → CLI args transformation", () => {
    it("exec tool with command → correct CLI args", async () => {
      const { handler } = registerAndGetHandler();
      mockCli({ exitCode: 0, stdout: '{"verdict":"pass","findings":[]}', stderr: "" });

      await handler(execEvent("rm -rf /"), {});

      assert.deepEqual(lastCliArgs, ["scan-code", "--code", "rm -rf /", "--language", "bash"]);
      assert.equal(lastCliOpts?.timeout, 10000);
    });

    it("non-exec tool (read_file) → no CLI call", async () => {
      const { handler } = registerAndGetHandler();
      mockCliNoCall();

      const result = await handler({ toolName: "read_file", params: {} }, {});
      assert.equal(result, undefined);
    });

    it("non-exec tool (search) → no CLI call", async () => {
      const { handler } = registerAndGetHandler();
      mockCliNoCall();

      const result = await handler({ toolName: "search", params: { query: "x" } }, {});
      assert.equal(result, undefined);
    });

    it("exec with empty command → no CLI call", async () => {
      const { handler } = registerAndGetHandler();
      mockCliNoCall();

      const result = await handler(execEvent(""), {});
      assert.equal(result, undefined);
    });

    it("exec with non-string command → no CLI call", async () => {
      const { handler } = registerAndGetHandler();
      mockCliNoCall();

      const result = await handler({ toolName: "exec", params: { command: 123 } }, {});
      assert.equal(result, undefined);
    });

    it("exec with missing command param → no CLI call", async () => {
      const { handler } = registerAndGetHandler();
      mockCliNoCall();

      const result = await handler({ toolName: "exec", params: {} }, {});
      assert.equal(result, undefined);
    });

    it("command with special chars → passed verbatim to CLI", async () => {
      const { handler } = registerAndGetHandler();
      mockCli({ exitCode: 0, stdout: '{"verdict":"pass","findings":[]}', stderr: "" });

      await handler(execEvent('echo "hello world"'), {});

      assert.deepEqual(lastCliArgs, ["scan-code", "--code", 'echo "hello world"', "--language", "bash"]);
    });
  });

  // =========================================================================
  // Dimension 3b: CLI Output-to-Hook Action Mapping
  // =========================================================================
  describe("CLI output → hook action mapping", () => {
    it("pass verdict → undefined (allow)", async () => {
      const { handler } = registerAndGetHandler();
      mockCli({ exitCode: 0, stdout: '{"verdict":"pass","findings":[]}', stderr: "" });

      const result = await handler(execEvent("ls"), {});
      assert.equal(result, undefined);
    });

    it("deny with 1 finding → { requireApproval } (unified ask strategy)", async () => {
      const { handler } = registerAndGetHandler();
      mockCli({
        exitCode: 0,
        stdout: '{"verdict":"deny","findings":[{"desc_zh":"危险命令"}]}',
        stderr: "",
      });

      const result = await handler(execEvent("rm -rf /"), {});

      assert.ok(result.requireApproval);
      assert.equal(result.requireApproval.title, "Code Scanner Security Warning");
      assert.equal(result.requireApproval.severity, "warning");
      assert.ok(result.requireApproval.description.includes("[code-scanner] Detected 1 issue(s):"));
      assert.ok(result.requireApproval.description.includes("- 危险命令"));
      assert.ok(result.requireApproval.description.includes("Command: rm -rf /"));
    });

    it("deny with 2 findings → requireApproval.description contains both", async () => {
      const { handler } = registerAndGetHandler();
      mockCli({
        exitCode: 0,
        stdout: '{"verdict":"deny","findings":[{"desc_zh":"A"},{"desc_zh":"B"}]}',
        stderr: "",
      });

      const result = await handler(execEvent("bad-cmd"), {});

      assert.ok(result.requireApproval);
      assert.ok(result.requireApproval.description.includes("Detected 2 issue(s):"));
      assert.ok(result.requireApproval.description.includes("- A"));
      assert.ok(result.requireApproval.description.includes("- B"));
    });

    it("warn with findings → { requireApproval }", async () => {
      const { handler } = registerAndGetHandler();
      mockCli({
        exitCode: 0,
        stdout: '{"verdict":"warn","findings":[{"desc_zh":"注意"}]}',
        stderr: "",
      });

      const result = await handler(execEvent("risky-cmd"), {});

      assert.ok(result.requireApproval);
      assert.equal(result.requireApproval.title, "Code Scanner Security Warning");
      assert.equal(result.requireApproval.severity, "warning");
      assert.ok(result.requireApproval.description.includes("- 注意"));
    });

    it("deny but empty findings → undefined (findings.length === 0 gate)", async () => {
      const { handler } = registerAndGetHandler();
      mockCli({
        exitCode: 0,
        stdout: '{"verdict":"deny","findings":[]}',
        stderr: "",
      });

      const result = await handler(execEvent("cmd"), {});
      assert.equal(result, undefined);
    });

    it("unknown verdict with findings → undefined (falls through)", async () => {
      const { handler } = registerAndGetHandler();
      mockCli({
        exitCode: 0,
        stdout: '{"verdict":"info","findings":[{"desc_zh":"x"}]}',
        stderr: "",
      });

      const result = await handler(execEvent("cmd"), {});
      assert.equal(result, undefined);
    });

    it("missing verdict field → undefined (no branch matches)", async () => {
      const { handler } = registerAndGetHandler();
      mockCli({
        exitCode: 0,
        stdout: '{"findings":[{"desc_zh":"x"}]}',
        stderr: "",
      });

      const result = await handler(execEvent("cmd"), {});
      assert.equal(result, undefined);
    });

    it("missing findings field → undefined (defaults to [], length === 0)", async () => {
      const { handler } = registerAndGetHandler();
      mockCli({
        exitCode: 0,
        stdout: '{"verdict":"deny"}',
        stderr: "",
      });

      const result = await handler(execEvent("cmd"), {});
      assert.equal(result, undefined);
    });
  });

  // =========================================================================
  // Dimension 4: Fail-Open Guarantee
  // =========================================================================
  describe("fail-open guarantee", () => {
    it("CLI failure (exitCode 1) → undefined, never throws", async () => {
      const { handler } = registerAndGetHandler();
      mockCli({ exitCode: 1, stdout: "", stderr: "command not found" });

      const result = await handler(execEvent("ls"), {});
      assert.equal(result, undefined);
    });

    it("CLI timeout (exitCode 124) → undefined, never throws", async () => {
      const { handler } = registerAndGetHandler();
      mockCli({ exitCode: 124, stdout: "", stderr: "timed out" });

      const result = await handler(execEvent("ls"), {});
      assert.equal(result, undefined);
    });

    it("malformed JSON → undefined (JSON.parse caught)", async () => {
      const { handler } = registerAndGetHandler();
      mockCli({ exitCode: 0, stdout: "not json at all", stderr: "" });

      const result = await handler(execEvent("ls"), {});
      assert.equal(result, undefined);
    });

    it("CLI mock throws → undefined (catch block)", async () => {
      const { handler } = registerAndGetHandler();
      _setCliMock(async () => { throw new Error("process crashed"); });

      const result = await handler(execEvent("ls"), {});
      assert.equal(result, undefined);
    });
  });
});
