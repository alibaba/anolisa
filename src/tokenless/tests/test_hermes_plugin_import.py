#!/usr/bin/env python3
"""Regression tests for the Hermes plugin hook_utils resolution.

Covers the review findings on PR #2058:
- P1-a: the hooks directory itself, its parent, and hook_utils.py must all
  be rejected when world-writable or foreign-owned (not just the parent).
- P1-b: copy-installs must honor XDG_DATA_HOME (anolisa FsLayout::user
  prefers it over ~/.local/share).
- P1-c: an existing-but-incomplete high-priority candidate must not stop
  the search; later valid candidates are still tried.
- P2: candidate list contains no empty placeholders; _validate_hooks_dir
  rejects relative/empty paths; the ImportError mentions trust-policy
  rejections, not just "missing".
"""

import importlib.util
import os
import shutil
import sys
import tempfile
import unittest

_REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
_PLUGIN_SRC = os.path.join(_REPO_ROOT, "adapters", "tokenless", "hermes", "__init__.py")
_HOOKS_SRC = os.path.join(_REPO_ROOT, "adapters", "tokenless", "common", "hooks")


def _load_plugin(path: str, name: str):
    """Load a copy of the Hermes plugin module under a unique name."""
    # Drop any previously imported hook_utils so each load re-resolves it.
    sys.modules.pop("hook_utils", None)
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    pre_path = sys.path[:]
    try:
        spec.loader.exec_module(module)
    finally:
        sys.path[:] = pre_path
    return module


def _make_hooks_dir(base: str) -> str:
    """Create a complete, trusted hooks dir under base and return its path."""
    hooks = os.path.join(base, "anolisa", "adapters", "tokenless", "common", "hooks")
    os.makedirs(hooks, mode=0o755)
    for fname in ("hook_utils.py", "tool_categories.json"):
        shutil.copy(os.path.join(_HOOKS_SRC, fname), hooks)
    os.chmod(hooks, 0o755)
    return hooks


class ValidateHooksDirTest(unittest.TestCase):
    """Unit tests for _validate_hooks_dir (loaded from the source tree)."""

    @classmethod
    def setUpClass(cls):
        # Source-tree import: the relative candidate resolves, so loading
        # the real plugin file always succeeds here.
        cls.plugin = _load_plugin(_PLUGIN_SRC, "hermes_plugin_srctree")

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hermes-hooks-test-")
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)

    def test_rejects_empty_and_relative_paths(self):
        self.assertIsNotNone(self.plugin._validate_hooks_dir(""))
        self.assertIsNotNone(self.plugin._validate_hooks_dir("relative/hooks"))

    def test_rejects_missing_directory(self):
        reason = self.plugin._validate_hooks_dir(os.path.join(self.tmp, "nope"))
        self.assertIn("does not exist", reason)

    def test_rejects_incomplete_dir_without_hook_utils(self):
        # P1-c: uninstall residue — dir exists but hook_utils.py is gone.
        empty = os.path.join(self.tmp, "hooks")
        os.makedirs(empty)
        reason = self.plugin._validate_hooks_dir(empty)
        self.assertIn("hook_utils.py missing", reason)

    def test_accepts_trusted_complete_dir(self):
        hooks = _make_hooks_dir(self.tmp)
        self.assertIsNone(self.plugin._validate_hooks_dir(hooks))

    def test_rejects_world_writable_hooks_dir(self):
        # P1-a: the hooks dir itself is world-writable.
        hooks = _make_hooks_dir(self.tmp)
        os.chmod(hooks, 0o777)
        reason = self.plugin._validate_hooks_dir(hooks)
        self.assertIn("world-writable", reason)

    def test_rejects_world_writable_hook_utils_file(self):
        # P1-a: hook_utils.py itself is world-writable (0666).
        hooks = _make_hooks_dir(self.tmp)
        os.chmod(os.path.join(hooks, "hook_utils.py"), 0o666)
        reason = self.plugin._validate_hooks_dir(hooks)
        self.assertIn("world-writable", reason)

    def test_rejects_world_writable_parent_dir(self):
        hooks = _make_hooks_dir(self.tmp)
        os.chmod(os.path.dirname(hooks), 0o777)
        reason = self.plugin._validate_hooks_dir(hooks)
        self.assertIn("world-writable", reason)

    def test_candidate_list_has_no_empty_entries(self):
        # P2: no "" placeholder elements in the candidate list.
        for candidate in self.plugin._HOOK_UTILS_CANDIDATES:
            self.assertTrue(candidate, "empty candidate in _HOOK_UTILS_CANDIDATES")
            self.assertTrue(os.path.isabs(candidate) or candidate.startswith(self.plugin._HERE))


