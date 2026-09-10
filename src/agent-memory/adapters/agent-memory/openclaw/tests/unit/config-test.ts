/**
 * Unit tests for config resolution.
 *
 * These exercise the exported helpers directly (`validateUserId`,
 * `normalizePositiveInt`) and use `resolveConfig` for end-to-end
 * assertions that don't need a real `agent-memory` binary on PATH.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

const {
  resolveConfig,
  validateUserId,
  normalizePositiveInt,
} = await import("../../src/config.js");

function mockApi(pluginConfig: Record<string, unknown> = {}) {
  return {
    pluginConfig,
    resolvePath: (p: string) => p,
    logger: { info: () => {}, warn: () => {}, debug: () => {} },
  } as any;
}

describe("validateUserId", () => {
  it("accepts a plain ASCII userId", () => {
    assert.equal(validateUserId("alice"), "alice");
  });

  it("accepts digits and dashes", () => {
    assert.equal(validateUserId("user-1234"), "user-1234");
  });

  it("rejects empty", () => {
    assert.throws(() => validateUserId(""), /must not be empty/);
  });

  it("rejects > 128 bytes", () => {
    assert.throws(() => validateUserId("a".repeat(129)), /exceeds 128 bytes/);
  });

  it("accepts exactly 128 bytes", () => {
    assert.equal(validateUserId("a".repeat(128)).length, 128);
  });

  // The Rust side counts UTF-8 bytes (`str::len() > 128` in
  // `ns::mod.rs::validate_user_id`, whose own error text says "bytes"), so
  // this mirror has to count them too. A value accepted here and rejected
  // there is not a hard failure — it is silently swapped in the subprocess:
  // `userId` falls back to the OS uid (`config.rs::read_validated_user_id_env`
  // only warns) and `sessionId` to a freshly generated one (`service/mod.rs`),
  // which is precisely the pinning the operator asked for, lost without an
  // error anywhere the operator would look.
  it("rejects 43 CJK characters (43 code points, 129 bytes)", () => {
    const cjk = "\u5b57".repeat(43);
    assert.equal(cjk.length, 43);
    assert.equal([...cjk].length, 43);
    assert.equal(Buffer.byteLength(cjk, "utf8"), 129);
    assert.throws(() => validateUserId(cjk), /length 129 exceeds 128 bytes/);
  });

  it("rejects 33 emoji (66 code units, 33 code points, 132 bytes)", () => {
    const emoji = "\u{1f600}".repeat(33);
    assert.equal(emoji.length, 66);
    assert.equal([...emoji].length, 33);
    assert.equal(Buffer.byteLength(emoji, "utf8"), 132);
    assert.throws(() => validateUserId(emoji), /length 132 exceeds 128 bytes/);
  });

  it("accepts 42 CJK characters (126 bytes)", () => {
    const cjk = "\u5b57".repeat(42);
    assert.equal(Buffer.byteLength(cjk, "utf8"), 126);
    assert.equal(validateUserId(cjk), cjk);
  });

  it("accepts exactly 128 bytes of mixed-width text", () => {
    const mixed = "\u5b57".repeat(42) + "ab"; // 126 + 2 bytes, 44 code points
    assert.equal(Buffer.byteLength(mixed, "utf8"), 128);
    assert.equal(validateUserId(mixed), mixed);
  });

  it("rejects '..' substring", () => {
    assert.throws(() => validateUserId("foo..bar"), /contains '\.\.'/);
  });

  it("rejects forward slash", () => {
    assert.throws(() => validateUserId("a/b"), /path separator/);
  });

  it("rejects backslash", () => {
    assert.throws(() => validateUserId("a\\b"), /path separator/);
  });

  it("rejects null byte", () => {
    assert.throws(() => validateUserId("a\0b"), /control character/);
  });

  it("rejects newline", () => {
    assert.throws(() => validateUserId("a\nb"), /control character/);
  });

  it("rejects DEL (0x7f)", () => {
    assert.throws(() => validateUserId("a\x7fb"), /control character/);
  });

  it("rejects C1 control char (0x9b)", () => {
    assert.throws(() => validateUserId("ab"), /control character/);
  });
});

describe("normalizePositiveInt", () => {
  it("returns fallback for non-numeric", () => {
    assert.equal(normalizePositiveInt("notnumber", 42), 42);
  });

  it("returns fallback for zero", () => {
    assert.equal(normalizePositiveInt(0, 42), 42);
  });

  it("returns fallback for negative", () => {
    assert.equal(normalizePositiveInt(-1, 42), 42);
  });

  it("accepts a plain number", () => {
    assert.equal(normalizePositiveInt(2048, 42), 2048);
  });

  it("parses numeric strings", () => {
    assert.equal(normalizePositiveInt("2048", 42), 2048);
  });

  it("floors fractional values", () => {
    assert.equal(normalizePositiveInt(2048.9, 42), 2048);
  });

  it("rejects values above the hard cap (4 GiB)", () => {
    const fourG = 4 * 1024 * 1024 * 1024;
    // Anything beyond the cap falls back, with a stderr warning.
    assert.equal(normalizePositiveInt(fourG + 1, 42), 42);
  });

  it("respects a custom cap", () => {
    assert.equal(normalizePositiveInt(1000, 42, 500), 42);
    assert.equal(normalizePositiveInt(400, 42, 500), 400);
  });
});

describe("resolveConfig (userId surface)", () => {
  it("propagates validateUserId rejection of '..'", () => {
    // Use a payload that contains `..` but no '/' or '\\', so the
    // '..' check fires before the path-separator check.
    assert.throws(() => resolveConfig(mockApi({ userId: "foo..bar" })), /contains '\.\.'/);
  });

  it("propagates validateUserId rejection of '/'", () => {
    assert.throws(() => resolveConfig(mockApi({ userId: "a/b" })), /path separator/);
  });

  it("propagates validateUserId rejection of '\\\\'", () => {
    assert.throws(() => resolveConfig(mockApi({ userId: "a\\b" })), /path separator/);
  });

  it("propagates validateUserId rejection of NUL", () => {
    assert.throws(() => resolveConfig(mockApi({ userId: "a\0b" })), /control character/);
  });

  it("accepts a valid userId (may fail later on missing binary)", () => {
    try {
      resolveConfig(mockApi({ userId: "user123" }));
    } catch (err: any) {
      // OK to fail on binary lookup; must NOT fail on userId.
      assert.ok(
        !err.message.includes("userId"),
        `did not expect a userId error: ${err.message}`,
      );
      assert.ok(
        err.message.includes("binary"),
        `expected binary-related error, got: ${err.message}`,
      );
    }
  });
});

describe("resolveConfig sessionId (R6-1 regression)", () => {
  it("generates a `ses_<hex>` sessionId by default", () => {
    delete process.env["MEMORY_SESSION_ID"];
    try {
      const cfg = resolveConfig(mockApi({}));
      assert.match(cfg.sessionId, /^ses_[0-9a-f]+$/);
    } catch (err: any) {
      // If the test machine has no binary, the config still went through
      // sessionId resolution before the binary check. We can't assert on
      // sessionId then — skip this case rather than fail.
      assert.ok(err.message.includes("binary"), err.message);
    }
  });

  it("honours MEMORY_SESSION_ID env when no plugin config overrides it", () => {
    process.env["MEMORY_SESSION_ID"] = "ses_abcdef";
    try {
      const cfg = resolveConfig(mockApi({}));
      assert.equal(cfg.sessionId, "ses_abcdef");
    } catch (err: any) {
      assert.ok(err.message.includes("binary"), err.message);
    } finally {
      delete process.env["MEMORY_SESSION_ID"];
    }
  });

  it("explicit plugin-config sessionId wins over env", () => {
    process.env["MEMORY_SESSION_ID"] = "ses_fromenv";
    try {
      const cfg = resolveConfig(mockApi({ sessionId: "ses_fromcfg" }));
      assert.equal(cfg.sessionId, "ses_fromcfg");
    } catch (err: any) {
      assert.ok(err.message.includes("binary"), err.message);
    } finally {
      delete process.env["MEMORY_SESSION_ID"];
    }
  });

  it("sessionId still validated by validateUserId rules", () => {
    assert.throws(
      () => resolveConfig(mockApi({ sessionId: "../escape" })),
      /path separator|control|contains/,
    );
  });

  it("rejects a multibyte sessionId the manifest's code-point bound accepts", () => {
    // JSON Schema `maxLength` counts code points and has no byte-length
    // keyword, so the manifest cannot express the Rust limit; the resolver is
    // the binding check. 43 code points sail through `maxLength: 128` and are
    // 129 UTF-8 bytes.
    const sessionId = "\u5b57".repeat(43);
    assert.equal([...sessionId].length, 43);
    assert.throws(
      () => resolveConfig(mockApi({ sessionId })),
      /length 129 exceeds 128 bytes/,
    );
  });

  it("sessionDir defaults to /run/anolisa/sessions", () => {
    delete process.env["MEMORY_SESSION_DIR"];
    try {
      const cfg = resolveConfig(mockApi({}));
      assert.equal(cfg.sessionDir, "/run/anolisa/sessions");
    } catch (err: any) {
      assert.ok(err.message.includes("binary"), err.message);
    }
  });
});
