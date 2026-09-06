#!/usr/bin/env python3
"""Regression tests for the Hermes plugin hook_utils resolution.

Covers the review findings on PR #2058 and PR #2249:
- P1-a: the hooks directory itself, its parent, and hook_utils.py must all
  be rejected when world-writable or foreign-owned (not just the parent).
- P1-b: copy-installs must honor XDG_DATA_HOME (anolisa FsLayout::user
  prefers it over ~/.local/share).
- P1-c: an existing-but-incomplete high-priority candidate must not stop
  the search; later valid candidates are still tried.
- P2: candidate list contains no empty placeholders; _validate_hooks_dir
  rejects relative/empty paths; the ImportError mentions trust-policy
  rejections, not just "missing".
- PR #2249 P1: a trusted candidate shipping an older hook_utils.py that
  lacks the Protocol v2 lifecycle symbols must be rejected by the API
  compatibility check so the search continues to later candidates.

After the Hermes lifecycle migration the adapter is a thin Core client:
RTK execution and rtk-prefix anchoring are owned by tokenless-runtime, so
the anchoring cases below exercise the shared hook_utils implementation
directly. There is no degraded mode anymore — an incompatible hook_utils
fails the plugin import with a diagnostic instead.
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


def _load_shared_hook_utils():
    """Import the real shared hook_utils module from the source tree."""
    sys.modules.pop("hook_utils", None)
    sys.path.insert(0, _HOOKS_SRC)
    try:
        import hook_utils as hook_utils_mod  # type: ignore[import-not-found]
        return hook_utils_mod
    finally:
        sys.path.pop(0)


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


class VersionMismatchTest(unittest.TestCase):
    """Regression tests for shared hook_utils version mismatch (PR #2249 P1).

    When a candidate passes the trust check (hook_utils.py exists, ownership
    and permissions OK) but ships an older hook_utils that lacks the
    Protocol v2 lifecycle symbols this adapter imports
    (build_pre_tool_request, build_post_tool_request, run_compress), the
    candidate must be rejected with an "API mismatch" reason and the search
    must continue to later candidates. When no compatible candidate exists,
    the plugin import fails with a diagnostic — there is no degraded mode
    anymore, because the lifecycle adapter delegates every feature to Core.
    """

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hermes-version-test-")
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

    def _make_old_hooks_dir(self, base: str) -> str:
        """Create a hooks dir whose hook_utils.py lacks the lifecycle API."""
        hooks = os.path.join(base, "anolisa", "adapters", "tokenless", "common", "hooks")
        os.makedirs(hooks, mode=0o755)
        # Write a minimal hook_utils.py without the Protocol v2 builders
        # (pre-lifecycle installs shipped exactly this shape).
        with open(os.path.join(hooks, "hook_utils.py"), "w") as f:
            f.write(
                "# Old hook_utils without the Protocol v2 lifecycle API\n"
                "def resolve_binary(name, *fallbacks): return None\n"
            )
        # Copy tool_categories.json (needed by some imports)
        shutil.copy(
            os.path.join(_HOOKS_SRC, "tool_categories.json"),
            hooks,
        )
        os.chmod(hooks, 0o755)
        return hooks

    def test_old_hooks_rejected_by_api_compat_check(self):
        # A candidate whose hook_utils.py lacks the lifecycle builders must
        # be rejected by _check_api_compat, not accepted and then crash on
        # the top-level from-import.
        xdg = os.path.join(self.tmp, "xdg-data")
        old_hooks = self._make_old_hooks_dir(xdg)
        os.environ["XDG_DATA_HOME"] = xdg
        try:
            plugin = _load_plugin(self.plugin_copy, "hermes_plugin_old_hooks")
        except ImportError as exc:
            # No compatible candidate found — diagnostic mentions API mismatch.
            self.assertIn("API mismatch", str(exc))
            self.assertIn(old_hooks, str(exc))
        else:
            # A later candidate with the correct version won.
            self.assertNotEqual(plugin._HOOK_UTILS_RESOLVED, os.path.realpath(old_hooks))

    def test_no_compat_candidate_fails_with_api_mismatch(self):
        # When no candidate provides the Protocol v2 symbols, the plugin
        # must fail loudly with the API mismatch diagnostic instead of
        # silently importing a stale hook_utils. (The pre-lifecycle version
        # degraded to local fallbacks; Core now owns every feature, so an
        # adapter without the lifecycle API has nothing useful to do.)
        xdg = os.path.join(self.tmp, "xdg-data")
        old_hooks = self._make_old_hooks_dir(xdg)
        os.environ["XDG_DATA_HOME"] = xdg
        try:
            plugin = _load_plugin(self.plugin_copy, "hermes_plugin_incompat")
        except ImportError as exc:
            msg = str(exc)
            self.assertIn("API mismatch", msg)
            self.assertIn(old_hooks, msg)
            self.assertIn("build_pre_tool_request", msg)
        else:
            # A later, newer candidate won — never the stale one.
            self.assertNotEqual(plugin._HOOK_UTILS_RESOLVED, os.path.realpath(old_hooks))

class AnchorRtkPrefixTest(unittest.TestCase):
    """Regression tests for _anchor_rtk_prefix semicolon and newline handling.

    Covers ikunkun-sys's review findings on PR #2249:
    - P1: semicolon-chained ``rtk`` (e.g. ``rtk git status; rtk cargo test``)
      must anchor the second ``rtk`` token.
    - P1: newline-separated commands must preserve the newline, not collapse
      to spaces.
    - P2: _check_api_compat must keep the freshly imported module in
      sys.modules on success, not restore the stale cached copy.

    The anchoring cases target the shared hook_utils implementation (the
    Hermes adapter no longer imports _anchor_rtk_prefix itself — Core owns
    RTK execution and anchoring after the lifecycle migration).
    """

    @classmethod
    def setUpClass(cls):
        cls.plugin = _load_plugin(_PLUGIN_SRC, "hermes_anchor_test")
        cls.hook_utils = _load_shared_hook_utils()

    # -- P1: semicolon-chained rtk (shared impl) ----------------------------

    def test_semicolon_chained_rtk_anchors_both_segments(self):
        # RTK 0.43 compound rewrite: "rtk git status; rtk cargo test"
        # The semicolon is attached to "status" as "status;".  Both rtk
        # tokens must be anchored.
        cmd = "rtk git status; rtk cargo test"
        result = self.hook_utils._anchor_rtk_prefix(cmd, "/usr/bin/rtk")
        self.assertEqual(
            result,
            "/usr/bin/rtk git status; /usr/bin/rtk cargo test",
            "both rtk tokens must be anchored",
        )

    # -- P1: newline preservation (shared impl) ------------------------------

    def test_newline_separator_preserved(self):
        # "rtk git status\ncargo build" — the newline must be preserved,
        # not collapsed to a space.  Otherwise "cargo build" would become
        # an argument to "status" and never execute.
        cmd = "rtk git status\ncargo build"
        result = self.hook_utils._anchor_rtk_prefix(cmd, "/usr/bin/rtk")
        self.assertIn("\n", result, "newline must be preserved, not collapsed to space")
        self.assertEqual(
            result,
            "/usr/bin/rtk git status\ncargo build",
            "only rtk should be replaced, newline and rest preserved",
        )


    def test_check_api_compat_keeps_fresh_module_on_success(self):
        # When _check_api_compat succeeds, the freshly imported module must
        # stay in sys.modules — the old cached module must NOT be restored.
        import types

        # Create a fake "old" module and put it in sys.modules
        old_mod = types.ModuleType("hook_utils")
        old_mod._STALE = True  # marker so we can detect it
        sys.modules["hook_utils"] = old_mod

        try:
            # Trial-import from the real source-tree hooks dir
            hooks_dir = os.path.realpath(_HOOKS_SRC)
            reason = self.plugin._check_api_compat(hooks_dir)
            self.assertIsNone(reason, "compatible candidate must pass API check")

            # The module in sys.modules must be the fresh one, not old_mod
            current = sys.modules.get("hook_utils")
            self.assertIsNotNone(current, "module must remain in sys.modules")
            self.assertFalse(
                getattr(current, "_STALE", False),
                "stale module was restored instead of fresh candidate",
            )
        finally:
            sys.modules.pop("hook_utils", None)

    # -- P1: newline is a real segment boundary (shared impl) ---------------

    def test_newline_separated_rtk_anchors_both_segments(self):
        # Newline terminates a command exactly like `;`: the rtk after
        # the newline starts a fresh segment and must be anchored.
        cmd = "rtk git status\nrtk cargo test"
        result = self.hook_utils._anchor_rtk_prefix(cmd, "/usr/bin/rtk")
        self.assertEqual(
            result,
            "/usr/bin/rtk git status\n/usr/bin/rtk cargo test",
            "rtk after newline must be anchored",
        )

    def test_escaped_semicolon_is_not_a_boundary(self):
        # `\;` is an escaped argument character, not a command
        # separator: the trailing `rtk` is grep's argument and must
        # stay bare instead of being anchored.
        cmd = "rtk grep foo\\; rtk file"
        result = self.hook_utils._anchor_rtk_prefix(cmd, "/usr/bin/rtk")
        self.assertEqual(
            result,
            "/usr/bin/rtk grep foo\\; rtk file",
            "escaped semicolon must not start a new segment",
        )


    def test_rtk_argument_in_ignored_segment_not_anchored(self):
        # "echo rtk && rtk git status" — the first rtk is a positional
        # argument to echo (an ignored segment kept as-is) and must not be
        # anchored; the second rtk is in command position and must be
        # anchored.
        cmd = "echo rtk && rtk git status"
        result = self.hook_utils._anchor_rtk_prefix(cmd, "/usr/bin/rtk")
        self.assertEqual(
            result,
            "echo rtk && /usr/bin/rtk git status",
            "only command-position rtk is anchored",
        )

    def test_rtk_argument_alone_not_anchored(self):
        cmd = "echo rtk done"
        result = self.hook_utils._anchor_rtk_prefix(cmd, "/usr/bin/rtk")
        self.assertEqual(result, "echo rtk done")

    def test_wrapper_before_rtk_still_anchors(self):
        # Transparent wrappers (e.g. sudo) must not consume command position.
        cmd = "sudo rtk git status"
        result = self.hook_utils._anchor_rtk_prefix(cmd, "/usr/bin/rtk")
        self.assertEqual(result, "sudo /usr/bin/rtk git status")

    # -- P1: command position vs argument position (degraded/fallback impl) -

class AnchorTransparentPrefixTest(unittest.TestCase):
    """Regression tests for RTK's transparent-prefix protocol anchoring.

    Covers ikunkun-sys's 2026-08-14 review finding on PR #2249: the
    command-position state machine only whitelisted fixed shell wrappers,
    so RTK v0.43 outputs that start with a transparent prefix — built-ins
    (``uv run``, ``noglob``, ``command``, ``builtin``, ``exec``,
    ``nocorrect``) or user-configured multi-word
    ``[hooks].transparent_prefixes`` (e.g. ``shadowenv exec --``) — kept a
    bare ``rtk`` that fails with exit 127 in trimmed-PATH environments.

    Every test sandboxes HOME/XDG_CONFIG_HOME under a fresh temp dir with
    a known rtk config.toml so results are deterministic on any host,
    with or without a real rtk installation or user config.
    """

    CONFIGURED = 'transparent_prefixes = ["shadowenv exec --", "docker exec c1", "foo bar"]'

    @classmethod
    def setUpClass(cls):
        cls.plugin = _load_plugin(_PLUGIN_SRC, "hermes_anchor_tprefix_test")
        cls.hook_utils = _load_shared_hook_utils()

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="anchor-tprefix-test-")
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self._saved_home = os.environ.get("HOME")
        self._saved_xdg_config = os.environ.get("XDG_CONFIG_HOME")
        # Sandboxed HOME so rtk's config.toml (configured transparent
        # prefixes) resolves deterministically under the sandbox.
        os.environ["HOME"] = self.tmp
        cfg = os.path.join(self.tmp, ".config", "rtk")
        os.makedirs(cfg)
        os.environ["XDG_CONFIG_HOME"] = os.path.join(self.tmp, ".config")
        with open(os.path.join(cfg, "config.toml"), "w") as f:
            f.write("[hooks]\n" + self.CONFIGURED + "\n")

    def tearDown(self):
        for name, saved in (
            ("HOME", self._saved_home),
            ("XDG_CONFIG_HOME", self._saved_xdg_config),
        ):
            if saved is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = saved

    # -- anchoring cases -------------------------------------------------------

    def test_builtin_transparent_prefixes_anchor(self):
        for wrapper in ("noglob", "command", "builtin", "exec", "nocorrect"):
            with self.subTest(wrapper=wrapper):
                result = self.hook_utils._anchor_rtk_prefix(
                    f"{wrapper} rtk git status", "/usr/bin/rtk",
                )
                self.assertEqual(result, f"{wrapper} /usr/bin/rtk git status")

    def test_uv_run_multiword_builtin_anchors(self):
        result = self.hook_utils._anchor_rtk_prefix("uv run rtk pytest tests/", "/usr/bin/rtk")
        self.assertEqual(result, "uv run /usr/bin/rtk pytest tests/")

    def test_env_assignment_composes_before_builtin(self):
        result = self.hook_utils._anchor_rtk_prefix(
            "PYTHONPATH=. uv run rtk pytest tests/", "/usr/bin/rtk",
        )
        self.assertEqual(result, "PYTHONPATH=. uv run /usr/bin/rtk pytest tests/")

    def test_wrapper_nests_with_builtin(self):
        result = self.hook_utils._anchor_rtk_prefix("sudo noglob rtk git status", "/usr/bin/rtk")
        self.assertEqual(result, "sudo noglob /usr/bin/rtk git status")

    def test_configured_transparent_prefix_anchors(self):
        result = self.hook_utils._anchor_rtk_prefix(
            "shadowenv exec -- rtk git status", "/usr/bin/rtk",
        )
        self.assertEqual(result, "shadowenv exec -- /usr/bin/rtk git status")

    def test_second_configured_prefix_anchors(self):
        result = self.hook_utils._anchor_rtk_prefix("docker exec c1 rtk git status", "/usr/bin/rtk")
        self.assertEqual(result, "docker exec c1 /usr/bin/rtk git status")

    def test_env_between_configured_prefix_and_rtk_anchors(self):
        result = self.hook_utils._anchor_rtk_prefix(
            "shadowenv exec -- FOO=bar rtk git status", "/usr/bin/rtk",
        )
        self.assertEqual(result, "shadowenv exec -- FOO=bar /usr/bin/rtk git status")

    def test_configured_prefix_anchors_in_every_segment(self):
        result = self.hook_utils._anchor_rtk_prefix(
            "noglob rtk git status; shadowenv exec -- rtk cargo test", "/usr/bin/rtk",
        )
        self.assertEqual(
            result,
            "noglob /usr/bin/rtk git status; shadowenv exec -- /usr/bin/rtk cargo test",
        )

    def test_partial_configured_prefix_not_matched(self):
        # Bare `shadowenv` is not the configured `shadowenv exec --`: the
        # command position is consumed and the rtk stays bare.
        result = self.hook_utils._anchor_rtk_prefix("shadowenv rtk git status", "/usr/bin/rtk")
        self.assertEqual(result, "shadowenv rtk git status")

    def test_configured_prefix_never_crosses_segment_boundary(self):
        # "foo bar" is configured, but `;` splits the sequence: segment 2
        # starts with `bar`, which alone is not a prefix.
        result = self.hook_utils._anchor_rtk_prefix("foo; bar rtk git status", "/usr/bin/rtk")
        self.assertEqual(result, "foo; bar rtk git status")

    def test_echo_rtk_argument_still_not_anchored(self):
        result = self.hook_utils._anchor_rtk_prefix("echo rtk && rtk git status", "/usr/bin/rtk")
        self.assertEqual(result, "echo rtk && /usr/bin/rtk git status")

    # -- wrapper option operands (mixed compound negatives) ------------------

    def test_wrapper_option_operands_not_anchored(self):
        # A wrapper option consumes the command position: the username
        # operand of `sudo -u`, the variable name of `env -u`, and the
        # query argument of `command -v` must never be rewritten into the
        # executable path, while the following segment's command-position
        # rtk is still anchored.
        cases = [
            (
                "sudo -u rtk true && rtk git status",
                "sudo -u rtk true && /usr/bin/rtk git status",
            ),
            ("env -u rtk && rtk git status", "env -u rtk && /usr/bin/rtk git status"),
            (
                "command -v rtk && rtk git status",
                "command -v rtk && /usr/bin/rtk git status",
            ),
        ]
        for cmd, want in cases:
            with self.subTest(cmd=cmd):
                result = self.hook_utils._anchor_rtk_prefix(cmd, "/usr/bin/rtk")
                self.assertEqual(result, want)

    def test_wrapper_option_segment_alone_not_anchored(self):
        result = self.hook_utils._anchor_rtk_prefix("sudo -u rtk true", "/usr/bin/rtk")
        self.assertEqual(result, "sudo -u rtk true")

    def test_option_operand_in_later_segment_not_anchored(self):
        result = self.hook_utils._anchor_rtk_prefix(
            "rtk git status && sudo -u rtk true", "/usr/bin/rtk",
        )
        self.assertEqual(result, "/usr/bin/rtk git status && sudo -u rtk true")

    def test_bare_wrappers_still_anchor_after_option_fix(self):
        # Regression guard for the operand fix: option-less wrappers and
        # transparent prefixes keep anchoring.
        for cmd, want in (
            ("sudo rtk git status", "sudo /usr/bin/rtk git status"),
            ("sudo noglob rtk git status", "sudo noglob /usr/bin/rtk git status"),
            ("uv run rtk pytest tests/", "uv run /usr/bin/rtk pytest tests/"),
            ("env FOO=bar rtk git status", "env FOO=bar /usr/bin/rtk git status"),
            (
                "shadowenv exec -- rtk git status",
                "shadowenv exec -- /usr/bin/rtk git status",
            ),
        ):
            with self.subTest(cmd=cmd):
                result = self.hook_utils._anchor_rtk_prefix(cmd, "/usr/bin/rtk")
                self.assertEqual(result, want)


if __name__ == "__main__":
    unittest.main()
