import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

// Reset module-level `alreadyEnsured` guard between tests.
// We import the module fresh each time via dynamic import + vi.resetModules.

describe("WS_CKPT_TOOL_NAMES", () => {
  it("contains 7 tool names", async () => {
    const { WS_CKPT_TOOL_NAMES } = await import("../whitelist.js");
    expect(WS_CKPT_TOOL_NAMES).toHaveLength(7);
  });

  it("all start with ws-ckpt-", async () => {
    const { WS_CKPT_TOOL_NAMES } = await import("../whitelist.js");
    for (const name of WS_CKPT_TOOL_NAMES) {
      expect(name).toMatch(/^ws-ckpt-/);
    }
  });

  it("includes known tools", async () => {
    const { WS_CKPT_TOOL_NAMES } = await import("../whitelist.js");
    expect(WS_CKPT_TOOL_NAMES).toContain("ws-ckpt-checkpoint");
    expect(WS_CKPT_TOOL_NAMES).toContain("ws-ckpt-rollback");
    expect(WS_CKPT_TOOL_NAMES).toContain("ws-ckpt-list");
    expect(WS_CKPT_TOOL_NAMES).toContain("ws-ckpt-config");
    expect(WS_CKPT_TOOL_NAMES).toContain("ws-ckpt-status");
  });
});

describe("ensureToolsAlsoAllow", () => {
  let origEnv: Record<string, string | undefined>;

  beforeEach(() => {
    vi.resetModules();
    origEnv = { ...process.env };
  });

  afterEach(() => {
    process.env = origEnv;
    vi.restoreAllMocks();
  });

  it("adds missing tools to openclaw.json", async () => {
    const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "ws-ckpt-test-"));
    const configPath = path.join(tmpDir, "openclaw.json");
    fs.writeFileSync(configPath, JSON.stringify({ tools: { alsoAllow: [] } }));

    process.env.OPENCLAW_CONFIG_PATH = configPath;

    const { ensureToolsAlsoAllow, WS_CKPT_TOOL_NAMES } = await import(
      "../whitelist.js"
    );

    const api = {
      config: { tools: { alsoAllow: [] } },
    } as any;

    ensureToolsAlsoAllow(api);

    const written = JSON.parse(fs.readFileSync(configPath, "utf-8"));
    expect(written.tools.alsoAllow).toEqual(
      expect.arrayContaining(WS_CKPT_TOOL_NAMES),
    );

    // Cleanup
    fs.rmSync(tmpDir, { recursive: true });
  });

  it("skips when all tools already present", async () => {
    const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "ws-ckpt-test-"));
    const configPath = path.join(tmpDir, "openclaw.json");

    const { WS_CKPT_TOOL_NAMES } = await import("../whitelist.js");
    fs.writeFileSync(
      configPath,
      JSON.stringify({ tools: { alsoAllow: [...WS_CKPT_TOOL_NAMES] } }),
    );

    process.env.OPENCLAW_CONFIG_PATH = configPath;

    // Re-import to reset alreadyEnsured
    vi.resetModules();
    const mod = await import("../whitelist.js");

    const writeSpy = vi.spyOn(fs, "writeFileSync");

    const api = {
      config: { tools: { alsoAllow: [...WS_CKPT_TOOL_NAMES] } },
    } as any;

    mod.ensureToolsAlsoAllow(api);
    expect(writeSpy).not.toHaveBeenCalled();

    fs.rmSync(tmpDir, { recursive: true });
  });

  it("handles missing config file gracefully — uses api.config fallback", async () => {
    process.env.OPENCLAW_CONFIG_PATH = "/nonexistent/openclaw.json";

    const mod = await import("../whitelist.js");

    // api has empty alsoAllow, so tools are "missing" and the write will
    // fail because the dir doesn't exist — but ensureToolsAlsoAllow should
    // catch the error and not throw.
    const api = {
      config: { tools: { alsoAllow: [] } },
    } as any;

    expect(() => mod.ensureToolsAlsoAllow(api)).not.toThrow();
  });
});
