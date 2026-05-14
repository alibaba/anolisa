"""Unit tests for security_middleware.backends.pii_scan."""

import json

from agent_sec_cli.security_middleware.backends.pii_scan import PiiScanBackend
from agent_sec_cli.security_middleware.context import RequestContext


def test_backend_returns_json_result():
    backend = PiiScanBackend()
    result = backend.execute(
        RequestContext(action="pii_scan"),
        text="email alice@company.cn",
        source="manual",
    )

    assert result.success is True
    assert result.exit_code == 0
    parsed = json.loads(result.stdout)
    assert parsed == result.data
    assert parsed["verdict"] == "warn"


def test_backend_redact_output():
    backend = PiiScanBackend()
    result = backend.execute(
        RequestContext(action="pii_scan"),
        text="password=supersecretvalue12345",
        redact_output=True,
    )

    assert result.data["verdict"] == "deny"
    assert "redacted_text" in result.data
    assert "supersecretvalue12345" not in result.data["redacted_text"]


def test_backend_rejects_invalid_max_bytes():
    backend = PiiScanBackend()
    result = backend.execute(RequestContext(action="pii_scan"), text="x", max_bytes=0)

    assert result.success is False
    assert result.exit_code == 1
    assert result.data["verdict"] == "error"
    assert result.data["summary"]["error_type"] == "ValueError"


def test_backend_rejects_non_integer_max_bytes():
    backend = PiiScanBackend()
    result = backend.execute(
        RequestContext(action="pii_scan"),
        text="x",
        max_bytes="not-an-int",
    )

    assert result.success is False
    assert result.exit_code == 1
    assert result.data["summary"]["error_type"] == "ValueError"


def test_backend_error_preserves_error_type_without_traceback(monkeypatch):
    def fail_scan(self, text, **kwargs):
        raise RuntimeError("scanner failed")

    monkeypatch.setattr(
        "agent_sec_cli.security_middleware.backends.pii_scan.PiiScanner.scan",
        fail_scan,
    )

    backend = PiiScanBackend()
    result = backend.execute(RequestContext(action="pii_scan"), text="hello")

    assert result.success is False
    assert result.exit_code == 1
    assert result.data["summary"]["error_type"] == "RuntimeError"
    assert result.error == "pii_scan error: scanner failed"
    assert "Traceback" not in result.stdout


def test_backend_audit_details_omit_exception_text_with_input(monkeypatch):
    sensitive = "alice@example.com"

    def fail_scan(self, text, **kwargs):
        raise RuntimeError(f"scanner failed on {sensitive}")

    monkeypatch.setattr(
        "agent_sec_cli.security_middleware.backends.pii_scan.PiiScanner.scan",
        fail_scan,
    )

    backend = PiiScanBackend()
    result = backend.execute(RequestContext(action="pii_scan"), text=sensitive)
    details = backend.build_event_details(result, {"text": sensitive})
    details_text = json.dumps(details, ensure_ascii=False)

    assert sensitive not in details_text
    assert details["result"]["summary"]["error"] == (
        "pii_scan error details omitted from audit"
    )
    assert details["result"]["summary"]["error_type"] == "RuntimeError"


def test_backend_error_audit_details_omit_exception_text_with_input():
    sensitive = "alice@example.com"
    backend = PiiScanBackend()
    details = backend.build_error_details(
        RuntimeError(f"router failed on {sensitive}"),
        {"text": sensitive},
    )
    details_text = json.dumps(details, ensure_ascii=False)

    assert sensitive not in details_text
    assert details["error"] == "pii_scan error details omitted from audit"
    assert details["error_type"] == "RuntimeError"
