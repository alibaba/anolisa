#!/usr/bin/env node
/**
 * Unit tests for the OpenClaw anchorRtkPrefix helper.
 *
 * Covers the same case matrix as test_rewrite_hook.py:
 *   - wrapper position (sudo)
 *   - env assignments
 *   - single & connective
 *   - quoted patterns left intact
 *   - unquoted globs preserved
 *   - fd redirections preserved
 *   - command substitutions preserved
 *   - multiple pipeline segments
 *   - escaped double quotes in arguments (regression: shellTokenize P1)
 *   - single quotes in RTK path (regression: quoting P1)
 *   - rtk argument vs command position (regression: PR #2249 P1)
 *   - RTK v0.43 transparent prefixes: built-ins (noglob, command,
 *     builtin, exec, nocorrect, uv run) and configured multi-word
 *     [hooks].transparent_prefixes (regression: PR #2249 P1)
 *   - wrapper option operands never anchored (sudo -u / env -u /
 *     command -v, regression: PR #2249 round-5 P1)
 *
 * Imports the production helpers from the compiled plugin build so CI
 * catches any drift between the test expectations and the shipped code.
 * Requires ``make build-openclaw-plugin`` before running.
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  SEGMENT_OPS,
  isEnvAssignment,
  shellTokenize,
  anchorRtkPrefix,
  parseHooksTransparentPrefixes,
} from "../adapters/tokenless/openclaw/dist/anchor-helpers.js";

// ---- Sandboxed rtk config ----------------------------------------------------
//
// anchorRtkPrefix reads the user's [hooks].transparent_prefixes from rtk's
// config.toml.  Point HOME/XDG_CONFIG_HOME at a temp dir carrying a known
// config so the tests are deterministic on any host (with or without a real
// rtk installation / user config), and restore the original env at exit.

const ORIG_HOME = process.env.HOME;
const ORIG_XDG = process.env.XDG_CONFIG_HOME;
const SANDBOX_HOME = mkdtempSync(join(tmpdir(), "openclaw-anchor-home-"));
mkdirSync(join(SANDBOX_HOME, ".config", "rtk"), { recursive: true });
writeFileSync(
  join(SANDBOX_HOME, ".config", "rtk", "config.toml"),
  "[hooks]\n" +
    'exclude_commands = ["curl"]\n' +
    'transparent_prefixes = ["shadowenv exec --", "docker exec c1", "foo bar"]\n',
);
process.env.HOME = SANDBOX_HOME;
process.env.XDG_CONFIG_HOME = join(SANDBOX_HOME, ".config");
process.on("exit", () => {
  if (ORIG_HOME === undefined) delete process.env.HOME;
  else process.env.HOME = ORIG_HOME;
  if (ORIG_XDG === undefined) delete process.env.XDG_CONFIG_HOME;
  else process.env.XDG_CONFIG_HOME = ORIG_XDG;
  rmSync(SANDBOX_HOME, { recursive: true, force: true });
});

// ---- Tests ------------------------------------------------------------------

const RTK = "/home/user/.local/share/anolisa/tokenless/rtk";

test("simple single command", () => {
  assert.equal(
    anchorRtkPrefix("rtk grep foo bar", RTK),
    `${RTK} grep foo bar`,
  );
});

test("multiple pipeline segments separated by &&", () => {
  assert.equal(
    anchorRtkPrefix("rtk grep --cached foo && rtk git status", RTK),
    `${RTK} grep --cached foo && ${RTK} git status`,
  );
});

test("wrapper before rtk (sudo)", () => {
  assert.equal(
    anchorRtkPrefix("sudo rtk git status", RTK),
    `sudo ${RTK} git status`,
  );
});

test("leading env assignment", () => {
  assert.equal(
    anchorRtkPrefix("RUST_BACKTRACE=1 rtk cargo test", RTK),
    `RUST_BACKTRACE=1 ${RTK} cargo test`,
  );
});

test("single & connective", () => {
  assert.equal(
    anchorRtkPrefix("git status & rtk grep foo", RTK),
    `git status & ${RTK} grep foo`,
  );
});

test("quoted regex pattern with rtk inside is untouched", () => {
  assert.equal(
    anchorRtkPrefix("rtk grep -E 'foo|rtk bar' src/", RTK),
    `${RTK} grep -E 'foo|rtk bar' src/`,
  );
});

test("unquoted glob preserved", () => {
  assert.equal(
    anchorRtkPrefix("rtk grep foo *.txt", RTK),
    `${RTK} grep foo *.txt`,
  );
});

test("hash argument not treated as comment", () => {
  assert.equal(
    anchorRtkPrefix("rtk grep foo #include src/", RTK),
    `${RTK} grep foo #include src/`,
  );
});

test("fd merge token preserved (2>&1)", () => {
  assert.equal(
    anchorRtkPrefix("rtk git log 2>&1 | rtk head", RTK),
    `${RTK} git log 2>&1 | ${RTK} head`,
  );
});

test("fd redirection token preserved (2>/dev/null)", () => {
  assert.equal(
    anchorRtkPrefix("rtk git status 2>/dev/null", RTK),
    `${RTK} git status 2>/dev/null`,
  );
});

test("command substitution preserved $(date)", () => {
  assert.equal(
    anchorRtkPrefix("rtk echo $(date)", RTK),
    `${RTK} echo $(date)`,
  );
});

test("rtk path with spaces is single-quoted", () => {
  const spacedRtk = "/path with spaces/rtk";
  assert.equal(
    anchorRtkPrefix("rtk grep foo", spacedRtk),
    `'${spacedRtk}' grep foo`,
  );
});

test("no rtk token — passthrough unchanged", () => {
  const cmd = "git status && grep foo bar";
  assert.equal(anchorRtkPrefix(cmd, RTK), cmd);
});

test("unparseable input (unmatched quote) — returned untouched", () => {
  const cmd = "rtk grep 'unclosed";
  assert.equal(anchorRtkPrefix(cmd, RTK), cmd);
});

// ---- Regression tests (PR #2249 review) ------------------------------------

test("escaped double quote inside double-quoted argument", () => {
  // shellTokenize must skip \" inside double quotes instead of treating
  // the backslash-quote as the closing delimiter (P1 review finding).
  const cmd = 'rtk grep "foo\\"bar" src/';
  const tokens = shellTokenize(cmd);
  assert.notEqual(tokens, null, "tokenize must not return null for escaped quote");
  assert.equal(
    anchorRtkPrefix(cmd, RTK),
    `${RTK} grep "foo\\"bar" src/`,
  );
});

test("escaped backslash before closing double quote", () => {
  // \\\\ inside double quotes: the backslash escapes the next backslash,
  // so the closing quote is the one after the second backslash.
  const cmd = 'rtk echo "path\\\\end" done';
  const tokens = shellTokenize(cmd);
  assert.notEqual(tokens, null);
  assert.deepEqual(tokens, ["rtk", "echo", '"path\\\\end"', "done"]);
});

test("rtk path containing single quote is properly escaped", () => {
  // A path like /home/o'brien/rtk must not produce broken shell quoting.
  const trickyRtk = "/home/o'brien/rtk";
  const result = anchorRtkPrefix("rtk grep foo", trickyRtk);
  // Expected: '/home/o'\''brien/rtk' grep foo  (standard shell single-quote escaping)
  assert.equal(
    result,
    `'/home/o'\\''brien/rtk' grep foo`,
  );
});

test("rtk as positional argument is not anchored", () => {
  // A bare rtk that appears as an argument (not in command position)
  // must stay bare.  Regression: mixed compound rewrite kept an ignored
  // segment unchanged (e.g. "echo rtk") and the rtk was mis-anchored.
  assert.equal(anchorRtkPrefix("echo rtk done", RTK), "echo rtk done");
});

test("ignored segment with rtk argument plus rewritten segment", () => {
  // RTK 0.43 compound rewrite keeps ignored commands unchanged and
  // rewrites the active segment: "echo rtk && git status" becomes
  // "echo rtk && rtk git status".  Only the command-position rtk in the
  // second segment is anchored; the argument rtk in the echo segment
  // must remain bare.
  assert.equal(
    anchorRtkPrefix("echo rtk && rtk git status", RTK),
    `echo rtk && ${RTK} git status`,
  );
});

test("shellTokenize handles mixed escaped and normal content", () => {
  const cmd = 'rtk grep "normal" "with\\"escape" file';
  const tokens = shellTokenize(cmd);
  assert.notEqual(tokens, null);
  assert.deepEqual(tokens, ["rtk", "grep", '"normal"', '"with\\"escape"', "file"]);
});

test("backslash-escaped quote outside quotes does not start quoted context", () => {
  // Python shlex posix=False treats backslash as escape even outside quotes.
  // rtk grep foo\"bar src/ must tokenize successfully.
  const cmd = 'rtk grep foo\\"bar src/';
  const tokens = shellTokenize(cmd);
  assert.notEqual(tokens, null, "tokenize must not return null for escaped quote outside quotes");
  assert.deepEqual(tokens, ["rtk", "grep", 'foo\\"bar', "src/"]);
  // anchorRtkPrefix must still anchor rtk
  assert.equal(
    anchorRtkPrefix(cmd, RTK),
    `${RTK} grep foo\\"bar src/`,
  );
});

test("semicolon-chained rtk anchors both segments", () => {
  // RTK 0.43 compound rewrite: "rtk git status; rtk cargo test"
  // The semicolon is attached to "status" as "status;" — both rtk
  // tokens must be anchored.
  const cmd = "rtk git status; rtk cargo test";
  const result = anchorRtkPrefix(cmd, RTK);
  assert.equal(result, `${RTK} git status; ${RTK} cargo test`);
});

test("newline separator is preserved, not collapsed to space", () => {
  // "rtk git status\ncargo build" — the newline must be preserved.
  // If collapsed to a space, "cargo build" becomes an argument to
  // "status" and never executes.
  const cmd = "rtk git status\ncargo build";
  const result = anchorRtkPrefix(cmd, RTK);
  assert.ok(result.includes("\n"), "newline must be preserved");
  assert.equal(result, `${RTK} git status\ncargo build`);
});

test("newline-separated rtk commands anchor both segments", () => {
  // Newline terminates a command exactly like `;`: the rtk after the
  // newline starts a fresh segment and must be anchored (PR #2249 P1).
  const cmd = "rtk git status\nrtk cargo test";
  assert.equal(
    anchorRtkPrefix(cmd, RTK),
    `${RTK} git status\n${RTK} cargo test`,
  );
});

test("backslash-escaped semicolon is not a segment boundary", () => {
  // `\;` is an escaped argument character, not a command separator:
  // the trailing rtk is grep's argument and must stay bare (PR #2249
  // P1).  The old endsWith(";") heuristic misdetected this shape.
  const cmd = "rtk grep foo\\; rtk file";
  assert.equal(
    anchorRtkPrefix(cmd, RTK),
    `${RTK} grep foo\\; rtk file`,
  );
});

// ---- RTK v0.43 transparent-prefix protocol (PR #2249 review) --------------

test("built-in single-word transparent prefixes anchor rtk", () => {
  // rtk strips its SHELL_PREFIX_BUILTINS (noglob, command, builtin, exec,
  // nocorrect) before routing and re-prepends them in front of the rtk
  // wrapper; the rtk behind each must be anchored.
  for (const wrapper of ["noglob", "command", "builtin", "exec", "nocorrect"]) {
    assert.equal(
      anchorRtkPrefix(`${wrapper} rtk git status`, RTK),
      `${wrapper} ${RTK} git status`,
      `prefix ${wrapper}`,
    );
  }
});

test("uv run multi-word built-in prefix anchors rtk", () => {
  assert.equal(
    anchorRtkPrefix("uv run rtk pytest tests/", RTK),
    `uv run ${RTK} pytest tests/`,
  );
});

test("env assignment composes before built-in prefix", () => {
  assert.equal(
    anchorRtkPrefix("PYTHONPATH=. uv run rtk pytest tests/", RTK),
    `PYTHONPATH=. uv run ${RTK} pytest tests/`,
  );
});

test("shell wrapper nests with RTK built-in prefix", () => {
  assert.equal(
    anchorRtkPrefix("sudo noglob rtk git status", RTK),
    `sudo noglob ${RTK} git status`,
  );
});

test("configured multi-word transparent prefix anchors rtk", () => {
  // "shadowenv exec --" comes from the sandboxed rtk config.toml.
  assert.equal(
    anchorRtkPrefix("shadowenv exec -- rtk git status", RTK),
    `shadowenv exec -- ${RTK} git status`,
  );
});

test("second configured prefix anchors rtk", () => {
  assert.equal(
    anchorRtkPrefix("docker exec c1 rtk git status", RTK),
    `docker exec c1 ${RTK} git status`,
  );
});

test("env assignment between configured prefix and rtk anchors", () => {
  assert.equal(
    anchorRtkPrefix("shadowenv exec -- FOO=bar rtk git status", RTK),
    `shadowenv exec -- FOO=bar ${RTK} git status`,
  );
});

test("configured prefixes work in every segment", () => {
  assert.equal(
    anchorRtkPrefix("noglob rtk git status; shadowenv exec -- rtk cargo test", RTK),
    `noglob ${RTK} git status; shadowenv exec -- ${RTK} cargo test`,
  );
});

test("partial configured prefix does not anchor", () => {
  // Only the full configured sequence is transparent: bare `shadowenv`
  // consumes the command position, so the rtk behind it is positional.
  assert.equal(
    anchorRtkPrefix("shadowenv rtk git status", RTK),
    "shadowenv rtk git status",
  );
});

test("configured prefix never matches across a segment boundary", () => {
  // "foo bar" is configured, but the `;` boundary splits the sequence:
  // segment 2 starts with `bar`, which alone is not a prefix.
  assert.equal(
    anchorRtkPrefix("foo; bar rtk git status", RTK),
    "foo; bar rtk git status",
  );
});

test("rtk argument behind a built-in prefix lookalike stays bare", () => {
  // echo consumes the command position; noglob/rtk are its arguments.
  assert.equal(
    anchorRtkPrefix("echo noglob rtk done", RTK),
    "echo noglob rtk done",
  );
});

test("parseHooksTransparentPrefixes reads the [hooks] table", () => {
  const text =
    "[tracking]\n" +
    "enabled = true\n" +
    "[hooks]\n" +
    'exclude_commands = ["curl"]\n' +
    "transparent_prefixes = [\n" +
    '  "direnv exec .",\n' +
    "  'nix develop --command',\n" +
    ']\n' +
    "[limits]\n" +
    "grep_max_results = 200\n";
  assert.deepEqual(parseHooksTransparentPrefixes(text), [
    "direnv exec .",
    "nix develop --command",
  ]);
});

test("parseHooksTransparentPrefixes unescapes double-quoted values", () => {
  const text = '[hooks]\ntransparent_prefixes = ["a \\"b\\" c"]\n';
  assert.deepEqual(parseHooksTransparentPrefixes(text), ['a "b" c']);
});

test("parseHooksTransparentPrefixes ignores other tables", () => {
  const text = "[other]\ntransparent_prefixes = [\"nope\"]\n";
  assert.deepEqual(parseHooksTransparentPrefixes(text), []);
});

// ---- wrapper option operands must not be anchored (PR #2249 round 5) -------

test("sudo -u operand rtk is not anchored (mixed compound)", () => {
  // `sudo -u rtk true`: the word after `-u` is the username operand, not
  // a command — anchoring it would break the user switch.  Only the
  // second segment's command-position rtk is anchored.
  assert.equal(
    anchorRtkPrefix("sudo -u rtk true && rtk git status", RTK),
    `sudo -u rtk true && ${RTK} git status`,
  );
});

test("env -u operand rtk is not anchored (mixed compound)", () => {
  assert.equal(
    anchorRtkPrefix("env -u rtk && rtk git status", RTK),
    `env -u rtk && ${RTK} git status`,
  );
});

test("command -v argument rtk is not anchored (mixed compound)", () => {
  assert.equal(
    anchorRtkPrefix("command -v rtk && rtk git status", RTK),
    `command -v rtk && ${RTK} git status`,
  );
});

test("option-bearing wrapper segment alone stays untouched", () => {
  assert.equal(anchorRtkPrefix("sudo -u rtk true", RTK), "sudo -u rtk true");
  assert.equal(anchorRtkPrefix("env -u rtk", RTK), "env -u rtk");
  assert.equal(anchorRtkPrefix("command -v rtk", RTK), "command -v rtk");
});

test("operand negative holds in later segments too", () => {
  assert.equal(
    anchorRtkPrefix("rtk git status && sudo -u rtk true", RTK),
    `${RTK} git status && sudo -u rtk true`,
  );
});

test("option-less wrappers still anchor after operand fix", () => {
  // Regression guard: the operand fix must not over-close command
  // position for bare wrappers, built-ins, and configured prefixes.
  assert.equal(anchorRtkPrefix("sudo rtk git status", RTK), `sudo ${RTK} git status`);
  assert.equal(
    anchorRtkPrefix("sudo noglob rtk git status", RTK),
    `sudo noglob ${RTK} git status`,
  );
  assert.equal(
    anchorRtkPrefix("env FOO=bar rtk git status", RTK),
    `env FOO=bar ${RTK} git status`,
  );
  assert.equal(
    anchorRtkPrefix("shadowenv exec -- rtk git status", RTK),
    `shadowenv exec -- ${RTK} git status`,
  );
});
