#!/usr/bin/env python3
"""Regression tests for the cosh extension PreToolUse rewrite matcher.

cosh-ng's built-in shell tool is named ``shell`` on the wire (lowercase
snake_case), while the matcher historically only listed shell tool names
used by other agents (``Bash``, ``Shell``, ``run_shell_command``, ...).
On cosh builds without the hook-system tool-name alias table the rewrite
hook therefore never fired and rtk command rewriting was silently skipped.

The extension manifest must match cosh's real tool name directly instead
of relying on host-side aliasing: these tests pin the matcher against the
exact names cosh fires PreToolUse with, plus the negative space of cosh
tools that must never be rewritten.

Matching semantics mirror cosh-ng's ``HookSystem::matches_tool``: the
matcher is compiled as a regex and applied as an *unanchored* search
(Rust ``Regex::is_match`` ≈ ``re.search``); a matcher that fails to
compile would fall back to exact string equality, which the anchored
alternation here never triggers.
"""

import json
import re
import unittest
from pathlib import Path

MANIFEST = (
    Path(__file__).resolve().parent.parent
    / "adapters"
    / "tokenless"
    / "common"
    / "cosh-extension.json"
)

# Tool names cosh-ng fires PreToolUse with. ``shell`` is the cosh-ng
# internal name; ``run_shell_command`` is the copilot-shell standard name
# kept so both naming conventions keep reaching the rewrite hook.
MATCHING_TOOLS = [
    "shell",
    "run_shell_command",
    "Bash",
    "Shell",
    "terminal",
    "exec",
    "process",
]

# cosh-ng tools (and common foreign shapes) that must never be rewritten:
# only shell-execution tools may reach rtk.
NON_MATCHING_TOOLS = [
    "read_file",
    "write_file",
    "edit",
    "grep",
    "todo",
    "glob",
    "web_search",
    "shell_prompt",   # anchored: prefix overlap must not match
    "my_shell",       # anchored: suffix overlap must not match
    "",
]


class CoshExtensionMatcherTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        with open(MANIFEST, "r", encoding="utf-8") as f:
            cls.manifest = json.load(f)

    def _rewrite_matcher(self) -> str:
        """Return the matcher of the PreToolUse rtk rewrite hook group."""
        for group in self.manifest["hooks"]["PreToolUse"]:
            for hook in group.get("hooks", []):
                if hook.get("name") == "tokenless-rewrite":
                    matcher = group.get("matcher")
                    self.assertIsInstance(
                        matcher, str,
                        "tokenless-rewrite group must declare an explicit "
                        "matcher so non-shell tools never reach rtk",
                    )
                    self.assertTrue(
                        matcher,
                        "an empty matcher matches every tool, including "
                        "non-shell tools",
                    )
                    return matcher
        self.fail("tokenless-rewrite hook not found in PreToolUse groups")

    def test_rewrite_hook_is_registered(self):
        matcher = self._rewrite_matcher()
        self.assertTrue(matcher)

    def test_matcher_compiles_as_regex(self):
        # The pattern must be valid regex syntax in both Python's ``re``
        # and Rust's ``regex`` crate (cosh compiles it with Regex::new).
        re.compile(self._rewrite_matcher())

    def test_matcher_hits_cosh_shell_tool_name(self):
        # The regression itself: cosh-ng names its shell tool ``shell``.
        pattern = re.compile(self._rewrite_matcher())
        self.assertIsNotNone(
            pattern.search("shell"),
            "matcher must match cosh-ng's lowercase 'shell' tool name "
            "directly, without relying on host-side tool-name aliasing",
        )

    def test_matcher_hits_all_shell_family_names(self):
        pattern = re.compile(self._rewrite_matcher())
        for name in MATCHING_TOOLS:
            with self.subTest(tool=name):
                self.assertIsNotNone(
                    pattern.search(name),
                    f"matcher must match shell-family tool name {name!r}",
                )

    def test_matcher_rejects_non_shell_tools(self):
        pattern = re.compile(self._rewrite_matcher())
        for name in NON_MATCHING_TOOLS:
            with self.subTest(tool=name):
                self.assertIsNone(
                    pattern.search(name),
                    f"matcher must not match non-shell tool name {name!r}",
                )

    def test_rewrite_hook_command_unchanged(self):
        # Structural guard: the rewrite group still dispatches to
        # rewrite_hook.py, so the matcher above gates the real hook.
        for group in self.manifest["hooks"]["PreToolUse"]:
            for hook in group.get("hooks", []):
                if hook.get("name") == "tokenless-rewrite":
                    self.assertIn("rewrite_hook.py", hook["command"])
                    return
        self.fail("tokenless-rewrite hook not found in PreToolUse groups")


if __name__ == "__main__":
    unittest.main()
