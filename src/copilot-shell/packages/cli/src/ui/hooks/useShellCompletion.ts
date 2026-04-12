/**
 * @license
 * Copyright 2025 Qwen Team
 * SPDX-License-Identifier: Apache-2.0
 */

import { useCallback, useMemo, useEffect } from 'react';
import * as fs from 'node:fs';
import * as nodePath from 'node:path';
import * as os from 'node:os';
import type { Suggestion } from '../components/SuggestionsDisplay.js';
import type { TextBuffer } from '../components/shared/text-buffer.js';
import { logicalPosToOffset } from '../components/shared/text-buffer.js';
import { useCompletion } from './useCompletion.js';
import type { UseCommandCompletionReturn } from './useCommandCompletion.js';

// ─── PATH command cache ──────────────────────────────────────────────────────

const PATH_CACHE_TTL_MS = 30_000; // 30 s

interface PathCommandCache {
  commands: string[];
  timestamp: number;
}

let _pathCommandCache: PathCommandCache | null = null;

function getPathExecutables(): string[] {
  const now = Date.now();
  if (
    _pathCommandCache &&
    now - _pathCommandCache.timestamp < PATH_CACHE_TTL_MS
  ) {
    return _pathCommandCache.commands;
  }

  const pathEnv = process.env['PATH'] ?? '';
  const dirs = pathEnv.split(nodePath.delimiter).filter(Boolean);
  const seen = new Set<string>();

  for (const dir of dirs) {
    try {
      const entries = fs.readdirSync(dir);
      for (const entry of entries) {
        if (seen.has(entry)) continue;
        try {
          const fullPath = nodePath.join(dir, entry);
          const stat = fs.statSync(fullPath);
          if (stat.isFile() && (stat.mode & 0o111) !== 0) {
            seen.add(entry);
          }
        } catch {
          // ignore permission / broken symlink errors
        }
      }
    } catch {
      // ignore unreadable PATH directories
    }
  }

  const commands = Array.from(seen).sort();
  _pathCommandCache = { commands, timestamp: now };
  return commands;
}

// ─── Token parsing ───────────────────────────────────────────────────────────

interface ShellToken {
  /** Raw text of the token under/before the cursor */
  value: string;
  /** Start column (logical) of the token in the line */
  start: number;
  /** End column = cursor column */
  end: number;
  /** Whether this is the first token on the line (command position) */
  isFirstToken: boolean;
}

/**
 * Given a line and cursor column, return the token at/before the cursor.
 * Handles backslash-escaped spaces.
 */
function parseTokenAtCursor(line: string, cursorCol: number): ShellToken {
  // Walk backwards from cursor to find the start of the current token.
  let tokenStart = 0;
  for (let i = cursorCol - 1; i >= 0; i--) {
    if (line[i] === ' ') {
      // Check for escaped space: odd number of preceding backslashes
      let backslashes = 0;
      for (let j = i - 1; j >= 0 && line[j] === '\\'; j--) {
        backslashes++;
      }
      if (backslashes % 2 === 0) {
        tokenStart = i + 1;
        break;
      }
    }
  }

  const value = line.slice(tokenStart, cursorCol);
  const beforeToken = line.slice(0, tokenStart).trimStart();
  const isFirstToken = beforeToken.length === 0;

  return { value, start: tokenStart, end: cursorCol, isFirstToken };
}

// ─── Path completions ─────────────────────────────────────────────────────

/** Maximum number of suggestions to return for any completion type */
const MAX_SHELL_SUGGESTIONS = 100;

function getPathCompletions(partial: string, cwd: string): Suggestion[] {
  // Expand ~ prefix
  let expanded = partial;
  if (partial === '~' || partial.startsWith('~/')) {
    expanded = os.homedir() + partial.slice(1);
  }

  const dirPart = nodePath.dirname(expanded);
  const basePart = nodePath.basename(expanded);
  const isAbsolute = nodePath.isAbsolute(expanded);

  // Resolve the directory to scan
  let resolvedDir: string;
  if (isAbsolute) {
    resolvedDir = dirPart;
  } else if (dirPart === '.') {
    resolvedDir = cwd;
  } else {
    resolvedDir = nodePath.resolve(cwd, dirPart);
  }

  let entries: fs.Dirent[];
  try {
    entries = fs.readdirSync(resolvedDir, { withFileTypes: true });
  } catch {
    return [];
  }

  const suggestions: Suggestion[] = [];
  const showHidden = basePart.startsWith('.');

  for (const entry of entries) {
    // Hide dotfiles unless the prefix starts with '.'
    if (!showHidden && entry.name.startsWith('.')) continue;
    if (!entry.name.startsWith(basePart)) continue;

    const isDir =
      entry.isDirectory() ||
      (entry.isSymbolicLink() &&
        isSymlinkToDir(nodePath.join(resolvedDir, entry.name)));
    const displayName = isDir ? entry.name + '/' : entry.name;

    // Build the completion value (same format as what the user typed)
    let completionValue: string;
    if (dirPart === '.') {
      // No directory prefix typed – just use the entry name
      completionValue = displayName;
    } else if (partial.startsWith('~/')) {
      completionValue =
        '~/' +
        (dirPart === os.homedir()
          ? ''
          : nodePath.relative(os.homedir(), dirPart) + '/') +
        displayName;
    } else {
      completionValue = nodePath.join(dirPart, displayName);
      // Preserve trailing slash for dirs
      if (isDir && !completionValue.endsWith('/')) {
        completionValue += '/';
      }
    }

    suggestions.push({ label: displayName, value: completionValue });
    if (suggestions.length >= MAX_SHELL_SUGGESTIONS) break;
  }

  return suggestions.sort((a, b) => {
    // Directories first, then alphabetical
    const aIsDir = a.label.endsWith('/');
    const bIsDir = b.label.endsWith('/');
    if (aIsDir !== bIsDir) return aIsDir ? -1 : 1;
    return a.label.localeCompare(b.label);
  });
}

