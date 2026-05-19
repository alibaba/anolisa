"""Unit tests for cosh-extension/hooks/sandbox-guard.py."""

import importlib.util
import json
import sys
from pathlib import Path
from types import SimpleNamespace

_COSH_EXTENSION_DIR = Path(__file__).resolve().parents[2] / ".." / "cosh-extension"
_HOOKS_DIR = _COSH_EXTENSION_DIR / "hooks"
sys.path.insert(0, str(_HOOKS_DIR))


def _load_sandbox_guard_hook():
    hook_path = _HOOKS_DIR / "sandbox-guard.py"
    spec = importlib.util.spec_from_file_location("sandbox_guard_hook", hook_path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_sandbox_guard_log_injects_trace_context_into_logging_command(monkeypatch):
    sandbox_guard = _load_sandbox_guard_hook()
    calls = []

    def fake_popen(cmd, **kwargs):
        calls.append((cmd, kwargs))
        return SimpleNamespace()

    monkeypatch.setattr(
        sandbox_guard.shutil,
        "which",
        lambda name: "agent-sec-cli" if name == "agent-sec-cli" else None,
    )
    monkeypatch.setattr(sandbox_guard.subprocess, "Popen", fake_popen)

    sandbox_guard._log_sandbox_event(
        {
            "session_id": "session-1",
            "run_id": "run-1",
            "toolUseId": "tool-1",
        },
        decision="sandbox",
        command="rm -rf build",
    )

    expected_context = json.dumps(
        {
            "session_id": "session-1",
            "run_id": "run-1",
            "tool_call_id": "tool-1",
        },
        ensure_ascii=False,
        separators=(",", ":"),
    )
    assert calls[0][0][:3] == [
        "agent-sec-cli",
        "--trace-context",
        expected_context,
    ]
    assert calls[0][0][3:] == [
        "log-sandbox",
        "--decision",
        "sandbox",
        "--command",
        "rm -rf build",
    ]
