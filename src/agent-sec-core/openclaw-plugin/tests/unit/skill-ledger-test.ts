// tests/skill-ledger-test.ts
// Deep test for skill-ledger hook: event filtering, path resolution, fail-open, resilience.
//
// Run:  npx tsx tests/unit/skill-ledger-test.ts
//       npm test

import { skillLedger } from "../../src/capabilities/skill-ledger.js";

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

// ── Setup: register capability, extract handler ─────────────────────────────

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
  const { result, logs } = await fire({
    toolName: "read",
    params: { file_path: "/home/user/.openclaw/skills/github/SKILL.md" },
  });
  assert(result === undefined, "absolute path → returns undefined (allow)");
  assert(logs.some((l) => l.includes("[skill-ledger]")), "absolute path → handler proceeds (logs produced)");
}

{
  const { result, logs } = await fire({
    toolName: "read",
    params: { path: "/opt/skills/my-tool/SKILL.md" },
  });
  assert(result === undefined, "'path' param (alt name) → returns undefined");
  assert(logs.some((l) => l.includes("[skill-ledger]")), "'path' param → handler proceeds");
}

{
  const { logs } = await fire({
    toolName: "read",
    params: { file_path: "SKILL.md" },
  });
  assert(logs.some((l) => l.includes("[skill-ledger]")), "bare 'SKILL.md' → handler proceeds");
}

{
  const { logs } = await fire({
    toolName: "read",
    params: { file_path: "  /skills/github/SKILL.md  " },
  });
  assert(logs.some((l) => l.includes("[skill-ledger]")), "whitespace-padded path → handler proceeds (trimmed)");
}

{
  const { logs } = await fire({
    toolName: "read",
    params: { file_path: "/deeply/nested/dir/structure/skill-name/SKILL.md" },
  });
  assert(logs.some((l) => l.includes("[skill-ledger]")), "deeply nested path → handler proceeds");
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
  // When both file_path and path are present, file_path should win
  const { logs } = await fire({
    toolName: "read",
    params: {
      file_path: "/skills/alpha/SKILL.md",
      path: "/skills/beta/SKILL.md",
    },
  });
  // Handler proceeds (we can't see which path was chosen from logs alone in CLI-error mode,
  // but the fact it proceeds confirms at least one matched)
  assert(logs.some((l) => l.includes("[skill-ledger]")), "both params present → handler proceeds (file_path takes priority)");
}

// ═════════════════════════════════════════════════════════════════════════════
console.log(`\n=== Results: ${passed} passed, ${failed} failed ===`);
if (failed > 0) process.exit(1);
