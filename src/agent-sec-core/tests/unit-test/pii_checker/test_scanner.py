"""Unit tests for the PII scanner."""

import pytest
from agent_sec_cli.pii_checker.detectors.base import PiiCandidate
from agent_sec_cli.pii_checker.models import PiiFinding
from agent_sec_cli.pii_checker.redactor import redact_text
from agent_sec_cli.pii_checker.scanner import DEFAULT_MAX_BYTES, PiiScanner


@pytest.fixture(autouse=True)
def _isolate_custom_rules_home(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))


def _scan(text: str, **kwargs):
    return PiiScanner().scan(text, **kwargs).to_dict()


def _types(result: dict) -> set[str]:
    return {finding["type"] for finding in result["findings"]}


def test_pass_when_no_findings():
    result = _scan("hello world")
    assert result["ok"] is True
    assert result["verdict"] == "pass"
    assert result["findings"] == []
    assert result["summary"]["custom_rules"]["status"] == "absent"


def test_personal_data_findings_are_warn():
    result = _scan(
        "Contact alice@company.cn, 13800138000, id 11010519491231002X, card 4111111111111111."
    )
    assert result["verdict"] == "warn"
    assert {"email", "phone_cn", "cn_id", "credit_card"}.issubset(_types(result))
    assert {finding["severity"] for finding in result["findings"]} == {"warn"}


def test_cn_id_with_lowercase_x_is_detected():
    result = _scan("id 11010519491231002x")

    assert result["verdict"] == "warn"
    assert "cn_id" in _types(result)


def test_credentials_are_deny():
    token = (
        "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9."
        "eyJzdWIiOiIxMjM0NTY3ODkwIn0."
        "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
    )
    result = _scan(
        "Authorization: Bearer abcdefghijklmnopqrstuvwxyz123456\n"
        f"jwt={token}\n"
        "api_key=sk-abcdefghijklmnopqrstuvwxyz123456\n"
        "accessKeySecret=abcdefghijklmnopqrstuvwxyz123456\n"
        "id=LTAI5tQnKxExampleToken12"
    )
    assert result["verdict"] == "deny"
    assert {"bearer_token", "jwt", "api_key", "aliyun_access_key_secret"}.issubset(
        _types(result)
    )


def test_bearer_jwt_preserves_both_types():
    token = (
        "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9."
        "eyJzdWIiOiIxMjM0NTY3ODkwIn0."
        "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
    )
    result = _scan(f"Authorization: Bearer {token}", redact_output=True)

    assert {"bearer_token", "jwt"}.issubset(_types(result))
    assert result["summary"]["by_type"]["bearer_token"] == 1
    assert result["summary"]["by_type"]["jwt"] == 1
    assert token not in result["redacted_text"]


def test_chinese_secret_field_is_detected_with_high_confidence():
    result = _scan("密码=abcdefghijklmnopqrstuvwxyz123456")

    assert result["verdict"] == "deny"
    assert result["findings"][0]["type"] == "generic_secret_field"
    assert result["findings"][0]["confidence"] >= 0.9
    assert result["findings"][0]["metadata"]["detector"] == "regex"
    assert result["findings"][0]["metadata"]["engine"] == "regex_v1"


def test_custom_detector_can_be_injected():
    class LocalModelDetector:
        name = "local_model"
        engine = "tiny_pii_v0"

        def detect(self, text: str):
            start = text.index("bob@example.com")
            return [
                PiiCandidate(
                    pii_type="email",
                    category="personal_data",
                    severity="warn",
                    confidence=0.99,
                    value="bob@example.com",
                    span=(start, start + len("bob@example.com")),
                    metadata={"model": "tiny-pii"},
                )
            ]

    result = (
        PiiScanner(detectors=[LocalModelDetector()])
        .scan("contact bob@example.com")
        .to_dict()
    )

    assert result["verdict"] == "warn"
    assert result["findings"][0]["type"] == "email"
    assert result["findings"][0]["metadata"]["detector"] == "local_model"
    assert result["findings"][0]["metadata"]["engine"] == "tiny_pii_v0"
    assert result["findings"][0]["metadata"]["model"] == "tiny-pii"
    assert "custom_rules" not in result["summary"]


def test_exact_type_and_span_duplicate_keeps_highest_confidence():
    class DuplicateDetector:
        name = "duplicate"
        engine = "duplicate_v1"

        def detect(self, text: str):
            value = "bob@example.com"
            start = text.index(value)
            common = {
                "pii_type": "email",
                "category": "personal_data",
                "severity": "warn",
                "value": value,
                "span": (start, start + len(value)),
            }
            return [
                PiiCandidate(confidence=0.7, **common),
                PiiCandidate(confidence=0.99, **common),
            ]

    result = (
        PiiScanner(detectors=[DuplicateDetector()])
        .scan("contact bob@example.com")
        .to_dict()
    )

    assert len(result["findings"]) == 1
    assert result["findings"][0]["confidence"] == 0.99


def test_private_key_detected_and_redacted():
    pem = """-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEA0testbody
-----END RSA PRIVATE KEY-----"""
    result = _scan(pem, redact_output=True)
    assert result["verdict"] == "deny"
    assert result["findings"][0]["type"] == "private_key"
    assert result["redacted_text"] == "[REDACTED_PRIVATE_KEY]"


def test_large_private_key_omits_raw_candidate_value():
    pem = (
        "-----BEGIN RSA PRIVATE KEY-----\n"
        + ("A" * 20_000)
        + "\n-----END RSA PRIVATE KEY-----"
    )
    result = _scan(pem, raw_evidence=True)
    finding = result["findings"][0]

    assert finding["type"] == "private_key"
    assert finding["raw_evidence"] == "[PRIVATE_KEY_OMITTED]"
    assert finding["metadata"]["evidence_omitted"] is True
    assert "A" * 100 not in finding["raw_evidence"]


