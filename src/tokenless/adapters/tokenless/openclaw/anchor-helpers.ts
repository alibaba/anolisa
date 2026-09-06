// Shared shell-tokenization and RTK anchor helpers.
// Used by the OpenClaw plugin (index.ts) and its unit tests.

import { readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join } from "node:path";

export const SEGMENT_OPS = new Set(["&&", "||", ";", "|", "&"]);

export function isEnvAssignment(token: string): boolean {
  const eq = token.indexOf("=");
  if (eq <= 0) return false;
  const name = token.slice(0, eq);
  if (!/^[A-Za-z_]/.test(name)) return false;
  return /^[A-Za-z0-9_]+$/.test(name);
}

/**
 * Tokenize a shell command string without a shell, preserving quoted strings.
 *
 * Splits on spaces and tabs (newlines stay inside the enclosing token —
 * use shellWordSpans when segment boundaries matter) while keeping
 * single- and double-quoted spans intact.
 * Handles backslash-escaped characters both outside and inside double quotes
 * (mirrors Python shlex with ``posix=False``).  Does **not** recognize fd
 * redirections, globs, or command substitutions as special tokens — they pass
 * through as ordinary characters within their enclosing whitespace-delimited
 * token.
 *
 * Returns ``null`` when the input contains an unmatched quote.
 */
export function shellTokenize(cmd: string): string[] | null {
  const tokens: string[] = [];
  let i = 0;
  const n = cmd.length;
  while (i < n) {
    while (i < n && (cmd[i] === " " || cmd[i] === "\t")) i++;
    if (i >= n) break;
    let tok = "";
    while (i < n && cmd[i] !== " " && cmd[i] !== "\t") {
      const ch = cmd[i];
      if (ch === "'") {
        const end = cmd.indexOf("'", i + 1);
        if (end === -1) return null;
        tok += cmd.slice(i, end + 1);
        i = end + 1;
      } else if (ch === '"') {
        tok += ch;
        i++;
        while (i < n && cmd[i] !== '"') {
          if (cmd[i] === "\\" && i + 1 < n) {
            tok += cmd[i] + cmd[i + 1];
            i += 2;
          } else {
            tok += cmd[i];
            i++;
          }
        }
        if (i >= n) return null;
        tok += cmd[i];
        i++;
      } else if (ch === "\\" && i + 1 < n) {
        tok += cmd[i] + cmd[i + 1];
        i += 2;
      } else {
        tok += ch;
        i++;
      }
    }
    if (tok) tokens.push(tok);
  }
  return tokens;
}

/** Characters that break a word: whitespace plus segment metacharacters. */
const WORD_BREAK_CHARS = new Set([" ", "\t", "\n", "\r", ";", "|", "&"]);

/** Single-char shell metacharacters that can separate segments. */
const SEGMENT_META_CHARS = new Set([";", "|", "&"]);

/** Gap characters that start a new segment (metachars plus newline). */
const SEGMENT_BOUNDARY_CHARS = new Set([";", "|", "&", "\n"]);

/** Transparent wrappers that delegate to the real command. */
const TRANSPARENT_WRAPPERS = new Set([
  "sudo", "doas", "pkexec",
  "env", "nice", "nohup", "stdbuf", "time", "timeout",
]);

/**
 * RTK's own built-in transparent prefixes (rtk >= 0.43 contract, see rtk
 * registry SHELL_PREFIX_BUILTINS / ROUTABLE_WRAPPER_PREFIXES): rtk strips
 * them before routing and re-prepends them in front of the `rtk` wrapper
 * it inserts, so its rewrite output can start a segment with e.g.
 * `noglob rtk git status` or `uv run rtk pytest tests/`.  Anchoring must
 * treat them exactly like the shell wrappers above.
 */
const RTK_BUILTIN_TRANSPARENT_PREFIXES = [
  "uv run",
  "noglob", "command", "builtin", "exec", "nocorrect",
];

