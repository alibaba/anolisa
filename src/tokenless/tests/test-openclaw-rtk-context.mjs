import assert from "node:assert/strict";
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { after, beforeEach, test } from "node:test";

const testDir = dirname(fileURLToPath(import.meta.url));
const sandbox = mkdtempSync(join(tmpdir(), "tokenless-openclaw-context-"));
const fakeBinDir = join(sandbox, "bin");
const contextDir = join(sandbox, ".tokenless");
const contextFile = join(contextDir, ".rewrite-context");
const redirectedContextDir = join(sandbox, "redirected-tokenless");
const victimFile = join(sandbox, "symlink-victim");
const originalHome = process.env.HOME;
const originalPath = process.env.PATH;

mkdirSync(fakeBinDir, { recursive: true });
const fakeRtk = join(fakeBinDir, "rtk");
writeFileSync(
  fakeRtk,
  `#!/bin/sh
if [ "$1" = "rewrite" ]; then
  if [ "$2" = "no-rewrite" ]; then
    exit 1
  fi
  printf 'rtk optimized %s\n' "$2"
  exit 0
fi
exit 1
`,
);
chmodSync(fakeRtk, 0o755);

process.env.HOME = sandbox;
process.env.PATH = `${fakeBinDir}:${originalPath || ""}`;

const pluginPath = resolve(testDir, "../adapters/tokenless/openclaw/dist/index.js");
assert.equal(
  existsSync(pluginPath),
  true,
  "OpenClaw plugin build missing; run `make build-openclaw-plugin` before this test",
);
const { default: plugin } = await import(pathToFileURL(pluginPath).href);

const handlers = new Map();
plugin.register({
  config: {
    rtk_enabled: true,
    tool_ready_enabled: false,
    response_compression_enabled: false,
    toon_compression_enabled: false,
    verbose: false,
  },
  on(name, handler) {
    handlers.set(name, handler);
  },
});

const beforeToolCall = handlers.get("before_tool_call");
assert.equal(typeof beforeToolCall, "function", "before_tool_call hook was not registered");

function invokeRewrite(command, context = {}) {
  return beforeToolCall(
    { toolName: "exec", params: { command } },
    { toolName: "exec", ...context },
  );
}

beforeEach(() => {
  rmSync(contextDir, { recursive: true, force: true });
  rmSync(redirectedContextDir, { recursive: true, force: true });
  rmSync(victimFile, { force: true });
});

after(() => {
  if (originalHome === undefined) delete process.env.HOME;
  else process.env.HOME = originalHome;
  if (originalPath === undefined) delete process.env.PATH;
  else process.env.PATH = originalPath;
  rmSync(sandbox, { recursive: true, force: true });
});

test("persists RTK context after a successful rewrite", () => {
  const result = invokeRewrite("git status", {
    sessionId: "session-123",
    toolCallId: "tool-456",
  });

  assert.equal(result.params.command, "rtk optimized git status");
  assert.deepEqual(result.params.env, {
    TOKENLESS_AGENT_ID: "openclaw",
    TOKENLESS_SESSION_ID: "session-123",
    TOKENLESS_TOOL_USE_ID: "tool-456",
  });
  assert.equal(readFileSync(contextFile, "utf8"), "openclaw\nsession-123\ntool-456\n");
  assert.equal(statSync(contextDir).mode & 0o777, 0o700);
  assert.equal(statSync(contextFile).mode & 0o777, 0o600);
});

test("keeps rewritten exec context isolated across sessions", () => {
  const first = beforeToolCall(
    {
      toolName: "exec",
      params: {
        command: "first command",
        env: {
          EXISTING_VALUE: "preserved",
          TOKENLESS_TOOL_USE_ID: "stale-tool",
        },
      },
    },
    {
      toolName: "exec",
      sessionId: "session-a",
      toolCallId: "tool-a",
    },
  );
  const second = invokeRewrite("second command", {
    sessionId: "session-b",
    toolCallId: "tool-b",
  });

  assert.equal(first.params.command, "rtk optimized first command");
  assert.deepEqual(first.params.env, {
    EXISTING_VALUE: "preserved",
    TOKENLESS_AGENT_ID: "openclaw",
    TOKENLESS_SESSION_ID: "session-a",
    TOKENLESS_TOOL_USE_ID: "tool-a",
  });
  assert.deepEqual(second.params.env, {
    TOKENLESS_AGENT_ID: "openclaw",
    TOKENLESS_SESSION_ID: "session-b",
    TOKENLESS_TOOL_USE_ID: "tool-b",
  });
  assert.equal(first.params.env.TOKENLESS_TOOL_USE_ID, "tool-a");
});

test("does not persist context when RTK declines the rewrite", () => {
  const result = invokeRewrite("no-rewrite", {
    sessionId: "session-123",
    toolCallId: "tool-456",
  });

  assert.equal(result, undefined);
  assert.equal(existsSync(contextFile), false);
});

test("truncates existing context without reusing IDs from the previous call", () => {
  invokeRewrite("first command", {
    sessionId: "session-123",
    toolCallId: "tool-456",
  });
  const result = invokeRewrite("second command");

  assert.equal(result.params.command, "rtk optimized second command");
  assert.equal(readFileSync(contextFile, "utf8"), "openclaw\n\n\n");
});

test("refuses a symlink without blocking the rewritten command", () => {
  mkdirSync(contextDir, { recursive: true, mode: 0o700 });
  writeFileSync(victimFile, "sentinel");
  symlinkSync(victimFile, contextFile);
  const warnings = [];
  const originalWarn = console.warn;
  console.warn = (message) => warnings.push(String(message));

  let result;
  try {
    result = invokeRewrite("git status", {
      sessionId: "session-123",
      toolCallId: "tool-456",
    });
  } finally {
    console.warn = originalWarn;
  }

  assert.equal(result.params.command, "rtk optimized git status");
  assert.equal(readFileSync(victimFile, "utf8"), "sentinel");
  assert.equal(lstatSync(contextFile).isSymbolicLink(), true);
  assert.equal(warnings.some((warning) => warning.includes("cannot persist rewrite context")), true);
});

test("refuses a symlinked context directory without blocking the rewrite", () => {
  mkdirSync(redirectedContextDir, { recursive: true, mode: 0o700 });
  symlinkSync(redirectedContextDir, contextDir, "dir");
  const redirectedContextFile = join(redirectedContextDir, ".rewrite-context");
  const warnings = [];
  const originalWarn = console.warn;
  console.warn = (message) => warnings.push(String(message));

  let result;
  try {
    result = invokeRewrite("git status", {
      sessionId: "session-123",
      toolCallId: "tool-456",
    });
  } finally {
    console.warn = originalWarn;
  }

  assert.equal(result.params.command, "rtk optimized git status");
  assert.equal(existsSync(redirectedContextFile), false);
  assert.equal(lstatSync(contextDir).isSymbolicLink(), true);
  assert.equal(warnings.some((warning) => warning.includes("cannot persist rewrite context")), true);
});