def test_low_confidence_hidden_by_default_and_included_on_request():
    hidden = _scan("example email test@example.invalid")
    shown = _scan("example email test@example.invalid", include_low_confidence=True)

    assert hidden["verdict"] == "pass"
    assert hidden["findings"] == []
    assert shown["verdict"] == "warn"
    assert shown["findings"][0]["type"] == "email"


def test_raw_evidence_default_off_and_opt_in():
    text = "email alice@company.cn"
    default = _scan(text)
    raw = _scan(text, raw_evidence=True)

    assert "raw_evidence" not in default["findings"][0]
    assert raw["findings"][0]["raw_evidence"] == "alice@company.cn"


def test_redacted_text_keeps_structure_without_sensitive_values():
    secret = "password=supersecretvalue12345"
    result = _scan(secret, redact_output=True, raw_evidence=True)

    assert "password=" in result["redacted_text"]
    assert "supersecretvalue12345" not in result["redacted_text"]
    assert "supersecretvalue12345" in result["findings"][0]["raw_evidence"]


def test_redaction_merges_transitive_overlaps_and_uses_stable_custom_type():
    findings = [
        PiiFinding(
            type="api_key",
            category="credential",
            severity="deny",
            confidence=0.9,
            evidence_redacted="abcd...[REDACTED]...wxyz",
            span=(0, 4),
        ),
        PiiFinding(
            type="beta_custom",
            category="custom",
            severity="deny",
            confidence=1.0,
            evidence_redacted="[BETA_CUSTOM_REDACTED]",
            span=(3, 7),
        ),
        PiiFinding(
            type="alpha_custom",
            category="custom",
            severity="deny",
            confidence=1.0,
            evidence_redacted="[ALPHA_CUSTOM_REDACTED]",
            span=(6, 10),
        ),
    ]

    assert redact_text("abcdefghij", findings) == "[ALPHA_CUSTOM_REDACTED]"


def test_redaction_prefers_custom_warn_placeholder_over_builtin_deny():
    findings = [
        PiiFinding(
            type="api_key",
            category="credential",
            severity="deny",
            confidence=0.9,
            evidence_redacted="0123...[REDACTED]...89",
            span=(0, 6),
        ),
        PiiFinding(
            type="custom_field",
            category="custom",
            severity="warn",
            confidence=1.0,
            evidence_redacted="[CUSTOM_FIELD_REDACTED]",
            span=(2, 10),
        ),
    ]

    assert redact_text("0123456789", findings) == "[CUSTOM_FIELD_REDACTED]"


def test_quoted_secret_span_keeps_quote_boundaries_balanced():
    secret = 'password="supersecretvalue12345"'
    result = _scan(secret, redact_output=True, raw_evidence=True)
    finding = result["findings"][0]
    span = finding["span"]

    assert secret[span["start"] : span["end"]] == '"supersecretvalue12345"'
    assert result["redacted_text"].startswith('password="')
    assert result["redacted_text"].endswith('"')
    assert "supersecretvalue12345" not in result["redacted_text"]


def test_max_bytes_truncates_input():
    result = _scan("alice@example.com trailing", max_bytes=5)
    assert result["summary"]["truncated"] is True
    assert result["verdict"] == "pass"


def test_invalid_max_bytes_is_rejected():
    with pytest.raises(ValueError, match="max_bytes must be greater than zero"):
        PiiScanner().scan("alice@example.com", max_bytes=0)


def test_multibyte_truncation_boundary_is_safe():
    max_bytes = len("备注".encode("utf-8")) + 1
    result = _scan("备注🙂 alice@example.com", max_bytes=max_bytes, redact_output=True)

    assert result["summary"]["truncated"] is True
    assert result["summary"]["bytes_scanned"] == max_bytes
    assert result["verdict"] == "pass"
    assert result["redacted_text"] == "备注"


def test_large_input_over_default_limit_scans_tail_by_default():
    email = "alice@company.cn"
    text = f"{'x' * (DEFAULT_MAX_BYTES + 10)} {email}"
    result = _scan(text)

    assert result["summary"]["truncated"] is False
    assert result["summary"]["bytes_scanned"] == len(text.encode("utf-8"))
    assert "email" in _types(result)


def test_explicit_default_limit_truncates_large_input_tail():
    email = "alice@company.cn"
    text = f"{'x' * (DEFAULT_MAX_BYTES + 10)} {email}"
    result = _scan(text, max_bytes=DEFAULT_MAX_BYTES)

    assert result["summary"]["truncated"] is True
    assert result["summary"]["bytes_scanned"] == DEFAULT_MAX_BYTES
    assert "email" not in _types(result)


def test_large_input_near_default_limit_scans_tail():
    email = "alice@company.cn"
    padding = "x" * (DEFAULT_MAX_BYTES - len(email.encode("utf-8")) - 1)
    result = _scan(f"{padding} {email}")

    assert result["summary"]["truncated"] is False
    assert result["summary"]["bytes_scanned"] == DEFAULT_MAX_BYTES
    assert "email" in _types(result)


def test_malformed_private_key_stress_does_not_backtrack_slowly():
    text = (
        "-----BEGIN RSA PRIVATE KEY-----"
        + ("A" * 10_000)
        + "-----END EC PRIVATE KEY-----"
    )

    result = _scan(text)

    assert "private_key" not in _types(result)
