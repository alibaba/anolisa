/**
 * Line-window slicing for the memory corpus supplement.
 *
 * OpenClaw's `MemoryCorpusGetResult` (plugin-sdk `plugins/memory-state`)
 * requires a supplement to report the window it actually returned: `fromLine`
 * is the 1-based first line and `lineCount` the number of lines in `content`.
 * The host spreads that result straight into the `memory_read` response, so
 * these two fields are what the model sees — and the host's builtin reader
 * (`buildMemoryReadResult`) clamps the same two inputs, which means a
 * supplement that echoes the *requested* window instead of the *returned* one
 * makes the two corpora disagree about what "from line 0, 10 lines" produced.
 *
 * Pure and standalone on purpose: the supplement itself needs a live plugin
 * API and MCP client, this does not, so the window arithmetic is testable.
 */

export type CorpusWindow = {
  /** The selected lines, joined with "\n". */
  content: string;
  /** 1-based first line of `content`; always >= 1. */
  fromLine: number;
  /** Number of lines in `content`; 0 when the window starts past the end. */
  lineCount: number;
};

/** 1-based start line, clamped exactly like the host's builtin reader.
 *  Without the clamp a 0 or negative request reaches `Array.slice` as a
 *  negative index and silently returns the document's *tail*, and a non-finite
 *  value propagates NaN into the reported window. */
function normalizeFromLine(fromLine?: number): number {
  if (fromLine === undefined || !Number.isFinite(fromLine)) return 1;
  return Math.max(1, Math.trunc(fromLine));
}

/** Requested line count, or undefined for "to the end of the document".
 *  Clamped to >= 1 for the same reason: to the host's builtin reader a
 *  non-positive count means one line, not the whole file — and this plugin's
 *  entire job is keeping payloads out of the context window. */
function normalizeLineCount(lineCount?: number): number | undefined {
  if (lineCount === undefined || !Number.isFinite(lineCount)) return undefined;
  return Math.max(1, Math.trunc(lineCount));
}

/**
 * Select a line window from a document body.
 *
 * `text` is split on "\n" and the selected lines are re-joined, so a document
 * ending in a newline carries one trailing empty line — the same accounting the
 * host's builtin reader uses, which keeps `fromLine + lineCount` pagination
 * consistent across corpora.
 */
export function sliceCorpusWindow(
  text: string,
  fromLine?: number,
  lineCount?: number,
): CorpusWindow {
  const lines = text.split("\n");
  const start = normalizeFromLine(fromLine);
  const requested = normalizeLineCount(lineCount);
  const end = requested === undefined ? lines.length : start - 1 + requested;
  const selected = lines.slice(start - 1, end);
  return {
    content: selected.join("\n"),
    fromLine: start,
    lineCount: selected.length,
  };
}