/**
 * Return rtk's global config.toml path, mirroring rtk's own lookup
 * (`dirs::config_dir()/rtk/config.toml`): `~/Library/Application Support`
 * on macOS, `%APPDATA%` on Windows, and `$XDG_CONFIG_HOME` (fallback
 * `~/.config`) elsewhere.  The plugin reads the same file so anchoring
 * knows the user's configured `transparent_prefixes`.
 */
function rtkConfigPath(): string | null {
  const home = homedir();
  if (!home) return null;
  if (process.platform === "darwin") {
    return join(home, "Library", "Application Support", "rtk", "config.toml");
  }
  if (process.platform === "win32") {
    const appData = process.env.APPDATA;
    return appData ? join(appData, "rtk", "config.toml") : null;
  }
  const xdg = process.env.XDG_CONFIG_HOME ?? "";
  const base = isAbsolute(xdg) ? xdg : join(home, ".config");
  return join(base, "rtk", "config.toml");
}

/** Body of the `[hooks]` table (up to the next table header). */
function hooksTableBody(text: string): string {
  const body: string[] = [];
  let inHooks = false;
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (trimmed.startsWith("[")) {
      inHooks = /^\[hooks\][ \t]*(?:[#;].*)?$/.test(trimmed);
      continue;
    }
    if (inHooks) body.push(line);
  }
  return body.join("\n");
}

/**
 * Best-effort extraction of `[hooks].transparent_prefixes` string values.
 * Handles multi-line arrays, double-quoted values with `\"` / `\\`
 * escapes, and single-quoted literal values — the shapes rtk's own TOML
 * writer and hand-edited configs produce.
 */
export function parseHooksTransparentPrefixes(text: string): string[] {
  const section = hooksTableBody(text);
  const match = /transparent_prefixes[ \t]*=[ \t]*\[([^\]]*)\]/s.exec(section);
  if (!match) return [];
  const values: string[] = [];
  const literal = /"((?:[^"\\]|\\.)*)"|'([^']*)'/g;
  let sm: RegExpExecArray | null;
  while ((sm = literal.exec(match[1])) !== null) {
    if (sm[1] !== undefined) {
      values.push(sm[1].replace(/\\(["\\])/g, "$1"));
    } else {
      values.push(sm[2]);
    }
  }
  return values;
}

// Configured-prefix cache keyed by `path:mtime` so the long-lived plugin
// notices config edits without re-parsing on every tool call.
let configuredCache: { key: string; prefixes: string[] } | null = null;

/** User-configured rtk transparent prefixes (may be empty). */
export function loadRtkTransparentPrefixes(): string[] {
  const configPath = rtkConfigPath();
  if (!configPath) return [];
  let mtimeMs: number;
  try {
    mtimeMs = statSync(configPath).mtimeMs;
  } catch {
    return [];
  }
  const key = `${configPath}:${mtimeMs}`;
  if (configuredCache && configuredCache.key === key) {
    return configuredCache.prefixes;
  }
  let prefixes: string[] = [];
  try {
    // Mirror rtk's normalize_transparent_prefixes: trim, drop empties, dedup.
    prefixes = [...new Set(
      parseHooksTransparentPrefixes(readFileSync(configPath, "utf8"))
        .map((p) => p.trim())
        .filter((p) => p.length > 0),
    )];
  } catch {
    prefixes = [];
  }
  configuredCache = { key, prefixes };
  return prefixes;
}

let wordListsCache: { configured: string[]; lists: string[][] } | null = null;

/**
 * All transparent prefixes as word lists, longest match first: shell
 * wrappers plus RTK built-ins plus configured `transparent_prefixes`,
 * mirroring rtk's longest-prefix-first matching so e.g. a configured
 * `docker exec mycontainer` wins over a bare `docker`.
 */
function transparentPrefixWordLists(): string[][] {
  const configured = loadRtkTransparentPrefixes();
  if (
    wordListsCache &&
    wordListsCache.configured.length === configured.length &&
    wordListsCache.configured.every((p, idx) => p === configured[idx])
  ) {
    return wordListsCache.lists;
  }
  const merged = new Set<string>(TRANSPARENT_WRAPPERS);
  for (const prefix of RTK_BUILTIN_TRANSPARENT_PREFIXES) merged.add(prefix);
  for (const prefix of configured) {
    const trimmed = prefix.trim();
    if (trimmed) merged.add(trimmed);
  }
  const lists = [...merged]
    .map((prefix) => prefix.split(/\s+/))
    .sort(
      (a, b) =>
        b.length - a.length ||
        b.join("").length - a.join("").length,
    );
  wordListsCache = { configured, lists };
  return lists;
}

/**
 * Return how many word spans starting at `index` form a transparent
 * prefix.  Tries every known prefix word list (longest first) against the
 * raw word surfaces and returns the span count of the first (longest)
 * match, or 0 when none matches.  A match never crosses a segment
 * boundary: every span after the first must belong to the same segment.
 */
function matchTransparentPrefix(
  rewritten: string,
  spans: WordSpan[],
  index: number,
  prefixWordLists: string[][],
): number {
  for (const words of prefixWordLists) {
    const count = words.length;
    if (count === 0 || index + count > spans.length) continue;
    let matched = true;
    for (let offset = 0; offset < count; offset++) {
      const span = spans[index + offset];
      if (offset > 0 && span.newSegment) {
        matched = false;
        break;
      }
      if (rewritten.slice(span.start, span.end) !== words[offset]) {
        matched = false;
        break;
      }
    }
    if (matched) return count;
  }
  return 0;
}

/**
 * Return true when the metacharacter at `cmd[i]` really separates
 * segments.  `&` is not a separator when it is part of an fd
 * redirection — `2>&1` / `>&2` (fd duplication) or bash `&> file`.
 */
function isSegmentBoundary(cmd: string, i: number): boolean {
  if (cmd[i] !== "&") return true;
  if (i > 0 && cmd[i - 1] === ">") return false;
  if (i + 1 < cmd.length && cmd[i + 1] === ">") return false;
  return true;
}

export interface WordSpan {
  start: number;
  end: number;
  newSegment: boolean;
}

/**
 * Lex a command into word spans, tracking real segment boundaries.
 *
 * Returns spans where `cmd.slice(start, end)` is a maximal shell word and
 * `newSegment` marks that the word starts a fresh command segment: it is
 * the first word, or at least one real separator sits between it and the
 * previous word.  Real separators are unquoted, unescaped `;` / `|` / `&`
 * metacharacters and newlines — exactly the boundaries a shell honors.
 * Backslash-escaped metacharacters (`\;`) and metacharacters inside
 * quotes are ordinary word material, never boundaries, and `&` inside fd
 * redirections (`2>&1`, `&>`) stays in its word.  Quoted spans remain
 * glued into the surrounding word and keep their original surface.
 * Returns `null` when a quote is never closed.
 */
export function shellWordSpans(cmd: string): WordSpan[] | null {
  const spans: WordSpan[] = [];
  let i = 0;
  const n = cmd.length;
  let prevEnd = -1;
  while (i < n) {
    const c = cmd[i];
    if (
      WORD_BREAK_CHARS.has(c) &&
      (!SEGMENT_META_CHARS.has(c) || isSegmentBoundary(cmd, i))
    ) {
      i++;
      continue;
    }
    // Start of a word.
    const start = i;
    while (i < n) {
      const ch = cmd[i];
      if (WORD_BREAK_CHARS.has(ch)) {
        if (SEGMENT_META_CHARS.has(ch) && !isSegmentBoundary(cmd, i)) {
          // fd redirection (& in 2>&1 / &>) stays inside the word.
          i++;
          continue;
        }
        break;
      }
      if (ch === "'") {
        const close = cmd.indexOf("'", i + 1);
        if (close === -1) return null;
        i = close + 1;
        continue;
      }
      if (ch === '"') {
        i++;
        let closed = false;
        while (i < n) {
          if (cmd[i] === '"') {
            closed = true;
            i++;
            break;
          }
          if (cmd[i] === "\\" && i + 1 < n && '"$`\\'.includes(cmd[i + 1])) {
            i += 2;
            continue;
          }
          i++;
        }
        if (!closed) return null;
        continue;
      }
      if (ch === "\\" && i + 1 < n) {
        i += 2;
        continue;
      }
      i++;
    }
    let newSegment: boolean;
    if (prevEnd < 0) {
      newSegment = true;
    } else {
      newSegment = false;
      for (let g = prevEnd; g < start; g++) {
        if (SEGMENT_BOUNDARY_CHARS.has(cmd[g])) {
          newSegment = true;
          break;
        }
      }
    }
    spans.push({ start, end: i, newSegment });
    prevEnd = i;
  }
  return spans;
}

/**
 * Replace bare `rtk` wrapper tokens with the resolved absolute binary path.
 *
 * Ports the Python `_anchor_rtk_prefix` logic: swaps the first unquoted
 * `rtk` word of each pipeline segment *in command position* — at segment
 * start or right after a real segment boundary: an unquoted, unescaped
 * `&&` / `||` / `;` / `|` / `&` connective or a newline.  Command
 * position survives leading env assignments, transparent wrappers like
 * `sudo`, and RTK's transparent-prefix protocol: rtk strips its built-in
 * prefixes (`uv run`, `noglob`,
 * `command`, `builtin`, `exec`, `nocorrect`) plus the user-configured
 * multi-word `[hooks].transparent_prefixes` before routing and re-prepends
 * them in front of the `rtk` wrapper it inserts, so outputs like
 * `noglob rtk git status` or `shadowenv exec -- rtk git status` must
 * anchor the `rtk` behind the full prefix sequence.  Prefixes are matched
 * as whole word sequences, longest first, never crossing a segment
 * boundary.  Once a segment's command position is consumed by any other
 * word — a plain command like `echo`, or a wrapper option such as
 * `sudo -u` / `command -v` whose operand or query argument must never be
 * anchored — a later bare `rtk` in that segment is treated as a
 * positional argument and left untouched.  Boundaries are detected from
 * the raw command text with full quote and escape awareness
 * (shellWordSpans), so a backslash-escaped operator (`grep foo\; rtk
 * file` — the `\;` is an argument, not a separator) never starts a new
 * segment and operators inside quotes never do either, while a newline
 * between two commands starts one exactly like `;`.  Words keep their
 * original surface: quoted patterns, globs, fd redirections, and command
 * substitutions are never modified, and everything outside the replaced
 * `rtk` words is copied through byte-for-byte (including all whitespace
 * and newlines).  Unparseable input is returned untouched.
 */
export function anchorRtkPrefix(rewritten: string, resolvedRtkPath: string): string {
  const spans = shellWordSpans(rewritten);
  if (!spans) return rewritten;

  const needsQuote = /[ \t'"\\$`!*?{}[\]|;&<>()#]/.test(resolvedRtkPath);
  const quoted = needsQuote
    ? `'${resolvedRtkPath.replace(/'/g, "'\\''")}'`
    : resolvedRtkPath;
  const prefixWordLists = transparentPrefixWordLists();

  let out = "";
  let pos = 0;
  let index = 0;
  let commandPending = true;
  while (index < spans.length) {
    const { start, end, newSegment } = spans[index];
    if (newSegment) {
      commandPending = true;
    }
    if (!commandPending) {
      index++;
      continue;
    }
    const word = rewritten.slice(start, end);
    if (isEnvAssignment(word)) {
      index++;
      continue;
    }
    const matched = matchTransparentPrefix(rewritten, spans, index, prefixWordLists);
    if (matched > 0) {
      index += matched;
      continue;
    }
    if (word === "rtk") {
      out += rewritten.slice(pos, start) + quoted;
      pos = end;
      commandPending = false;
    } else {
      // Any other word consumes the command position — including wrapper
      // options (`-u`, `-v`, ...) whose operands (e.g. the username in
      // `sudo -u rtk true`) or query arguments (`command -v rtk`) must
      // never be anchored.  rtk itself never composes a dash option in
      // front of the rtk wrapper it inserts, so this only affects
      // passed-through user literals.
      commandPending = false;
    }
    index++;
  }
  if (pos === 0) return rewritten;
  out += rewritten.slice(pos);
  return out;
}
