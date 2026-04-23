// tests/smoke-test.ts
import { testCapability } from "./test-harness.js";
import { codeScan } from "../src/capabilities/code-scan.js";
import { promptScan } from "../src/capabilities/prompt-scan.js";
import { skillLedger } from "../src/capabilities/skill-ledger.js";

// 每个 hook 的 mock 事件（字段与真实类型一致）
// Note: before_tool_call has two entries — one for exec-based tools (code-scan)
// and one for read-based tools (skill-ledger). The shared mock uses "exec" for
// code-scan / prompt-scan. skill-ledger uses its own dedicated mock events below.
const mockEvents: Record<string, Record<string, unknown>> = {
  before_tool_call: {
    toolName: "exec",
    params: { command: "ls -la" },
    runId: "run-001",
    toolCallId: "tc-001",
  },
  before_dispatch: {
    content: "hello world",
    body: "hello world",
    senderId: "user-123",
    isGroup: false,
  },
};

// 每个 hook 的 mock ctx（提供代表性字段值）
const mockCtx: Record<string, Record<string, unknown>> = {
  before_tool_call: {
    sessionKey: "sk-001", runId: "run-001", toolName: "exec", toolCallId: "tc-001",
  },
  before_dispatch: {
    channelId: "telegram", sessionKey: "sk-001", senderId: "user-123",
  },
};

const caps = [codeScan, promptScan];

// skill-ledger needs a dedicated mock with read + SKILL.md path
const skillLedgerMockEvents: Record<string, Record<string, unknown>> = {
  ...mockEvents,
  before_tool_call: {
    toolName: "read",
    params: { file_path: "/home/user/.openclaw/skills/github/SKILL.md" },
    runId: "run-002",
    toolCallId: "tc-002",
  },
};
const skillLedgerMockCtx: Record<string, Record<string, unknown>> = {
  ...mockCtx,
  before_tool_call: {
    sessionKey: "sk-001", runId: "run-002", toolName: "read", toolCallId: "tc-002",
  },
};

console.log("=== Agent-Sec Smoke Test ===");
console.log(`Mode: ${process.env.AGENT_SEC_LIVE ? "LIVE (real CLI)" : "MOCK (no CLI needed)"}\n`);

for (const cap of caps) {
  console.log(`[${cap.id}] hooks: [${cap.hooks.join(", ")}]`);
  const results = await testCapability(cap, mockEvents, undefined, mockCtx);
  for (const r of results) {
    const status = r.error ? `FAIL: ${r.error.message}` : "OK";
    const detail = r.result ? ` → ${JSON.stringify(r.result)}` : "";
    console.log(`  ${r.hookName}: ${status} (${r.durationMs.toFixed(0)}ms)${detail}`);
  }
  console.log();
}

// ── skill-ledger (separate mock events) ──────────────────────────
console.log(`[${skillLedger.id}] hooks: [${skillLedger.hooks.join(", ")}]`);
const slResults = await testCapability(skillLedger, skillLedgerMockEvents, undefined, skillLedgerMockCtx);
for (const r of slResults) {
  const status = r.error ? `FAIL: ${r.error.message}` : "OK";
  const detail = r.result ? ` → ${JSON.stringify(r.result)}` : "";
  console.log(`  ${r.hookName}: ${status} (${r.durationMs.toFixed(0)}ms)${detail}`);
}
console.log();
