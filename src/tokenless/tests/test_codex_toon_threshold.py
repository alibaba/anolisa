#!/usr/bin/env python3
"""Tests for the Codex adapter TOON minimum payload threshold.

TOON on small JSON saves only a handful of characters (~0.3% below
~500 chars) while the per-event encode cost stays the same, so the
Codex compression script must skip the TOON step for payloads under
``MIN_TOON_CHARS`` (500) — e.g. when response compression shrinks a
larger input below the threshold — without invoking
``tokenless compress-toon``. Larger payloads keep flowing to TOON.
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

SCRIPT = (
    Path(__file__).resolve().parent.parent
    / "adapters"
    / "tokenless"
    / "codex"
    / "scripts"
    / "compress-response"
)

_needs_py39 = sys.version_info < (3, 9)


def _create_marker_tokenless(tmpdir: str, compressed_len: int) -> str:
    """Mock tokenless CLI.

    - ``compress-response`` echoes a valid JSON object of roughly
      ``compressed_len`` characters (simulating a compressed result).
    - ``compress-toon`` records a call marker and emits a smaller
      TOON-like output, so tests can tell whether the script invoked
      compress-toon at all.
    """
    mock_script = os.path.join(tmpdir, "tokenless")
    script = textwrap.dedent(f"""\
        #!/usr/bin/env python3
        import os, sys
        data = sys.stdin.read()
        if sys.argv[1] == "compress-response":
            inner = "y" * max({compressed_len} - 30, 1)
            import json as _json
            print(_json.dumps({{"stdout": inner, "exit_code": 0}}))
        elif sys.argv[1] == "compress-toon":
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


def _run_codex_hook(tool_response: str, mock_bin: str, tmpdir: str) -> dict:
    """Run the codex compress-response hook against ``tool_response``."""
    env = os.environ.copy()
    env["TOKENLESS_BIN"] = mock_bin
    env["TOKENLESS_AGENT_ID"] = "codex"
    env["HOME"] = tmpdir

    stdin_data = {
        "tool_name": "shell",
        "tool_response": tool_response,
        "session_id": "test-session",
        "tool_use_id": "toolu_test",
    }
    proc = subprocess.run(
        [sys.executable, str(SCRIPT)],
        input=json.dumps(stdin_data),
        capture_output=True,
        text=True,
        timeout=15,
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


@unittest.skipIf(_needs_py39, "codex hook requires Python 3.9+")
class CodexToonMinPayloadThresholdTest(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.mkdtemp(prefix="test_codex_toon_")
        self.addCleanup(shutil.rmtree, self.tmpdir, ignore_errors=True)

    def test_compressed_result_below_threshold_skips_toon(self):
        """Input >= 500 but compressed result < 500: no compress-toon call."""
        mock_bin = _create_marker_tokenless(self.tmpdir, compressed_len=300)
        marker = os.path.join(self.tmpdir, "toon_called")

        result = _run_codex_hook(_json_response(700), mock_bin, self.tmpdir)

        self.assertNotIn("_subprocess_error", result,
                         f"Hook subprocess failed: {result}")
        self.assertFalse(os.path.exists(marker),
                         "compress-toon must not run below the threshold")
        # The compressed form is still injected.
        ctx = result.get("hookSpecificOutput", {}).get("additionalContext", "")
        self.assertIn("[tokenless:compressed]", ctx,
                      f"Expected compressed context, got: {ctx!r}")

    def test_compressed_result_above_threshold_still_toon_encoded(self):
        """Compressed result >= 500: compress-toon runs and output is used."""
        mock_bin = _create_marker_tokenless(self.tmpdir, compressed_len=800)
        marker = os.path.join(self.tmpdir, "toon_called")

        result = _run_codex_hook(_json_response(1200), mock_bin, self.tmpdir)

        self.assertNotIn("_subprocess_error", result,
                         f"Hook subprocess failed: {result}")
        self.assertTrue(os.path.exists(marker),
                        "compress-toon must run for large payloads")


if __name__ == "__main__":
    unittest.main(verbosity=2)