class CopyInstallResolutionTest(unittest.TestCase):
    """End-to-end: plugin copied to a bare dir (anolisa driver behavior)."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hermes-copy-test-")
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        plugin_dir = os.path.join(self.tmp, "plugins", "tokenless")
        os.makedirs(plugin_dir)
        shutil.copy(_PLUGIN_SRC, plugin_dir)
        self.plugin_copy = os.path.join(plugin_dir, "__init__.py")
        self._saved_xdg = os.environ.get("XDG_DATA_HOME")

    def tearDown(self):
        if self._saved_xdg is None:
            os.environ.pop("XDG_DATA_HOME", None)
        else:
            os.environ["XDG_DATA_HOME"] = self._saved_xdg

    def test_resolves_via_xdg_data_home(self):
        # P1-b: XDG_DATA_HOME layout must be honored for copy-installs.
        xdg = os.path.join(self.tmp, "xdg-data")
        hooks = _make_hooks_dir(xdg)
        os.environ["XDG_DATA_HOME"] = xdg
        plugin = _load_plugin(self.plugin_copy, "hermes_plugin_xdg")
        self.assertEqual(plugin._HOOK_UTILS_RESOLVED, os.path.realpath(hooks))

    def test_incomplete_xdg_candidate_does_not_mask_later_ones(self):
        # P1-c: an existing-but-empty XDG hooks dir must be skipped, and the
        # search must continue to later candidates instead of breaking.
        xdg = os.path.join(self.tmp, "xdg-data")
        empty_hooks = os.path.join(xdg, "anolisa", "adapters", "tokenless", "common", "hooks")
        os.makedirs(empty_hooks)
        os.environ["XDG_DATA_HOME"] = xdg
        try:
            plugin = _load_plugin(self.plugin_copy, "hermes_plugin_incomplete_xdg")
        except ImportError as exc:
            # No later candidate exists on this machine — the diagnostic must
            # name the incomplete dir with its rejection reason (P2 wording).
            self.assertIn("hook_utils.py missing", str(exc))
            self.assertIn(empty_hooks, str(exc))
        else:
            # A later candidate (e.g. passwd-home install) won — but never
            # the incomplete XDG dir.
            self.assertNotEqual(plugin._HOOK_UTILS_RESOLVED, os.path.realpath(empty_hooks))

    def test_import_error_mentions_trust_policy(self):
        # P2: the diagnostic must explain that existing paths can be
        # rejected by the trust policy, not only be "missing".
        xdg = os.path.join(self.tmp, "xdg-data")
        hooks = _make_hooks_dir(xdg)
        os.chmod(hooks, 0o777)  # exists but untrusted
        os.environ["XDG_DATA_HOME"] = xdg
        try:
            plugin = _load_plugin(self.plugin_copy, "hermes_plugin_untrusted_xdg")
        except ImportError as exc:
            self.assertIn("world-writable", str(exc))
            self.assertIn("trust policy", str(exc))
        else:
            # Later candidate won; the untrusted dir must not be selected.
            self.assertNotEqual(plugin._HOOK_UTILS_RESOLVED, os.path.realpath(hooks))


if __name__ == "__main__":
    unittest.main()
