#!/usr/bin/env python3
"""Integration tests for compress_schema_hook.py.

Validates the BeforeModel hook contract:
- Tool declarations are read from the canonical ``llm_request.config.tools``
  position, with the older top-level ``llm_request.tools`` still accepted.
- Compressed declarations are written back to the canonical position.
- Stats attribution: when the host announces itself via ``COSH_RUNTIME`` /
  ``COSH_NG_VERSION``, the agent ID is ``cosh-ng`` even though the shared
  extension manifest sets ``TOKENLESS_AGENT_ID=copilot-shell``.

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

_TOOLS = [
    {
        "name": "shell",
        "description": "Run a shell command. " * 20,
        "parameters": {
            "type": "object",
            "properties": {"command": {"type": "string"}},
        },
    }
]


def _create_mock_tokenless(tmpdir: str, argv_log: str) -> str:
    """Mock `tokenless compress-schema` that truncates descriptions.

    Records its own argv to ``argv_log`` so tests can assert the agent ID the
    hook attributed the invocation to.
    """
    mock_script = os.path.join(tmpdir, "tokenless")
    script = textwrap.dedent(f"""\
        #!/usr/bin/env python3
        import json, sys
        with open({argv_log!r}, "w") as log:
            log.write(json.dumps(sys.argv[1:]))
        tools = json.loads(sys.stdin.read())
        for tool in tools:
            tool["description"] = "compressed"
        print(json.dumps(tools))
    """)
    with open(mock_script, "w") as handle:
        handle.write(script)
    os.chmod(
        mock_script,
        os.stat(mock_script).st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH,
    )
    return mock_script


def _run_hook(stdin_data: dict, mock_tokenless_path: str, extra_env: dict) -> dict:
    """Run the hook as a subprocess with a mocked tokenless binary."""
    hooks_dir = os.path.normpath(os.path.join(
        os.path.dirname(__file__),
        os.pardir, "adapters", "tokenless", "common", "hooks",
    ))
    hook_path = os.path.join(hooks_dir, "compress_schema_hook.py")

    env = os.environ.copy()
    # The host-owned variables must not leak in from the caller's environment.
    env.pop("COSH_RUNTIME", None)
    env.pop("COSH_NG_VERSION", None)
    env["PATH"] = os.path.dirname(mock_tokenless_path) + ":" + env.get("PATH", "")
    env.update(extra_env)

    proc = subprocess.run(
        [sys.executable, hook_path],
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
class TestSchemaCompressionProtocol(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.argv_log = os.path.join(self.tmpdir, "argv.json")
        self.mock_bin = _create_mock_tokenless(self.tmpdir, self.argv_log)

    def tearDown(self):
        shutil.rmtree(self.tmpdir, ignore_errors=True)

    def _recorded_agent_id(self) -> str:
        with open(self.argv_log) as handle:
            argv = json.load(handle)
        return argv[argv.index("--agent-id") + 1]

    def test_reads_and_writes_canonical_config_tools(self):
        result = _run_hook(
            {"session_id": "s1", "llm_request": {"config": {"tools": _TOOLS}}},
            self.mock_bin,
            {"TOKENLESS_AGENT_ID": "copilot-shell"},
        )

        tools = result["hookSpecificOutput"]["llm_request"]["config"]["tools"]
        self.assertEqual(tools[0]["name"], "shell")
        self.assertEqual(tools[0]["description"], "compressed")

    def test_accepts_legacy_top_level_tools(self):
        result = _run_hook(
            {"session_id": "s1", "llm_request": {"tools": _TOOLS}},
            self.mock_bin,
            {"TOKENLESS_AGENT_ID": "copilot-shell"},
        )

        # Read from the legacy position, but always written back to the
        # canonical one.
        tools = result["hookSpecificOutput"]["llm_request"]["config"]["tools"]
        self.assertEqual(tools[0]["description"], "compressed")

    def test_canonical_position_wins_over_legacy_when_both_present(self):
        result = _run_hook(
            {
                "session_id": "s1",
                "llm_request": {
                    "config": {"tools": _TOOLS},
                    "tools": [{
                        "name": "legacy-only",
                        "description": "stale",
                        "parameters": {"type": "object"},
                    }],
                },
            },
            self.mock_bin,
            {"TOKENLESS_AGENT_ID": "copilot-shell"},
        )

        tools = result["hookSpecificOutput"]["llm_request"]["config"]["tools"]
        self.assertEqual([tool["name"] for tool in tools], ["shell"])

    def test_empty_canonical_tools_does_not_fall_back_to_legacy(self):
        """An explicitly empty canonical array means "no tools this request"."""
        result = _run_hook(
            {
                "session_id": "s1",
                "llm_request": {
                    "config": {"tools": []},
                    "tools": _TOOLS,
                },
            },
            self.mock_bin,
            {"TOKENLESS_AGENT_ID": "copilot-shell"},
        )

        self.assertEqual(result, {})

    def test_skips_when_no_tools_present(self):
        result = _run_hook(
            {"session_id": "s1", "llm_request": {"model": "m", "messages": []}},
            self.mock_bin,
            {"TOKENLESS_AGENT_ID": "copilot-shell"},
        )

        self.assertEqual(result, {})

    def test_cosh_ng_runtime_wins_over_manifest_agent_id(self):
        _run_hook(
            {"session_id": "s1", "llm_request": {"config": {"tools": _TOOLS}}},
            self.mock_bin,
            {
                "TOKENLESS_AGENT_ID": "copilot-shell",
                "COSH_RUNTIME": "cosh-ng",
                "COSH_NG_VERSION": "0.13.0",
            },
        )

        self.assertEqual(self._recorded_agent_id(), "cosh-ng")

    def test_manifest_agent_id_still_serves_copilot_shell(self):
        _run_hook(
            {"session_id": "s1", "llm_request": {"config": {"tools": _TOOLS}}},
            self.mock_bin,
            {"TOKENLESS_AGENT_ID": "copilot-shell"},
        )

        self.assertEqual(self._recorded_agent_id(), "copilot-shell")


if __name__ == "__main__":
    unittest.main()
