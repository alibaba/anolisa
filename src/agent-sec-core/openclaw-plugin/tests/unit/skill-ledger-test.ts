// tests/skill-ledger-test.ts
// Deep test for skill-ledger hook: event filtering, path resolution, fail-open, resilience.
//
// Run:  npx tsx tests/unit/skill-ledger-test.ts
//       npm test

import { skillLedger } from "../../src/capabilities/skill-ledger.js";
import { _resetCliMock, _setCliMock } from "../../src/utils.js";
import type { CliResult } from "../../src/utils.js";

// ── Minimal test framework ──────────────────────────────────────────────────

let passed = 0;
let failed = 0;

function assert(condition: boolean, message: string): void {
  if (condition) {
    passed++;
    console.log(`  ✅ ${message}`);
  } else {
    failed++;
    console.log(`  ❌ FAIL: ${message}`);
  }
}

// ── Mock API factory ────────────────────────────────────────────────────────

type RegisteredHook = {
  hookName: string;
  handler: (event: any, ctx: any) => Promise<any>;
  priority: number;
};

function createMockApi() {
  const hooks: RegisteredHook[] = [];
  const logs: string[] = [];

  const api = {
    pluginConfig: {},
    logger: {
      info: (msg: string) => logs.push(`[INFO] ${msg}`),
      error: (msg: string) => logs.push(`[ERROR] ${msg}`),
      warn: (msg: string) => logs.push(`[WARN] ${msg}`),
    },
    on: (hookName: string, handler: any, opts?: { priority?: number }) => {
      hooks.push({ hookName, handler, priority: opts?.priority ?? 0 });
    },
  };

  return { api: api as any, hooks, logs };
}

// ── CLI mock helpers ───────────────────────────────────────────────────────

let checkCallCount = 0;
let lastCheckArgs: string[] | undefined;

