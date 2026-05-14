"""Redaction helpers for PII findings."""

import re

from agent_sec_cli.pii_checker.models import PiiFinding


def _mask_middle(value: str, *, prefix: int = 4, suffix: int = 4) -> str:
    """Keep a short safe prefix/suffix and mask the middle."""
    if len(value) <= prefix + suffix:
        return "[REDACTED]"
    return f"{value[:prefix]}...[REDACTED]...{value[-suffix:]}"


def redact_value(value: str, pii_type: str) -> str:
    """Return a model-safe redaction for a detected value."""
    if pii_type == "email":
        local, _, domain = value.partition("@")
        if not domain:
            return "[REDACTED_EMAIL]"
        safe_local = local[:1] + "***" if local else "***"
        return f"{safe_local}@{domain}"

    if pii_type == "phone_cn":
        digits = re.sub(r"\D", "", value)
        if len(digits) >= 11:
            core = digits[-11:]
            return f"{core[:3]}****{core[-4:]}"
        return "[REDACTED_PHONE]"

    if pii_type == "credit_card":
        digits = re.sub(r"\D", "", value)
        return (
            f"[REDACTED_CARD:{digits[-4:]}]" if len(digits) >= 4 else "[REDACTED_CARD]"
        )

    if pii_type == "cn_id":
        return (
            f"{value[:3]}***********{value[-4:]}"
            if len(value) >= 7
            else "[REDACTED_CN_ID]"
        )

    if pii_type == "private_key":
        return "[REDACTED_PRIVATE_KEY]"

    if pii_type in {
        "api_key",
        "bearer_token",
        "jwt",
        "aliyun_access_key_id",
        "aliyun_access_key_secret",
        "generic_secret_field",
    }:
        return _mask_middle(value)

    return "[REDACTED]"


def redact_text(text: str, findings: list[PiiFinding]) -> str:
    """Replace finding spans with their redacted evidence."""
    redacted = text
    replaced_spans: list[tuple[int, int]] = []
    for finding in sorted(findings, key=lambda item: item.span[0], reverse=True):
        start, end = finding.span
        if any(
            start < prior_end and prior_start < end
            for prior_start, prior_end in replaced_spans
        ):
            continue
        redacted = redacted[:start] + finding.evidence_redacted + redacted[end:]
        replaced_spans.append((start, end))
    return redacted