function isSymlinkToDir(p: string): boolean {
  try {
    return fs.statSync(p).isDirectory();
  } catch {
    return false;
  }
}

// ─── Command completions ──────────────────────────────────────────────────

function getCommandCompletions(prefix: string): Suggestion[] {
  if (prefix.length === 0) return [];
  const commands = getPathExecutables();
  const suggestions: Suggestion[] = [];
  for (const cmd of commands) {
    if (!cmd.startsWith(prefix)) continue;
    suggestions.push({ label: cmd, value: cmd });
    if (suggestions.length >= MAX_SHELL_SUGGESTIONS) break;
  }
  return suggestions;
}

// ─── Hook ─────────────────────────────────────────────────────────────────

/**
 * Provides Tab-completion for shell (command) mode.
 *
 * - First token on a line → complete executables from PATH
 * - Subsequent tokens     → complete file-system paths relative to `cwd`
 *
 * Completions are computed reactively whenever `enabled` is true and the
 * buffer changes.  Pass `enabled = shellModeActive && shellCompletionTriggered`
 * to implement Tab-triggered behaviour.
 */
export function useShellCompletion(
  buffer: TextBuffer,
  cwd: string,
  enabled: boolean,
  active: boolean = true,
): UseCommandCompletionReturn {
  const {
    suggestions,
    activeSuggestionIndex,
    visibleStartIndex,
    showSuggestions,
    isLoadingSuggestions,
    isPerfectMatch,
    setSuggestions,
    setShowSuggestions,
    setActiveSuggestionIndex,
    setVisibleStartIndex,
    resetCompletionState,
    navigateUp,
    navigateDown,
  } = useCompletion();

  const cursorRow = buffer.cursor[0];
  const cursorCol = buffer.cursor[1];
  const currentLine = buffer.lines[cursorRow] ?? '';

  // Parse the token at cursor.  Memoised so we only re-compute on actual changes.
  const token = useMemo(
    () => parseTokenAtCursor(currentLine, cursorCol),
    [currentLine, cursorCol],
  );

  // Compute new suggestions whenever the token changes.
  const newSuggestions = useMemo((): Suggestion[] => {
    if (!enabled || !active) return [];
    if (token.value === '' && token.isFirstToken) return [];

    if (token.isFirstToken) {
      return getCommandCompletions(token.value);
    }
    return getPathCompletions(token.value, cwd);
  }, [enabled, active, token, cwd]);

  // Sync computed suggestions into completion state.
  useEffect(() => {
    if (!enabled || !active) {
      resetCompletionState();
      return;
    }
    setSuggestions(newSuggestions);
    setShowSuggestions(newSuggestions.length > 0);
    setActiveSuggestionIndex(newSuggestions.length > 0 ? 0 : -1);
    setVisibleStartIndex(0);
  }, [
    enabled,
    active,
    newSuggestions,
    setSuggestions,
    setShowSuggestions,
    setActiveSuggestionIndex,
    setVisibleStartIndex,
    resetCompletionState,
  ]);

  // Apply the selected suggestion to the buffer.
  const handleAutocomplete = useCallback(
    (indexToUse: number) => {
      if (indexToUse < 0 || indexToUse >= suggestions.length) return;
      const suggestion = suggestions[indexToUse].value;

      buffer.replaceRangeByOffset(
        logicalPosToOffset(buffer.lines, cursorRow, token.start),
        logicalPosToOffset(buffer.lines, cursorRow, token.end),
        suggestion,
      );
    },
    [buffer, cursorRow, token, suggestions],
  );

  return {
    suggestions,
    activeSuggestionIndex,
    visibleStartIndex,
    showSuggestions,
    isLoadingSuggestions,
    isPerfectMatch,
    setActiveSuggestionIndex,
    setShowSuggestions,
    resetCompletionState,
    navigateUp,
    navigateDown,
    handleAutocomplete,
  };
}
