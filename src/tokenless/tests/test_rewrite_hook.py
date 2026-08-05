#!/usr/bin/env python3
"""Regression tests for rewrite_hook.py rtk-prefix anchoring.

rtk emits rewritten commands with a bare `rtk` prefix, which only resolves
when the shell executing the tool call has the rtk location on its PATH.
Agent runtimes with a trimmed PATH (e.g. IDE tool environments without
~/.local/bin) would fail every rewritten command with exit 127. The hook
must anchor the rewrite to the resolved absolute rtk binary so the command
is self-contained — without touching quoting, globs, or any other part of
the command text.

The tests stage a fake rtk/tokenless pair in the fallback layout under a
sandboxed HOME and run the hook with a PATH that deliberately lacks the
rtk location — the exact shape of the affected environments.
"""

import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HOOK = (
    Path(__file__).resolve().parent.parent
    / "adapters"
    / "tokenless"
    / "common"
    / "hooks"
    / "rewrite_hook.py"
)

# Mirrors real rtk: --version answers; rewrite maps each input command to
# the shape real rtk would emit, with a bare `rtk` prefix at wrapper
# positions (including after `sudo`, env assignments, and connectives).
FAKE_RTK = """#!/usr/bin/env python3
import sys

REWRITES = {
    "grep foo bar && git status": "rtk grep --cached foo && rtk git status",
    "grep foo bar": "rtk grep foo bar",
    "sudo git status": "sudo rtk git status",
    "RUST_BACKTRACE=1 cargo test": "RUST_BACKTRACE=1 rtk cargo test",
    "git status & grep foo": "git status & rtk grep foo",
    "grep -E 'foo|rtk bar' src/": "rtk grep -E 'foo|rtk bar' src/",
    "grep foo *.txt": "rtk grep foo *.txt",
    "grep foo #include src/": "rtk grep foo #include src/",
    "git log 2>&1 | head": "rtk git log 2>&1 | rtk head",
    "git status 2>/dev/null": "rtk git status 2>/dev/null",
    "echo $(date)": "rtk echo $(date)",
}

if len(sys.argv) > 1 and sys.argv[1] == "--version":
    print("rtk 0.43.0")
    sys.exit(0)
if len(sys.argv) > 2 and sys.argv[1] == "rewrite" and sys.argv[2] in REWRITES:
    print(REWRITES[sys.argv[2]])
    sys.exit(0)
sys.exit(1)
"""

FAKE_TOKENLESS = """#!/bin/sh
echo "tokenless 0.7.3"
"""


def _write_exec(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


class RewriteAnchorTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.home = Path(self.tmp.name)
        # Fallback layout resolve_binary probes when PATH lookup fails.
        share = self.home / ".local" / "share" / "anolisa" / "tokenless"
        _write_exec(share / "rtk", FAKE_RTK)
        _write_exec(share / "tokenless", FAKE_TOKENLESS)
        self.rtk = str(share / "rtk")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def _rewrite(self, command: str) -> str:
        env = os.environ.copy()
        env["HOME"] = str(self.home)
        # The affected shape: PATH lacks the rtk location entirely.
        env["PATH"] = "/usr/bin:/bin"
        env.pop("TOKENLESS_AGENT_ID", None)
        proc = subprocess.run(
            [sys.executable, str(HOOK)],
            input=json.dumps({"tool_name": "Bash", "tool_input": {"command": command}}),
            capture_output=True,
            text=True,
            env=env,
            timeout=15,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = json.loads(proc.stdout or "{}")
        rewritten = (
            out.get("hookSpecificOutput", {}).get("tool_input", {}).get("command", "")
        )
        self.assertTrue(rewritten, f"hook did not rewrite: {out}")
        return rewritten

    def test_rewrite_anchored_to_resolved_rtk_path(self) -> None:
        command = self._rewrite("grep foo bar && git status")
        # Every segment starts with the absolute rtk binary, not bare `rtk`.
        self.assertEqual(command, f"{self.rtk} grep --cached foo && {self.rtk} git status")
        # Self-contained: the resolved first word is an executable file even
        # though PATH lacks its directory.
        first_word = command.split(" ", 1)[0]
        self.assertTrue(os.path.isfile(first_word), command)
        self.assertTrue(os.access(first_word, os.X_OK), command)

    def test_updated_input_matches_tool_input(self) -> None:
        env = os.environ.copy()
        env["HOME"] = str(self.home)
        env["PATH"] = "/usr/bin:/bin"
        env.pop("TOKENLESS_AGENT_ID", None)
        proc = subprocess.run(
            [sys.executable, str(HOOK)],
            input=json.dumps({"tool_name": "Bash", "tool_input": {"command": "grep foo bar"}}),
            capture_output=True,
            text=True,
            env=env,
            timeout=15,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        hook_out = json.loads(proc.stdout).get("hookSpecificOutput", {})
        command = hook_out.get("tool_input", {}).get("command", "")
        self.assertEqual(command, f"{self.rtk} grep foo bar")
        self.assertEqual(command, hook_out.get("updatedInput", {}).get("command", ""))

    def test_anchor_after_wrapper(self) -> None:
        self.assertEqual(
            self._rewrite("sudo git status"),
            f"sudo {self.rtk} git status",
        )

    def test_anchor_after_env_assignment(self) -> None:
        self.assertEqual(
            self._rewrite("RUST_BACKTRACE=1 cargo test"),
            f"RUST_BACKTRACE=1 {self.rtk} cargo test",
        )

    def test_anchor_after_single_ampersand(self) -> None:
        self.assertEqual(
            self._rewrite("git status & grep foo"),
            f"git status & {self.rtk} grep foo",
        )

    def test_quoted_rtk_pattern_untouched(self) -> None:
        # Only the leading wrapper is anchored; the `rtk` inside the quoted
        # regex pattern must survive byte-for-byte.
        self.assertEqual(
            self._rewrite("grep -E 'foo|rtk bar' src/"),
            f"{self.rtk} grep -E 'foo|rtk bar' src/",
        )

    def test_unquoted_glob_preserved(self) -> None:
        # The glob must stay an unquoted glob — re-quoting tokens would
        # produce '*.txt' and neuter expansion.
        self.assertEqual(
            self._rewrite("grep foo *.txt"),
            f"{self.rtk} grep foo *.txt",
        )

    def test_hash_argument_preserved(self) -> None:
        # `#` must not be treated as a comment starter — the argument and
        # everything after it stays in the command.
        self.assertEqual(
            self._rewrite("grep foo #include src/"),
            f"{self.rtk} grep foo #include src/",
        )

    def test_fd_merging_preserved(self) -> None:
        # `2>&1` must stay one unsplit token — splitting it into
        # `2 >& 1` would turn `2` into an argument and break the merge.
        self.assertEqual(
            self._rewrite("git log 2>&1 | head"),
            f"{self.rtk} git log 2>&1 | {self.rtk} head",
        )

    def test_fd_redirection_preserved(self) -> None:
        # `2>/dev/null` must stay attached — `2 > /dev/null` would make `2`
        # an argument and redirect stdout instead of stderr.
        self.assertEqual(
            self._rewrite("git status 2>/dev/null"),
            f"{self.rtk} git status 2>/dev/null",
        )

    def test_command_substitution_preserved(self) -> None:
        # `$(...)` must not be split into `$ ( ... )`, which would destroy
        # the substitution.
        self.assertEqual(
            self._rewrite("echo $(date)"),
            f"{self.rtk} echo $(date)",
        )


if __name__ == "__main__":
    unittest.main()
