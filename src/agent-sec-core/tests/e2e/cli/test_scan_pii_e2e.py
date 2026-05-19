"""Self-contained e2e tests for the scan-pii CLI."""

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

_MODES = ("binary", "module")


def _module_mode_available() -> bool:
    result = subprocess.run(
        [sys.executable, "-c", "import agent_sec_cli.cli"],
        capture_output=True,
        check=False,
        text=True,
        timeout=10,
    )
    return result.returncode == 0


def _command(mode: str) -> list[str]:
    if mode == "binary":
        return ["agent-sec-cli"]
    if mode == "module":
        if not _module_mode_available():
            pytest.skip(
                "module mode requires agent_sec_cli importable by this Python; "
                "RPM e2e validates the installed agent-sec-cli binary"
            )
        return [sys.executable, "-m", "agent_sec_cli.cli"]
    raise AssertionError(f"unknown CLI mode: {mode}")


def _run_cli(
    mode: str,
    *args: str,
    data_dir: Path,
    input_text: str | None = None,
) -> subprocess.CompletedProcess[str]:
    data_dir.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["AGENT_SEC_DATA_DIR"] = str(data_dir)
    try:
        return subprocess.run(
            [*_command(mode), *args],
            capture_output=True,
            text=True,
            input=input_text,
            check=False,
            timeout=30,
            env=env,
        )
    except FileNotFoundError as exc:
        raise AssertionError("agent-sec-cli binary not found on PATH") from exc


def _load_json(result: subprocess.CompletedProcess[str]) -> dict[str, Any]:
    assert result.returncode == 0, result.stderr
    data = json.loads(result.stdout)
    assert isinstance(data, dict)
    return data


@pytest.mark.parametrize("mode", _MODES)
def test_scan_pii_text_json(mode: str, tmp_path: Path) -> None:
    result = _run_cli(
        mode,
        "scan-pii",
        "--text",
        "Contact alice@securecorp.cn for help.",
        "--source",
        "manual",
        "--format",
        "json",
        data_dir=tmp_path / mode / "text-json",
    )
    data = _load_json(result)

    assert data["ok"] is True
    assert data["verdict"] == "warn"
    assert data["summary"]["source"] == "manual"
    assert any(finding["type"] == "email" for finding in data["findings"])
    assert "redacted_text" not in data
    assert all("raw_evidence" not in finding for finding in data["findings"])


@pytest.mark.parametrize("mode", _MODES)
def test_scan_pii_stdin_json(mode: str, tmp_path: Path) -> None:
    result = _run_cli(
        mode,
        "scan-pii",
        "--stdin",
        "--source",
        "manual",
        "--format",
        "json",
        data_dir=tmp_path / mode / "stdin-json",
        input_text="Contact alice@securecorp.cn for help.",
    )
    data = _load_json(result)

    assert data["ok"] is True
    assert data["verdict"] == "warn"
    assert data["summary"]["source"] == "manual"
    assert any(finding["type"] == "email" for finding in data["findings"])


@pytest.mark.parametrize("mode", _MODES)
def test_scan_pii_stdin_max_bytes_truncates_before_scan(
    mode: str, tmp_path: Path
) -> None:
    max_bytes = len("备注".encode("utf-8")) + 1
    result = _run_cli(
        mode,
        "scan-pii",
        "--stdin",
        "--source",
        "manual",
        "--format",
        "json",
        "--max-bytes",
        str(max_bytes),
        data_dir=tmp_path / mode / "stdin-max-bytes",
        input_text="备注🙂 alice@example.com",
    )
    data = _load_json(result)

    assert data["ok"] is True
    assert data["summary"]["source"] == "manual"
    assert data["summary"]["truncated"] is True
    assert data["summary"]["bytes_scanned"] == max_bytes
    assert not any(finding["type"] == "email" for finding in data["findings"])


@pytest.mark.parametrize("mode", _MODES)
def test_scan_pii_input_file_json(mode: str, tmp_path: Path) -> None:
    input_path = tmp_path / mode / "input.txt"
    input_path.parent.mkdir(parents=True, exist_ok=True)
    input_path.write_text("Phone: 13800138000\n", encoding="utf-8")

    result = _run_cli(
        mode,
        "scan-pii",
        "--input",
        str(input_path),
        "--source",
        "manual",
        "--format",
        "json",
        data_dir=tmp_path / mode / "input-json",
    )
    data = _load_json(result)

    assert data["ok"] is True
    assert data["verdict"] == "warn"
    assert any(finding["type"] == "phone_cn" for finding in data["findings"])
    assert all("raw_evidence" not in finding for finding in data["findings"])


@pytest.mark.parametrize("mode", _MODES)
def test_scan_pii_redact_output_adds_redacted_text(mode: str, tmp_path: Path) -> None:
    secret = "password=supersecretvalue12345"
    result = _run_cli(
        mode,
        "scan-pii",
        "--text",
        secret,
        "--source",
        "manual",
        "--format",
        "json",
        "--redact-output",
        data_dir=tmp_path / mode / "redact-output",
    )
    data = _load_json(result)

    assert data["verdict"] == "deny"
    assert "redacted_text" in data
    assert "supersecretvalue12345" not in data["redacted_text"]
    assert "password=" in data["redacted_text"]


@pytest.mark.parametrize("mode", _MODES)
def test_scan_pii_raw_evidence_stays_out_of_security_events(
    mode: str, tmp_path: Path
) -> None:
    token = "abcdefghijklmnopqrstuvwx12345678"
    text = f"Authorization: Bearer {token}"
    data_dir = tmp_path / mode / "events-sanitized"

    scan_result = _run_cli(
        mode,
        "scan-pii",
        "--text",
        text,
        "--source",
        "tool_output",
        "--format",
        "json",
        "--raw-evidence",
        "--redact-output",
        data_dir=data_dir,
    )
    scan_data = _load_json(scan_result)
    assert any("raw_evidence" in finding for finding in scan_data["findings"])

    events_result = _run_cli(
        mode,
        "events",
        "--category",
        "pii_scan",
        "--output",
        "json",
        data_dir=data_dir,
    )
    assert events_result.returncode == 0, events_result.stderr
    events = json.loads(events_result.stdout)
    assert isinstance(events, list)
    assert len(events) == 1

    event = events[0]
    details = event["details"]
    details_text = json.dumps(details, ensure_ascii=False)
    assert event["category"] == "pii_scan"
    assert details["request"]["source"] == "tool_output"
    assert "text" not in details["request"]
    assert "text_sha256" in details["request"]
    assert "redacted_text" not in details["result"]
    assert all(
        "raw_evidence" not in finding for finding in details["result"]["findings"]
    )
    assert text not in details_text
    assert token not in details_text
