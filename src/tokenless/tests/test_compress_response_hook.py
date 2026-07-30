#!/usr/bin/env python3
"""Integration tests for compress_response_hook.py.

Validates the PostToolUse hook output contract:
- Replacement semantics: updatedToolOutput replaces (not appends to) original.
- Additivity: additionalContext is reserved for env-attribution diagnostics.
- No duplicate content in the model-visible output.
- Pass-through when compression yields no size reduction.
- Legacy path for non-replacement adapters.

Uses subprocess to invoke the hook with a mock tokenless binary,
avoiding Python version issues with the hook_utils module.
"""

import importlib.machinery
import importlib.util
import json
import os
import stat
import subprocess
import sys
import shutil
import tempfile
import textwrap
import types
import unittest
from unittest import mock


def _make_large_json_payload(char_target: int = 500) -> dict:
    """Build a JSON payload larger than _MIN_RESPONSE_CHARS (200)."""
    return {
        "stdout": "x" * char_target,
        "stderr": "",
        "exit_code": 0,
        "interrupted": False,
    }


def _create_mock_tokenless(tmpdir: str, behavior: str = "compress") -> str:
    """Create a mock tokenless binary that simulates compression behavior."""
    mock_script = os.path.join(tmpdir, "tokenless")

    if behavior == "compress":
        script = textwrap.dedent("""\
            #!/usr/bin/env python3
            import json, sys
            if sys.argv[1] == "compress-response":
                data = json.loads(sys.stdin.read())
                compressed = {}
                for k, v in data.items():
                    if isinstance(v, str) and len(v) > 20:
                        compressed[k] = v[:20]
                    else:
                        compressed[k] = v
                print(json.dumps(compressed))
            elif sys.argv[1] == "compress-toon":
                sys.exit(1)
        """)
    elif behavior == "no-savings":
        script = textwrap.dedent("""\
            #!/usr/bin/env python3
            import json, sys
            if sys.argv[1] == "compress-response":
                data = json.loads(sys.stdin.read())
                data["extra_padding"] = "x" * 200
                print(json.dumps(data))
            elif sys.argv[1] == "compress-toon":
                sys.exit(1)
        """)
    elif behavior == "passthrough":
        script = textwrap.dedent("""\
            #!/usr/bin/env python3
            import sys
            data = sys.stdin.read()
            print(data)
        """)
    else:
        raise ValueError(f"Unknown behavior: {behavior}")

    with open(mock_script, "w") as f:
        f.write(script)
    os.chmod(mock_script, os.stat(mock_script).st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
    return mock_script


def _create_mock_claude(tmpdir: str, version: str = "2.1.121") -> str:
    """Create a mock claude binary that reports a specific version."""
    mock_script = os.path.join(tmpdir, "claude")
    script = textwrap.dedent(f"""\
        #!/usr/bin/env python3
        import sys
        if "--version" in sys.argv:
            print("{version}")
    """)
    with open(mock_script, "w") as f:
        f.write(script)
    os.chmod(mock_script, os.stat(mock_script).st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
    return mock_script


def _run_hook(stdin_data: dict, agent_id: str, mock_tokenless_path: str,
              isolated_home: str = None) -> dict:
    """Run the hook as a subprocess with mocked tokenless binary.

    Args:
        stdin_data: JSON payload to feed to the hook via stdin.
        agent_id: The adapter agent ID (e.g. "claude-code").
        mock_tokenless_path: Path to the mock tokenless binary.
        isolated_home: Temporary HOME directory for the subprocess to avoid
            touching the caller's ~/.tokenless state.

    Returns:
        Parsed JSON output dict from the hook, or a dict with ``_subprocess_error``
        key when the hook exits non-zero.
    """
    hooks_dir = os.path.normpath(os.path.join(
        os.path.dirname(__file__),
        os.pardir, "adapters", "tokenless", "common", "hooks",
    ))
    hook_path = os.path.join(hooks_dir, "compress_response_hook.py")

    env = os.environ.copy()
    env["TOKENLESS_AGENT_ID"] = agent_id
    env["PATH"] = os.path.dirname(mock_tokenless_path) + ":" + env.get("PATH", "")
    # Isolate HOME so hook doesn't read/write ~/.tokenless/.claude-version
    if isolated_home:
        env["HOME"] = isolated_home

    proc = subprocess.run(
        [sys.executable, hook_path],
        input=json.dumps(stdin_data),
        capture_output=True,
        text=True,
        timeout=10,
        env=env,
    )

    # Check returncode first — a non-zero exit indicates a real failure
    # (import error, runtime crash, etc.) that should not be silently
    # swallowed as an empty result.
    if proc.returncode != 0:
        return {
            "_subprocess_error": True,
            "_returncode": proc.returncode,
            "_stderr": proc.stderr,
            "_stdout": proc.stdout,
        }

    stdout = proc.stdout.strip()
    if not stdout or stdout == "{}":
        return {}
    try:
        return json.loads(stdout)
    except json.JSONDecodeError:
        return {"_raw_stdout": stdout, "_stderr": proc.stderr}


_needs_py39 = sys.version_info < (3, 9)


@unittest.skipIf(_needs_py39, "hook_utils requires Python 3.9+")
class TestBinaryFallbackPaths(unittest.TestCase):
    @staticmethod
    def _hook_utils() -> types.ModuleType:
        hooks_dir = os.path.normpath(os.path.join(
            os.path.dirname(__file__),
            os.pardir, "adapters", "tokenless", "common", "hooks",
        ))
        sys.path.insert(0, hooks_dir)
        try:
            import hook_utils
        finally:
            sys.path.pop(0)
        return hook_utils

    @staticmethod
    def _codex_check_tokenless() -> types.ModuleType:
        script_path = os.path.normpath(
            os.path.join(
                os.path.dirname(__file__),
                os.pardir,
                "adapters",
                "tokenless",
                "codex",
                "scripts",
                "check-tokenless",
            )
        )
        loader = importlib.machinery.SourceFileLoader(
            "codex_check_tokenless", script_path
        )
        spec = importlib.util.spec_from_loader("codex_check_tokenless", loader)
        if spec is None or spec.loader is None:
            raise RuntimeError("unable to load codex check-tokenless")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def test_supported_install_layouts_are_covered(self) -> None:
        paths = self._hook_utils()._known_binary_paths("rtk", "/home/alice")
        expected = (
            "/home/alice/.local/bin/rtk",
            "/home/alice/.local/lib/anolisa/libexec/tokenless/rtk",
            "/home/alice/.local/libexec/anolisa/tokenless/rtk",
            "/usr/local/bin/rtk",
            "/usr/local/libexec/anolisa/tokenless/rtk",
            "/usr/bin/rtk",
            "/usr/libexec/anolisa/tokenless/rtk",
            "/usr/lib/anolisa/tokenless/rtk",
            "/home/alice/.local/share/anolisa/tokenless/rtk",
            "/home/alice/.local/lib/anolisa/tokenless/rtk",
        )
        self.assertEqual(paths, expected)

    def test_generic_binaries_skip_tokenless_helper_dirs(self) -> None:
        paths = self._hook_utils()._known_binary_paths("docker", "/home/alice")
        self.assertIn("/home/alice/.local/bin/docker", paths)
        self.assertIn("/usr/local/bin/docker", paths)
        self.assertIn("/usr/bin/docker", paths)
        self.assertFalse(any("tokenless" in path for path in paths))

    def test_user_layouts_require_an_absolute_home(self) -> None:
        hook_utils = self._hook_utils()
        for home in ("", "relative/home"):
            with self.subTest(home=home):
                paths = hook_utils._known_binary_paths("rtk", home)
                self.assertTrue(all(os.path.isabs(path) for path in paths))
                self.assertFalse(any(".local" in path for path in paths))

    def test_codex_check_tokenless_uses_canonical_order(self) -> None:
        paths = self._codex_check_tokenless()._known_tokenless_paths("/home/alice")
        self.assertEqual(
            paths,
            (
                "/home/alice/.local/bin/tokenless",
                "/usr/local/bin/tokenless",
                "/usr/bin/tokenless",
                "/home/alice/.local/share/anolisa/tokenless/tokenless",
                "/home/alice/.local/lib/anolisa/tokenless/tokenless",
            ),
        )

    def test_codex_check_tokenless_rejects_invalid_home(self) -> None:
        check_tokenless = self._codex_check_tokenless()
        for home in ("", "relative/home"):
            with self.subTest(home=home):
                paths = check_tokenless._known_tokenless_paths(home)
                self.assertEqual(
                    paths,
                    ("/usr/local/bin/tokenless", "/usr/bin/tokenless"),
                )

    def test_resolver_finds_makefile_user_helper_without_path(self) -> None:
        hook_utils = self._hook_utils()
        with tempfile.TemporaryDirectory() as home:
            helper_dir = os.path.join(
                home, ".local", "libexec", "anolisa", "tokenless"
            )
            os.makedirs(helper_dir)
            rtk_path = os.path.join(helper_dir, "rtk")
            with open(rtk_path, "w", encoding="utf-8") as handle:
                handle.write("#!/bin/sh\n")
            os.chmod(rtk_path, 0o755)

            hook_utils._resolved_cache.clear()
            with (
                mock.patch.dict(os.environ, {"HOME": home}),
                mock.patch.object(hook_utils.shutil, "which", return_value=None),
            ):
                self.assertEqual(hook_utils.resolve_binary("rtk"), rtk_path)
            hook_utils._resolved_cache.clear()

    def test_resolver_prefers_user_layout_to_explicit_legacy_fallback(self) -> None:
        hook_utils = self._hook_utils()
        with tempfile.TemporaryDirectory() as home:
            local_bin = os.path.join(home, ".local", "bin")
            legacy_bin = os.path.join(home, "legacy")
            os.makedirs(local_bin)
            os.makedirs(legacy_bin)
            user_rtk = os.path.join(local_bin, "rtk")
            legacy_rtk = os.path.join(legacy_bin, "rtk")
            for path in (user_rtk, legacy_rtk):
                with open(path, "w", encoding="utf-8") as handle:
                    handle.write("#!/bin/sh\n")
                os.chmod(path, 0o755)

            hook_utils._resolved_cache.clear()
            with (
                mock.patch.dict(os.environ, {"HOME": home}),
                mock.patch.object(hook_utils.shutil, "which", return_value=None),
            ):
                self.assertEqual(
                    hook_utils.resolve_binary("rtk", legacy_rtk), user_rtk
                )
            hook_utils._resolved_cache.clear()


@unittest.skipIf(_needs_py39, "hook_utils requires Python 3.9+")
class TestReplacementProtocol(unittest.TestCase):
    """Verify updatedToolOutput replacement semantics for Claude Code."""

    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.isolated_home = tempfile.mkdtemp(prefix="test_hook_home_")
        self.mock_bin = _create_mock_tokenless(self.tmpdir, "compress")
        self.mock_claude = _create_mock_claude(self.tmpdir)

    def tearDown(self):
        shutil.rmtree(self.tmpdir, ignore_errors=True)
        shutil.rmtree(self.isolated_home, ignore_errors=True)

    def test_claude_code_uses_updated_tool_output(self):
        """Claude Code adapter should use updatedToolOutput, not additionalContext."""
        large_payload = _make_large_json_payload()

        result = _run_hook(
            {
                "tool_name": "Bash",
                "tool_response": large_payload,
                "session_id": "test-session",
                "tool_use_id": "toolu_test",
            },
            agent_id="claude-code",
            mock_tokenless_path=self.mock_bin,
            isolated_home=self.isolated_home,
        )

        self.assertNotIn("_subprocess_error", result,
                         f"Hook subprocess failed: {result}")
        hso = result.get("hookSpecificOutput", {})
        self.assertEqual(hso.get("hookEventName"), "PostToolUse")
        self.assertIn("updatedToolOutput", hso,
                       "Claude Code should use updatedToolOutput for replacement")
        self.assertNotIn("additionalContext", hso,
                         "Compressed content must not be in additionalContext (duplication)")

    def test_replacement_is_smaller(self):
        """The replacement output should be smaller than the original."""
        large_payload = _make_large_json_payload(1000)

        result = _run_hook(
            {
                "tool_name": "Bash",
                "tool_response": large_payload,
                "session_id": "s",
                "tool_use_id": "t",
            },
            agent_id="claude-code",
            mock_tokenless_path=self.mock_bin,
            isolated_home=self.isolated_home,
        )

        self.assertNotIn("_subprocess_error", result,
                         f"Hook subprocess failed: {result}")
        hso = result.get("hookSpecificOutput", {})
        replacement = hso.get("updatedToolOutput", "")
        original_size = len(json.dumps(large_payload, separators=(",", ":")))
        replacement_size = (
            len(json.dumps(replacement, separators=(",", ":")))
            if isinstance(replacement, (dict, list))
            else len(str(replacement))
        )
        self.assertLess(replacement_size, original_size,
                        "Replacement should be smaller than original")

    def test_replacement_content_structure(self):
        """Replacement should contain compressed stdout and valid schema fields."""
        large_payload = _make_large_json_payload()

        result = _run_hook(
            {
                "tool_name": "Bash",
                "tool_response": large_payload,
                "session_id": "s",
                "tool_use_id": "t",
            },
            agent_id="claude-code",
            mock_tokenless_path=self.mock_bin,
            isolated_home=self.isolated_home,
        )

        self.assertNotIn("_subprocess_error", result,
                         f"Hook subprocess failed: {result}")
        hso = result.get("hookSpecificOutput", {})
        replacement = hso.get("updatedToolOutput", "")

        # The mock compressor truncates strings > 20 chars, so stdout should
        # be truncated. Verify the compressed content is present and parseable.
        # Note: updatedToolOutput may be a JSON string or already-parsed dict
        # depending on how the hook encodes it.
        if isinstance(replacement, str):
            try:
                compressed_data = json.loads(replacement)
            except json.JSONDecodeError:
                self.fail(f"updatedToolOutput should be valid JSON, got: {replacement!r}")
        elif isinstance(replacement, (dict, list)):
            compressed_data = replacement
        else:
            self.fail(f"updatedToolOutput unexpected type: {type(replacement)}")

        # Verify stdout field was compressed to exactly 20 chars by mock
        # (mock truncates strings > 20 to their first 20 chars).
        self.assertIn("stdout", compressed_data,
                       "Compressed output should preserve stdout key")
        self.assertEqual(compressed_data["stdout"], "x" * 20,
                         "stdout should be truncated to exactly 'x' * 20")

        # Verify schema fields are preserved with correct values
        self.assertEqual(compressed_data["exit_code"], 0)
        self.assertEqual(compressed_data["interrupted"], False)

    def test_no_duplicate_content(self):
        """The original sentinel must not appear alongside compressed output."""
        sentinel = "UNIQUE_SENTINEL_12345"
        # Mock truncates strings > 20 chars; sentinel is 21 chars,
        # so truncated form is first 20 chars.
        truncated_sentinel = sentinel[:20]
        payload = {"stdout": sentinel * 30, "stderr": "", "exit_code": 0, "interrupted": False}

        result = _run_hook(
            {
                "tool_name": "Bash",
                "tool_response": payload,
                "session_id": "s",
                "tool_use_id": "t",
            },
            agent_id="claude-code",
            mock_tokenless_path=self.mock_bin,
            isolated_home=self.isolated_home,
        )

        self.assertNotIn("_subprocess_error", result,
                         f"Hook subprocess failed: {result}")
        hso = result.get("hookSpecificOutput", {})

        # additionalContext must not contain compressed content
        additional = hso.get("additionalContext", "")
        self.assertNotIn(sentinel, additional,
                         "additionalContext must not contain compressed content")

        # updatedToolOutput should exist and contain the truncated sentinel
        self.assertIn("updatedToolOutput", hso,
                       "Claude Code should use updatedToolOutput")
        updated = hso["updatedToolOutput"]
        if isinstance(updated, str):
            updated_data = json.loads(updated)
        else:
            updated_data = updated

        # The mock truncates the sentinel (21 chars) to its first 20 chars.
        # Assert the truncated form IS present (proves content wasn't lost).
        self.assertIn("stdout", updated_data,
                       "updatedToolOutput should contain stdout field")
        self.assertEqual(updated_data["stdout"], truncated_sentinel,
                         "stdout should be the truncated sentinel (first 20 chars)")

        # Full sentinel must NOT appear (proves content wasn't duplicated)
        updated_str = json.dumps(updated) if isinstance(updated, (dict, list)) else str(updated)
        self.assertNotIn(sentinel * 30, updated_str,
                         "updatedToolOutput must not contain the full original sentinel")


@unittest.skipIf(_needs_py39, "hook_utils requires Python 3.9+")
class TestPassthrough(unittest.TestCase):
    """Verify pass-through when compression yields no size reduction."""

    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.isolated_home = tempfile.mkdtemp(prefix="test_hook_home_")
        self.mock_bin = _create_mock_tokenless(self.tmpdir, "no-savings")
        self.mock_claude = _create_mock_claude(self.tmpdir)

    def tearDown(self):
        shutil.rmtree(self.tmpdir, ignore_errors=True)
        shutil.rmtree(self.isolated_home, ignore_errors=True)

    def test_skip_when_no_compression_savings(self):
        """When compression does not reduce size, output should be empty (skip)."""
        payload = _make_large_json_payload()

        result = _run_hook(
            {
                "tool_name": "Bash",
                "tool_response": payload,
                "session_id": "s",
                "tool_use_id": "t",
            },
            agent_id="claude-code",
            mock_tokenless_path=self.mock_bin,
            isolated_home=self.isolated_home,
        )

        self.assertNotIn("_subprocess_error", result,
                         f"Hook subprocess failed: {result}")
        self.assertEqual(result, {},
                         "Should skip when compression yields no savings")


@unittest.skipIf(_needs_py39, "hook_utils requires Python 3.9+")
class TestSkipTools(unittest.TestCase):
    """Verify skip-tools behavior (content retrieval tools)."""

    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.isolated_home = tempfile.mkdtemp(prefix="test_hook_home_")
        self.mock_bin = _create_mock_tokenless(self.tmpdir, "compress")
        self.mock_claude = _create_mock_claude(self.tmpdir)

    def tearDown(self):
        shutil.rmtree(self.tmpdir, ignore_errors=True)
        shutil.rmtree(self.isolated_home, ignore_errors=True)

    def test_skip_tools_no_replacement(self):
        """Skip-tools (Read) should not use updatedToolOutput."""
        payload = {"stdout": "file content", "stderr": "", "exit_code": 0}

        result = _run_hook(
            {
                "tool_name": "Read",
                "tool_response": payload,
                "session_id": "s",
                "tool_use_id": "t",
            },
            agent_id="claude-code",
            mock_tokenless_path=self.mock_bin,
            isolated_home=self.isolated_home,
        )

        self.assertNotIn("_subprocess_error", result,
                         f"Hook subprocess failed: {result}")
        self.assertEqual(result, {},
                         "Skip-tools (Read) should produce empty result (pass-through)")
        hso = result.get("hookSpecificOutput", {})
        self.assertNotIn("updatedToolOutput", hso,
                         "Skip-tools should not replace tool output")


@unittest.skipIf(_needs_py39, "hook_utils requires Python 3.9+")
class TestNonReplacementAdapters(unittest.TestCase):
    """Verify non-Claude-Code adapters still get the legacy additionalContext."""

    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.isolated_home = tempfile.mkdtemp(prefix="test_hook_home_")
        self.mock_bin = _create_mock_tokenless(self.tmpdir, "compress")
        self.mock_claude = _create_mock_claude(self.tmpdir)

    def tearDown(self):
        shutil.rmtree(self.tmpdir, ignore_errors=True)
        shutil.rmtree(self.isolated_home, ignore_errors=True)

    def test_qwencode_uses_additional_context(self):
        """Qwen Code should use additionalContext (legacy path)."""
        large_payload = _make_large_json_payload()

        result = _run_hook(
            {
                "tool_name": "Bash",
                "tool_response": large_payload,
                "session_id": "s",
                "tool_use_id": "t",
            },
            agent_id="qwencode",
            mock_tokenless_path=self.mock_bin,
            isolated_home=self.isolated_home,
        )

        self.assertNotIn("_subprocess_error", result,
                         f"Hook subprocess failed: {result}")
        hso = result.get("hookSpecificOutput", {})
        self.assertIn("additionalContext", hso,
                       "Non-replacement adapters should use additionalContext")
        self.assertNotIn("updatedToolOutput", hso,
                         "Non-replacement adapters should not use updatedToolOutput")


if __name__ == "__main__":
    unittest.main()
