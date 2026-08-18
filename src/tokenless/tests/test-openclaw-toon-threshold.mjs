// Tests for the OpenClaw plugin TOON minimum payload threshold.
//
// TOON on small JSON saves only a handful of characters (~0.3% below
// ~500 chars) while the per-event encode cost stays the same, so the
// plugin must skip the TOON step for payloads under MIN_TOON_CHARS (500)
// without spawning `tokenless compress-toon`. Larger payloads keep
// flowing to TOON.
//
// Requires `make build-openclaw-plugin` (loads dist/index.js).

import assert from "node:assert/strict";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { after, test } from "node:test";

const testDir = dirname(fileURLToPath(import.meta.url));
const sandbox = mkdtempSync(join(tmpdir(), "tokenless-openclaw-toon-"));
const localBinDir = join(sandbox, ".local", "bin");
const fakeTokenless = join(localBinDir, "tokenless");
const toonMarker = join(sandbox, "toon_called");

const originalHome = process.env.HOME;
const originalPath = process.env.PATH;
process.env.HOME = sandbox;

mkdirSync(localBinDir, { recursive: true });

// Fake tokenless CLI: compress-toon records a call marker and emits a
// smaller TOON-like output so tests can tell whether the plugin invoked
// compress-toon at all.
writeFileSync(
  fakeTokenless,
  [
    "#!/usr/bin/env python3",
    "import os, sys",
    "data = sys.stdin.read()",
    'if len(sys.argv) > 1 and sys.argv[1] == "compress-toon":',
    "    marker = os.path.join(os.path.dirname(os.path.abspath(__file__)),",
    '                        "..", "..", "toon_called")',
    '    open(marker, "a").close()',
    '    print("toon:" + data[: len(data) // 2])',
    "else:",
    '    print("{}")',
    "",
  ].join("\n"),
);
chmodSync(fakeTokenless, 0o755);

// resolveBinaryPath searches PATH before the fallback locations, so the
// fake tokenless must shadow any system install.
process.env.PATH = `${localBinDir}${originalPath ? ":" + originalPath : ""}`;

const pluginPath = resolve(testDir, "../adapters/tokenless/openclaw/dist/index.js");
assert.ok(
  existsSync(pluginPath),
  "OpenClaw plugin build missing; run `make build-openclaw-plugin` before this test",
);

const { default: plugin } = await import(pathToFileURL(pluginPath).href);

const handlers = new Map();
plugin.register({
  config: {
    rtk_enabled: false,
    tool_ready_enabled: false,
    response_compression_enabled: false,
    toon_compression_enabled: true,
    verbose: false,
  },
  on(name, handler) {
    handlers.set(name, handler);
  },
});

const toolResultPersist = handlers.get("tool_result_persist");
assert.equal(
  typeof toolResultPersist,
  "function",
  "tool_result_persist hook was not registered",
);

function messageOfSize(charTarget) {
  // JSON.stringify adds the {"stdout":"","exit_code":0} frame (~30 chars).
  const inner = "x".repeat(Math.max(charTarget - 30, 1));
  return { stdout: inner, exit_code: 0 };
}

after(() => {
  if (originalHome === undefined) delete process.env.HOME;
  else process.env.HOME = originalHome;
  if (originalPath === undefined) delete process.env.PATH;
  else process.env.PATH = originalPath;
  rmSync(sandbox, { recursive: true, force: true });
});

test("payload under MIN_TOON_CHARS skips compress-toon entirely", () => {
  const result = toolResultPersist(
    { toolName: "web_fetch", toolCallId: "tool-small", message: messageOfSize(300) },
    { toolName: "web_fetch", sessionId: "session-small", toolCallId: "tool-small" },
  );

  assert.equal(result, undefined, "small payloads pass through unchanged");
  assert.ok(
    !existsSync(toonMarker),
    "compress-toon must not run below the threshold",
  );
});

test("payload at/above MIN_TOON_CHARS still TOON encodes", () => {
  const result = toolResultPersist(
    { toolName: "web_fetch", toolCallId: "tool-large", message: messageOfSize(800) },
    { toolName: "web_fetch", sessionId: "session-large", toolCallId: "tool-large" },
  );

  assert.ok(existsSync(toonMarker), "compress-toon must run for large payloads");
  assert.ok(result && typeof result.message === "string",
    "large payloads get the TOON-encoded message");
  assert.ok(result.message.startsWith("toon:"),
    `expected TOON output, got: ${String(result.message).slice(0, 40)}`);
});

function emojiPayload(emojiCount) {
  // Each "😀" is one Unicode code point but two UTF-16 code units, so a
  // String.length-based gate counts these payloads roughly double.
  return { stdout: "😀".repeat(emojiCount), exit_code: 0 };
}

test("non-BMP payload under MIN_TOON_CHARS code points skips compress-toon", () => {
  // 244 emoji + the JSON frame ≈ 271 code points but ≈ 515 UTF-16 code
  // units; a UTF-16 count would wrongly pass the 500 threshold.
  rmSync(toonMarker, { force: true });
  const result = toolResultPersist(
    { toolName: "web_fetch", toolCallId: "tool-emoji", message: emojiPayload(244) },
    { toolName: "web_fetch", sessionId: "session-emoji", toolCallId: "tool-emoji" },
  );

  assert.equal(result, undefined, "under-threshold non-BMP payloads pass through unchanged");
  assert.ok(
    !existsSync(toonMarker),
    "compress-toon must not run when the code-point count is below the threshold",
  );
});

test("non-BMP payload at/above MIN_TOON_CHARS code points still TOON encodes", () => {
  // 520 emoji + frame ≈ 547 code points (≈ 1067 UTF-16 units): above the
  // threshold in either counting unit, so TOON must still run.
  rmSync(toonMarker, { force: true });
  const result = toolResultPersist(
    { toolName: "web_fetch", toolCallId: "tool-emoji-large", message: emojiPayload(520) },
    { toolName: "web_fetch", sessionId: "session-emoji-large", toolCallId: "tool-emoji-large" },
  );

  assert.ok(existsSync(toonMarker), "compress-toon must run at/above the code-point threshold");
  assert.ok(result && typeof result.message === "string",
    "large non-BMP payloads get the TOON-encoded message");
  assert.ok(result.message.startsWith("toon:"),
    `expected TOON output, got: ${String(result.message).slice(0, 40)}`);
});
