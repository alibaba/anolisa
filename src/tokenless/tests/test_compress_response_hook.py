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

import json
import os
import stat
import subprocess
import sys
import shutil
import tempfile
import textwrap
import unittest


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
