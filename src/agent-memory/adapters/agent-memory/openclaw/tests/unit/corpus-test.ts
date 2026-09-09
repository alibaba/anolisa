/**
 * Unit tests for the corpus-get line window (src/corpus.ts).
 *
 * The window the memory corpus supplement reports back to OpenClaw has to
 * match the host's own builtin reader: `MemoryCorpusGetResult` requires
 * `fromLine` / `lineCount`, and the host clamps both inputs, so an unclamped
 * supplement would answer the same `memory_read` differently depending on
 * which corpus served it.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { sliceCorpusWindow } from "../../src/corpus.js";

// Five lines, no trailing newline.
const DOC = "one\ntwo\nthree\nfour\nfive";

describe("sliceCorpusWindow", () => {
  it("returns the whole document when no window is requested", () => {
    assert.deepEqual(sliceCorpusWindow(DOC), {
      content: DOC,
      fromLine: 1,
      lineCount: 5,
    });
  });

  it("returns the requested window and reports it", () => {
    assert.deepEqual(sliceCorpusWindow(DOC, 2, 2), {
      content: "two\nthree",
      fromLine: 2,
      lineCount: 2,
    });
  });

  it("honours an open-ended window from a given line", () => {
    assert.deepEqual(sliceCorpusWindow(DOC, 4), {
      content: "four\nfive",
      fromLine: 4,
      lineCount: 2,
    });
  });

  it("clamps a window that runs past the end of the document", () => {
    assert.deepEqual(sliceCorpusWindow(DOC, 4, 10), {
      content: "four\nfive",
      fromLine: 4,
      lineCount: 2,
    });
  });

  it("reports an empty window when fromLine is past the end", () => {
    assert.deepEqual(sliceCorpusWindow(DOC, 6, 2), {
      content: "",
      fromLine: 6,
      lineCount: 0,
    });
  });

  it("clamps fromLine 0 to the first line instead of slicing from the end", () => {
    // Regression: (0 - 1) reached Array.slice as -1 and returned the tail.
    assert.deepEqual(sliceCorpusWindow(DOC, 0, 2), {
      content: "one\ntwo",
      fromLine: 1,
      lineCount: 2,
    });
  });

  it("clamps a negative fromLine the same way", () => {
    assert.deepEqual(sliceCorpusWindow(DOC, -3, 1), {
      content: "one",
      fromLine: 1,
      lineCount: 1,
    });
  });

  it("clamps lineCount 0 to one line, matching the builtin reader", () => {
    // Regression: 0 is falsy, so it used to select the rest of the document.
    assert.deepEqual(sliceCorpusWindow(DOC, 2, 0), {
      content: "two",
      fromLine: 2,
      lineCount: 1,
    });
  });

  it("clamps a negative lineCount the same way", () => {
    assert.deepEqual(sliceCorpusWindow(DOC, 3, -5), {
      content: "three",
      fromLine: 3,
      lineCount: 1,
    });
  });

  it("truncates fractional bounds instead of reporting them", () => {
    assert.deepEqual(sliceCorpusWindow(DOC, 2.7, 2.9), {
      content: "two\nthree",
      fromLine: 2,
      lineCount: 2,
    });
  });

  it("treats non-finite bounds as no window", () => {
    assert.deepEqual(sliceCorpusWindow(DOC, Number.NaN, Number.POSITIVE_INFINITY), {
      content: DOC,
      fromLine: 1,
      lineCount: 5,
    });
  });

  it("counts a trailing newline as one empty line", () => {
    assert.deepEqual(sliceCorpusWindow("a\nb\n"), {
      content: "a\nb\n",
      fromLine: 1,
      lineCount: 3,
    });
  });

  it("handles an empty document", () => {
    assert.deepEqual(sliceCorpusWindow(""), {
      content: "",
      fromLine: 1,
      lineCount: 1,
    });
  });
});
