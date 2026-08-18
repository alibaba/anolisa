#!/usr/bin/env python3
"""Tests for compress_toon_hook.py minimum payload threshold.

TOON on small JSON saves only a handful of characters (~0.3% below
~500 chars) while the per-event encode cost stays the same, so the
standalone TOON hook must skip payloads under _MIN_TOON_CHARS (500)
without invoking ``tokenless compress-toon`` — no subprocess, no
encode, no stats noise. Larger payloads keep flowing to TOON.
"""

import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

HOOK = (
    Path(__file__).resolve().parent.parent
    / "adapters"
    / "tokenless"
    / "common"
    / "hooks"
    / "compress_toon_hook.py"
)

_needs_py39 = sys.version_info < (3, 9)


def _create_toon_marker_tokenless(tmpdir: str) -> str:
    """Mock tokenless whose compress-toon records a call marker next to
    itself and emits a smaller TOON-like output, so tests can tell
    whether the hook invoked compress-toon at all."""
    mock_script = os.path.join(tmpdir, "tokenless")
    script = textwrap.dedent("""\
        #!/usr/bin/env python3
        import os, sys
        data = sys.stdin.read()
        if sys.argv[1] == "compress-toon":
            marker = os.path.join(
                os.path.dirname(os.path.abspath(__file__)), "toon_called")
            open(marker, "a").close()
            print("toon:" + data[: len(data) // 2])
    """)
    with open(mock_script, "w") as f:
        f.write(script)
    os.chmod(
        mock_script,
        os.stat(mock_script).st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IEXEC,
    )
    return mock_script


def _run_toon_hook(tool_response, mock_tokenless_path: str,
                   isolated_home: str) -> dict:
    """Run the standalone TOON hook against ``tool_response`` (a JSON
    string or a structured dict/list payload)."""
    env = os.environ.copy()
    env["TOKENLESS_AGENT_ID"] = "copilot-shell"
    env["PATH"] = os.path.dirname(mock_tokenless_path) + ":" + env.get("PATH", "")
    env["HOME"] = isolated_home
    env.pop("COSH_RUNTIME", None)
    env.pop("COSH_NG_VERSION", None)

    stdin_data = {
        "tool_name": "shell",
        "tool_response": tool_response,
        "session_id": "test-session",
        "tool_use_id": "toolu_test",
    }
    proc = subprocess.run(
        [sys.executable, str(HOOK)],
        input=json.dumps(stdin_data),
        capture_output=True,
        text=True,
        timeout=10,
        env=env,
    )
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


def _json_response(char_target: int) -> str:
    """A JSON object string of roughly ``char_target`` characters."""
    inner = "x" * max(char_target - 30, 1)
    return json.dumps({"stdout": inner, "exit_code": 0})


