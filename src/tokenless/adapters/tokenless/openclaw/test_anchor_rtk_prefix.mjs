/**
 * Unit tests for anchorRtkPrefix in index.ts.
 *
 * Run with: node test_anchor_rtk_prefix.mjs
 * Uses node:test (Node.js >= 18). Mirrors the case matrix from
 * tests/test_rewrite_hook.py to ensure consistent behaviour across adapters.
 *
 * The anchor logic is reimplemented inline so this file has no runtime
 * dependency on the plugin's side-effecting binary checks.
 */

import { test } from "node:test";
import assert from "node:assert/strict";

// ---- Inline copy of anchorRtkPrefix (kept in sync with index.ts) ------------
// This avoids importing index.ts and triggering its module-level binary checks.

const SEGMENT_OPS = new Set(["&&", "||", ";", "|", "&"]);

function isEnvAssignment(token) {
  const eq = token.indexOf("=");
  if (eq <= 0) return false;
  const name = token.slice(0, eq);
  if (!/^[A-Za-z_]/.test(name)) return false;
  return /^[A-Za-z0-9_]+$/.test(name);
}

function anchorRtkPrefix(rewritten, resolvedRtkPath) {
  const tokens = [];
  let i = 0;
  while (i < rewritten.length) {
    if (rewritten[i] === " " || rewritten[i] === "\t") { i++; continue; }
    if (rewritten[i] === "'" || rewritten[i] === '"') {
      const q = rewritten[i];
      let j = i + 1;
      while (j < rewritten.length && rewritten[j] !== q) j++;
      tokens.push(rewritten.slice(i, j + 1));
      i = j + 1;
      continue;
    }
    let j = i;
    while (j < rewritten.length && rewritten[j] !== " " && rewritten[j] !== "\t") {
      if (rewritten[j] === "'" || rewritten[j] === '"') break;
      j++;
    }
    tokens.push(rewritten.slice(i, j));
    i = j;
  }

  const quoted = /[\s'"\\$`!#&;|<>(){}]/.test(resolvedRtkPath)
    ? `'${resolvedRtkPath.replace(/'/g, "'\\''")}'`
    : resolvedRtkPath;

  let wrapped = false;
  for (let idx = 0; idx < tokens.length; idx++) {
    const tok = tokens[idx];
    if (SEGMENT_OPS.has(tok)) { wrapped = false; continue; }
    if (isEnvAssignment(tok)) continue;
    if (!wrapped && tok === "rtk") { tokens[idx] = quoted; wrapped = true; }
  }

  return tokens.join(" ");
}

// ---- Tests ------------------------------------------------------------------

const RTK = "/usr/local/lib/anolisa/tokenless/rtk";

test("anchors bare rtk at command start", () => {
  assert.equal(
    anchorRtkPrefix("rtk grep foo bar", RTK),
    `${RTK} grep foo bar`,
  );
});

test("anchors each segment in a pipeline (&&)", () => {
  assert.equal(
    anchorRtkPrefix("rtk grep --cached foo && rtk git status", RTK),
    `${RTK} grep --cached foo && ${RTK} git status`,
  );
});

test("anchors after sudo wrapper", () => {
  assert.equal(
    anchorRtkPrefix("sudo rtk git status", RTK),
    `sudo ${RTK} git status`,
  );
});

test("anchors after env assignment", () => {
  assert.equal(
    anchorRtkPrefix("RUST_BACKTRACE=1 rtk cargo test", RTK),
    `RUST_BACKTRACE=1 ${RTK} cargo test`,
  );
});

test("anchors after single & (background connective)", () => {
  assert.equal(
    anchorRtkPrefix("git status & rtk grep foo", RTK),
    `git status & ${RTK} grep foo`,
  );
});

test("quoted rtk pattern inside single-quoted arg is untouched", () => {
  assert.equal(
    anchorRtkPrefix("rtk grep -E 'foo|rtk bar' src/", RTK),
    `${RTK} grep -E 'foo|rtk bar' src/`,
  );
});

test("unquoted glob is preserved (not re-quoted)", () => {
  assert.equal(
    anchorRtkPrefix("rtk grep foo *.txt", RTK),
    `${RTK} grep foo *.txt`,
  );
});

test("hash argument not treated as comment (preserved)", () => {
  assert.equal(
    anchorRtkPrefix("rtk grep foo #include src/", RTK),
    `${RTK} grep foo #include src/`,
  );
});

test("fd merge redirection preserved (2>&1)", () => {
  assert.equal(
    anchorRtkPrefix("rtk git log 2>&1 | rtk head", RTK),
    `${RTK} git log 2>&1 | ${RTK} head`,
  );
});

test("fd redirection to /dev/null preserved (2>/dev/null)", () => {
  assert.equal(
    anchorRtkPrefix("rtk git status 2>/dev/null", RTK),
    `${RTK} git status 2>/dev/null`,
  );
});

test("command substitution token preserved ($(date))", () => {
  assert.equal(
    anchorRtkPrefix("rtk echo $(date)", RTK),
    `${RTK} echo $(date)`,
  );
});

test("path with spaces is shell-quoted", () => {
  const rtk = "/home/user/my tools/rtk";
  assert.equal(
    anchorRtkPrefix("rtk git status", rtk),
    `'/home/user/my tools/rtk' git status`,
  );
});

test("non-rtk command is returned unchanged", () => {
  assert.equal(
    anchorRtkPrefix("git status", RTK),
    "git status",
  );
});

test("empty string is returned unchanged", () => {
  assert.equal(anchorRtkPrefix("", RTK), "");
});

test("already-anchored path is not double-anchored", () => {
  const already = `${RTK} git status`;
  assert.equal(anchorRtkPrefix(already, RTK), already);
});
