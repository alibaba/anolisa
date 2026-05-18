import { afterEach, describe, it } from "node:test";
import assert from "node:assert/strict";
import { callAgentSecCli, _resetCliMock } from "../../src/utils.js";

describe("utils", () => {
  const originalPath = process.env.PATH;

  afterEach(() => {
    _resetCliMock();
    if (originalPath === undefined) {
      delete process.env.PATH;
    } else {
      process.env.PATH = originalPath;
    }
  });

  it("preserves spawn error details when agent-sec-cli cannot be started", async () => {
    process.env.PATH = "";

    const result = await callAgentSecCli(["observability", "record"], {
      timeout: 100,
    });

    assert.equal(result.exitCode, 1);
    assert.match(result.stderr, /agent-sec-cli/);
    assert.match(result.stderr, /ENOENT/);
  });
});