@unittest.skipIf(_needs_py39, "hook_utils requires Python 3.9+")
class CompressToonMinPayloadThresholdTest(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.isolated_home = tempfile.mkdtemp(prefix="test_toon_hook_home_")
        self.mock_bin = _create_toon_marker_tokenless(self.tmpdir)
        self.toon_marker = os.path.join(self.tmpdir, "toon_called")

    def tearDown(self):
        shutil.rmtree(self.tmpdir, ignore_errors=True)
        shutil.rmtree(self.isolated_home, ignore_errors=True)

    def test_small_payload_skipped_without_encoding(self):
        """Under the threshold: no compress-toon call, no output."""
        result = _run_toon_hook(
            _json_response(300), self.mock_bin, self.isolated_home)

        self.assertNotIn("_subprocess_error", result,
                         f"Hook subprocess failed: {result}")
        self.assertFalse(os.path.exists(self.toon_marker),
                         "compress-toon must not run below the threshold")
        self.assertEqual(result, {},
                         "Small payloads pass through untouched")

    def test_large_payload_still_encoded(self):
        """At/above the threshold: compress-toon runs and output is used."""
        result = _run_toon_hook(
            _json_response(800), self.mock_bin, self.isolated_home)

        self.assertNotIn("_subprocess_error", result,
                         f"Hook subprocess failed: {result}")
        self.assertTrue(os.path.exists(self.toon_marker),
                        "compress-toon must run for large payloads")
        hso = result.get("hookSpecificOutput", {})
        self.assertEqual(hso.get("hookEventName"), "PostToolUse")
        self.assertTrue(
            str(hso.get("additionalContext", "")).startswith("toon:"),
            "TOON output should be emitted for large payloads")

    def test_small_structured_non_bmp_payload_skipped(self):
        """Structured non-BMP payloads are gated by Unicode character count.

        {"stdout": "😀" × 40, ...} is ~67 Unicode characters but 507
        characters once serialized with \\u escapes; counting the escaped
        form would wrongly spawn compress-toon.
        """
        result = _run_toon_hook(
            {"stdout": "😀" * 40, "exit_code": 0},
            self.mock_bin, self.isolated_home)

        self.assertNotIn("_subprocess_error", result,
                         f"Hook subprocess failed: {result}")
        self.assertFalse(os.path.exists(self.toon_marker),
                         "compress-toon must not run for a structured payload "
                         "whose character count is below the threshold")
        self.assertEqual(result, {},
                         "Small payloads pass through untouched")

    def test_large_structured_non_bmp_payload_still_encoded(self):
        """520 emoji ≈ 547 Unicode chars: above threshold, TOON runs."""
        result = _run_toon_hook(
            {"stdout": "😀" * 520, "exit_code": 0},
            self.mock_bin, self.isolated_home)

        self.assertNotIn("_subprocess_error", result,
                         f"Hook subprocess failed: {result}")
        self.assertTrue(os.path.exists(self.toon_marker),
                        "compress-toon must run at/above the character "
                        "threshold for structured non-BMP payloads")
        hso = result.get("hookSpecificOutput", {})
        self.assertEqual(hso.get("hookEventName"), "PostToolUse")
        self.assertTrue(
            str(hso.get("additionalContext", "")).startswith("toon:"),
            "TOON output should be emitted for large payloads")

    def test_small_string_wrapped_non_bmp_payload_skipped(self):
        """String-wrapped JSON takes the unwrap_string_json path.

        A wrapped {"stdout": "😀" × 40, ...} payload is ~67 Unicode
        characters once unwrapped (wrapped input ~73), but re-serializing
        the inner object with ASCII escapes inflates it to 507 characters;
        the gate must count the unwrapped code points and skip TOON.
        """
        inner = json.dumps({"stdout": "😀" * 40, "exit_code": 0},
                           ensure_ascii=False)
        wrapped = json.dumps(inner, ensure_ascii=False)
        result = _run_toon_hook(wrapped, self.mock_bin, self.isolated_home)

        self.assertNotIn("_subprocess_error", result,
                         f"Hook subprocess failed: {result}")
        self.assertFalse(os.path.exists(self.toon_marker),
                         "compress-toon must not run for a string-wrapped "
                         "payload whose character count is below the threshold")
        self.assertEqual(result, {},
                         "Small payloads pass through untouched")

    def test_large_string_wrapped_non_bmp_payload_still_encoded(self):
        """Wrapped payload unwrapping to ~547 Unicode chars: TOON runs."""
        inner = json.dumps({"stdout": "😀" * 520, "exit_code": 0},
                           ensure_ascii=False)
        wrapped = json.dumps(inner, ensure_ascii=False)
        result = _run_toon_hook(wrapped, self.mock_bin, self.isolated_home)

        self.assertNotIn("_subprocess_error", result,
                         f"Hook subprocess failed: {result}")
        self.assertTrue(os.path.exists(self.toon_marker),
                        "compress-toon must run at/above the character "
                        "threshold for string-wrapped non-BMP payloads")
        hso = result.get("hookSpecificOutput", {})
        self.assertEqual(hso.get("hookEventName"), "PostToolUse")
        self.assertTrue(
            str(hso.get("additionalContext", "")).startswith("toon:"),
            "TOON output should be emitted for large payloads")


if __name__ == "__main__":
    unittest.main()