function mockSkillLedgerCheck(result: CliResult): void {
  _setCliMock(async (args) => {
    if (args[0] === "skill-ledger" && args[1] === "init" && args[2] === "--no-baseline") {
      return {
        exitCode: 0,
        stdout: JSON.stringify({ fingerprint: "test-fingerprint" }),
        stderr: "",
      };
    }

    if (args[0] === "skill-ledger" && args[1] === "check") {
      checkCallCount++;
      lastCheckArgs = args;
      return result;
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

process.on("exit", () => _resetCliMock());

// ── Setup: register capability, extract handler ─────────────────────────────

mockSkillLedgerStatus("pass");

const { api, hooks, logs } = createMockApi();
skillLedger.register(api);

// Wait for eager ensureKeys() fire-and-forget to settle
await new Promise((r) => setTimeout(r, 300));

const hook = hooks.find((h) => h.hookName === "before_tool_call")!;

/** Clear captured logs between test cases. */
function clearLogs(): void {
  logs.length = 0;
}

/** Fire the handler with a given event and return { result, logs snapshot }. */
async function fire(event: any, ctx: any = {}) {
  clearLogs();
  checkCallCount = 0;
  lastCheckArgs = undefined;
  const result = await hook.handler(event, ctx);
  return { result, logs: [...logs] };
}

// ═════════════════════════════════════════════════════════════════════════════
console.log("=== skill-ledger Deep Test ===\n");

// ── 1. Hook registration metadata ──────────────────────────────────────────
console.log("[1] Hook registration");

assert(hooks.length === 1, "registers exactly one hook");
assert(hooks[0].hookName === "before_tool_call", "hook name is before_tool_call");
assert(hooks[0].priority === 80, "priority is 80");

// ── 2. Positive filtering — events that SHOULD match ────────────────────────
console.log("\n[2] Positive filtering (should match → CLI invoked)");

{
  const { result } = await fire({
    toolName: "read",
    params: { file_path: "/home/user/.openclaw/skills/github/SKILL.md" },
  });
  assert(result === undefined, "absolute path → returns undefined (allow)");
  assert(checkCallCount === 1, "absolute path → CLI check invoked");
}

{
  const { result } = await fire({
    toolName: "read",
    params: { path: "/opt/skills/my-tool/SKILL.md" },
  });
  assert(result === undefined, "'path' param (alt name) → returns undefined");
  assert(checkCallCount === 1, "'path' param → CLI check invoked");
}

{
  await fire({
    toolName: "read",
    params: { file_path: "SKILL.md" },
  });
  assert(checkCallCount === 1, "bare 'SKILL.md' → CLI check invoked");
}

{
  await fire({
    toolName: "read",
    params: { file_path: "  /skills/github/SKILL.md  " },
  });
  assert(checkCallCount === 1, "whitespace-padded path → CLI check invoked");
}

{
  await fire({
    toolName: "read",
    params: { file_path: "/deeply/nested/dir/structure/skill-name/SKILL.md" },
  });
  assert(checkCallCount === 1, "deeply nested path → CLI check invoked");
}

// ── 3. Negative filtering — events that MUST be skipped ─────────────────────
console.log("\n[3] Negative filtering (should skip → no logs)");

{
  const { result, logs } = await fire({
    toolName: "exec",
    params: { command: "cat /skills/github/SKILL.md" },
  });
  assert(result === undefined, "exec tool → returns undefined");
  assert(logs.length === 0, "exec tool → no logs (skipped)");
}

{
  const { result, logs } = await fire({
    toolName: "shell",
    params: { command: "ls" },
  });
  assert(result === undefined, "shell tool → returns undefined");
  assert(logs.length === 0, "shell tool → no logs (skipped)");
}

{
  const { result, logs } = await fire({
    toolName: "write_file",
    params: { file_path: "/skills/github/SKILL.md", content: "..." },
  });
  assert(result === undefined, "write_file + SKILL.md → returns undefined (not a read tool)");
  assert(logs.length === 0, "write_file + SKILL.md → no logs (skipped)");
}

{
  const { result, logs } = await fire({
    toolName: "read",
    params: { file_path: "/home/user/project/README.md" },
  });
  assert(result === undefined, "read + README.md → returns undefined");
  assert(logs.length === 0, "read + README.md → no logs (skipped)");
}

{
  const { result, logs } = await fire({
    toolName: "read",
    params: { file_path: "/skills/SKILL.md.bak" },
  });
  assert(result === undefined, "SKILL.md.bak → returns undefined");
  assert(logs.length === 0, "SKILL.md.bak → no logs (skipped)");
}

{
  const { result, logs } = await fire({
    toolName: "read",
    params: { file_path: "/skills/SKILL.markdown" },
  });
  assert(result === undefined, "SKILL.markdown → returns undefined");
  assert(logs.length === 0, "SKILL.markdown → no logs (skipped)");
}

{
  const { result, logs } = await fire({
    toolName: "read",
    params: {},
  });
  assert(result === undefined, "read + no path param → returns undefined");
  assert(logs.length === 0, "read + no path param → no logs (skipped)");
}

{
  const { result, logs } = await fire({
    toolName: "read",
    params: { file_path: "" },
  });
  assert(result === undefined, "read + empty path → returns undefined");
  assert(logs.length === 0, "read + empty path → no logs (skipped)");
}

{
  const { result, logs } = await fire({
    toolName: "read",
    params: { file_path: "   " },
  });
  assert(result === undefined, "whitespace-only path → returns undefined");
  assert(logs.length === 0, "whitespace-only path → no logs (skipped)");
}

{
  const { result, logs } = await fire({
    toolName: "read",
    params: { file_path: 42 },
  });
  assert(result === undefined, "non-string file_path (number) → returns undefined");
  assert(logs.length === 0, "non-string file_path → no logs (skipped)");
}

// ── 4. Fail-open guarantee ──────────────────────────────────────────────────
console.log("\n[4] Fail-open (CLI unavailable → warn + allow)");

{
  mockSkillLedgerCheck({ exitCode: 1, stdout: "", stderr: "boom" });
  const { result, logs } = await fire({
    toolName: "read",
    params: { file_path: "/skills/test/SKILL.md" },
  });
  assert(result === undefined, "CLI failure → returns undefined (never blocks)");
  assert(
    logs.some((l) => l.includes("[WARN]") && l.includes("CLI error")),
    "CLI failure → emits WARN with 'CLI error'",
  );
}

// ── 5. Malformed event resilience (outer try-catch) ─────────────────────────
console.log("\n[5] Malformed event resilience");

{
  // Completely empty object — toolName is undefined → extractSkillPath returns early
  const { result, logs } = await fire({});
  assert(result === undefined, "empty object {} → returns undefined");
  // extractSkillPath: READ_TOOL_NAMES.includes(undefined) → false → returns undefined → no CLI
  assert(logs.length === 0, "empty object {} → no logs (skipped by filter)");
}

{
  // null event → event.toolName throws → caught by outer try-catch
  const { result, logs } = await fire(null);
  assert(result === undefined, "null event → returns undefined (fail-open catch)");
  assert(logs.some((l) => l.includes("[WARN]")), "null event → emits WARN from catch block");
}

{
  // read but params is missing → event.params[x] throws → caught by outer try-catch
  const { result, logs } = await fire({ toolName: "read" });
  assert(result === undefined, "missing params property → returns undefined (fail-open catch)");
  assert(logs.some((l) => l.includes("[WARN]")), "missing params → emits WARN from catch block");
}

{
  // params is null → event.params[x] throws → caught
  const { result, logs } = await fire({ toolName: "read", params: null });
  assert(result === undefined, "params: null → returns undefined (fail-open catch)");
  assert(logs.some((l) => l.includes("[WARN]")), "params: null → emits WARN from catch block");
}

// ── 6. Path param priority ──────────────────────────────────────────────────
console.log("\n[6] Path param priority (file_path before path)");

{
  mockSkillLedgerStatus("pass");
  // When both file_path and path are present, file_path should win
  await fire({
    toolName: "read",
    params: {
      file_path: "/skills/alpha/SKILL.md",
      path: "/skills/beta/SKILL.md",
    },
  });
  // Handler proceeds (we can't see which path was chosen from logs alone in CLI-error mode,
  // but the fact it proceeds confirms at least one matched)
  assert(checkCallCount === 1, "both params present → handler proceeds");
  assert(lastCheckArgs?.includes("/skills/alpha"), "both params present → file_path takes priority");
}

// ── 7. Status policy ────────────────────────────────────────────────────────
console.log("\n[7] Status policy");

{
  mockSkillLedgerStatus("pass");
  const { result, logs } = await fire({
    toolName: "read",
    params: { file_path: "/skills/pass/SKILL.md" },
  });
  assert(result === undefined, "pass → allow without approval");
  assert(logs.length === 0, "pass → no user-visible log");
}

{
  mockSkillLedgerStatus("warn");
  const { result, logs } = await fire({
    toolName: "read",
    params: { file_path: "/skills/warn/SKILL.md" },
  });
  assert(result === undefined, "warn → allow with warning log");
  assert(logs.some((l) => l.includes("low-risk")), "warn → low-risk warning");
}

{
  mockSkillLedgerStatus("error", 1);
  const { result, logs } = await fire({
    toolName: "read",
    params: { file_path: "/skills/error/SKILL.md" },
  });
  assert(result === undefined, "error → allow with warning log");
  assert(logs.some((l) => l.includes("check failed")), "error → check-failed warning");
}

{
  mockSkillLedgerStatus("mystery");
  const { result, logs } = await fire({
    toolName: "read",
    params: { file_path: "/skills/mystery/SKILL.md" },
  });
  assert(result === undefined, "unknown status → allow with warning log");
  assert(logs.some((l) => l.includes("unknown status 'mystery'")), "unknown status → unknown-status warning");
}

{
  mockSkillLedgerStatus("none");
  const { result } = await fire({
    toolName: "read",
    params: { file_path: "/skills/none/SKILL.md" },
  });
  assert(result?.requireApproval?.severity === "warning", "none → requireApproval warning");
  assert(result.requireApproval.description.includes("not been security-scanned"), "none → explains unscanned status");
}

{
  mockSkillLedgerStatus("drifted", 1);
  const { result } = await fire({
    toolName: "read",
    params: { file_path: "/skills/drifted/SKILL.md" },
  });
  assert(result?.requireApproval?.severity === "warning", "drifted → requireApproval warning");
  assert(result.requireApproval.description.includes("content has changed"), "drifted → explains changed content");
}

{
  mockSkillLedgerStatus("deny", 1);
  const { result } = await fire({
    toolName: "read",
    params: { file_path: "/skills/deny/SKILL.md" },
  });
  assert(result?.requireApproval?.severity === "critical", "deny → requireApproval critical");
  assert(result.requireApproval.description.includes("high-risk findings"), "deny → explains high-risk findings");
}

{
  mockSkillLedgerStatus("tampered", 1);
  const { result } = await fire({
    toolName: "read",
    params: { file_path: "/skills/tampered/SKILL.md" },
  });
  assert(result?.requireApproval?.severity === "critical", "tampered → requireApproval critical");
  assert(result.requireApproval.description.includes("signature verification failed"), "tampered → explains signature failure");
}

// ═════════════════════════════════════════════════════════════════════════════
console.log(`\n=== Results: ${passed} passed, ${failed} failed ===`);
if (failed > 0) process.exit(1);
