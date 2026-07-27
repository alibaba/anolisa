"""Built-in regex and validator based PII detector."""

import re

from agent_sec_cli.pii_checker.detectors.base import PiiCandidate
from agent_sec_cli.pii_checker.models import PiiCategory, PiiSeverity
from agent_sec_cli.pii_checker.validators import (
    luhn_check,
    validate_cn_id,
    validate_jwt,
)

_CONTEXT_WINDOW_RADIUS = 64
_CONTEXT_POSITIVE_DELTA = 0.12
_CONTEXT_NEGATIVE_DELTA = -0.35
_MAX_PRIVATE_KEY_CHARS = 16_384
_PRIVATE_KEY_EVIDENCE_PLACEHOLDER = "[PRIVATE_KEY_OMITTED]"

BUILTIN_PII_TYPES = frozenset(
    {
        "aliyun_access_key_id",
        "aliyun_access_key_secret",
        "api_key",
        "bearer_token",
        "cn_id",
        "credit_card",
        "email",
        "generic_secret_field",
        "jwt",
        "phone_cn",
        "private_key",
    }
)

# Confidence model (v1 fixed heuristic; values are not calibrated probabilities):
#
# | Signal class                        | Base |
# | ----------------------------------- | ---- |
# | private_key                         | 1.00 |
# | jwt                                 | 0.94 |
# | cn_id                               | 0.93 |
# | bearer_token, aliyun_access_key_id  | 0.92 |
# | credit_card with Luhn validation    | 0.92 |
# | api_key prefix patterns             | 0.86 |
# | generic_secret_field, email         | 0.82 |
# | phone_cn                            | 0.78 |
# | reserved .invalid email             | 0.35 |
#
# Context adjustment uses a 64-character window around each match. Security
# keywords raise credential-like matches by +0.12; fixture/example markers lower
# likely test data by -0.35. Scanner-level thresholding hides low-confidence
# findings unless include_low_confidence is enabled.
_BASE_CONFIDENCE: dict[str, float] = {
    "private_key": 1.0,
    "jwt": 0.94,
    "cn_id": 0.93,
    "bearer_token": 0.92,
    "aliyun_access_key_id": 0.92,
    "credit_card": 0.92,
    "api_key": 0.86,
    "generic_secret_field": 0.82,
    "email": 0.82,
    "phone_cn": 0.78,
    "email_reserved_invalid": 0.35,
}

_POSITIVE_CONTEXT = (
    "password",
    "secret",
    "token",
    "api_key",
    "apikey",
    "authorization",
    "bearer",
    "accesskeysecret",
    "access_key_secret",
    "密码",
    "口令",
    "密钥",
    "令牌",
    "授权",
    "访问密钥",
)
_NEGATIVE_CONTEXT = ("example", "dummy", "test", "sample", ".invalid")

_EMAIL_RE = re.compile(
    r"(?<![\w.+-])[A-Za-z0-9._%+-]{1,64}@[A-Za-z0-9.-]+\.[A-Za-z]{2,63}(?![\w.-])"
)
_PHONE_CN_RE = re.compile(
    r"(?<!\d)(?:\+?86[-\s]?)?1[3-9]\d[-\s]?\d{4}[-\s]?\d{4}(?!\d)"
)
_CREDIT_CARD_RE = re.compile(r"(?<!\d)(?:\d[ -]?){13,19}(?!\d)")
_CN_ID_RE = re.compile(r"(?<!\d)\d{17}[\dXx](?!\w)")
_JWT_RE = re.compile(
    r"(?<![A-Za-z0-9_-])[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}(?![A-Za-z0-9_-])"
)
_API_KEY_RE = re.compile(
    r"\b(?:sk|pk|rk|gh[pousr]|xox[baprs])[-_][A-Za-z0-9_=-]{16,}\b"
)
_BEARER_RE = re.compile(r"\bBearer\s+([A-Za-z0-9._~+/=-]{16,})", re.IGNORECASE)
_PRIVATE_KEY_RE = re.compile(
    r"-----BEGIN ([A-Z0-9 ]*PRIVATE KEY)-----[\s\S]+?-----END \1-----"
)
_ALIYUN_ACCESS_KEY_ID_RE = re.compile(r"\bLTAI[A-Za-z0-9]{12,30}\b")
_SECRET_FIELD_RE = re.compile(
    r"(?i)(?<![\w\u4e00-\u9fff])(?P<name>password|passwd|secret|token|"
    r"api[_-]?key|apikey|access[_-]?key[_-]?secret|accessKeySecret|"
    r"client[_-]?secret|authorization|密码|口令|密钥|令牌|授权|访问密钥)"
    r"\s*[:=：]\s*(?P<quoted_value>\"(?P<double_value>[^\s\"',;，；：]{8,})\"|"
    r"'(?P<single_value>[^\s\"',;，；：]{8,})'|"
    r"(?P<bare_value>[^\s\"',;，；：]{8,}))"
)


def _context_window(
    text: str, start: int, end: int, radius: int = _CONTEXT_WINDOW_RADIUS
) -> str:
    """Return lowercase context around a match."""
    return text[max(0, start - radius) : min(len(text), end + radius)].lower()


def _score_with_context(text: str, start: int, end: int, base: float) -> float:
    """Adjust confidence up/down based on surrounding context."""
    context = _context_window(text, start, end)
    score = base
    compact_context = context.replace("-", "_")
    if any(marker in compact_context for marker in _POSITIVE_CONTEXT):
        score += _CONTEXT_POSITIVE_DELTA
    if any(marker in compact_context for marker in _NEGATIVE_CONTEXT):
        score += _CONTEXT_NEGATIVE_DELTA
    return max(0.0, min(1.0, score))


def _severity_for(pii_type: str) -> tuple[str, str]:
    """Return category and severity for a finding type."""
    if pii_type in {"email", "phone_cn", "credit_card", "cn_id"}:
        return PiiCategory.PERSONAL_DATA.value, PiiSeverity.WARN.value
    return PiiCategory.CREDENTIAL.value, PiiSeverity.DENY.value


class RegexPiiDetector:
    """Built-in detector using regexes, validators, and context scoring."""

    name = "regex"
    engine = "regex_v1"

    def detect(self, text: str) -> list[PiiCandidate]:
        """Run all regex-backed detectors and return raw candidates."""
        candidates: list[PiiCandidate] = []
        self._detect_private_keys(text, candidates)
        self._detect_bearer_tokens(text, candidates)
        self._detect_secret_fields(text, candidates)
        self._detect_api_keys(text, candidates)
        self._detect_aliyun_access_key_ids(text, candidates)
        self._detect_jwts(text, candidates)
        self._detect_credit_cards(text, candidates)
        self._detect_cn_ids(text, candidates)
        self._detect_phone_numbers(text, candidates)
        self._detect_emails(text, candidates)
        return candidates

    def _add_candidate(
        self,
        candidates: list[PiiCandidate],
        *,
        pii_type: str,
        value: str,
        span: tuple[int, int],
        confidence: float,
        metadata: dict[str, object] | None = None,
    ) -> None:
        """Append a candidate with type-derived category and severity."""
        category, severity = _severity_for(pii_type)
        candidates.append(
            PiiCandidate(
                pii_type=pii_type,
                category=category,
                severity=severity,
                confidence=confidence,
                value=value,
                span=span,
                metadata=metadata or {},
                detector=self.name,
                engine=self.engine,
            )
        )

    def _detect_private_keys(self, text: str, candidates: list[PiiCandidate]) -> None:
        for match in _PRIVATE_KEY_RE.finditer(text):
            span = match.span()
            if span[1] - span[0] > _MAX_PRIVATE_KEY_CHARS:
                self._add_candidate(
                    candidates,
                    pii_type="private_key",
                    value=_PRIVATE_KEY_EVIDENCE_PLACEHOLDER,
                    span=span,
                    confidence=_BASE_CONFIDENCE["private_key"],
                    metadata={
                        "validator": "pem_private_key",
                        "evidence_omitted": True,
                    },
                )
                continue

            value = match.group(0)
            self._add_candidate(
                candidates,
                pii_type="private_key",
                value=value,
                span=span,
                confidence=_BASE_CONFIDENCE["private_key"],
                metadata={"validator": "pem_private_key"},
            )

    def _detect_bearer_tokens(self, text: str, candidates: list[PiiCandidate]) -> None:
        for match in _BEARER_RE.finditer(text):
            value = match.group(1)
            span = match.span(1)
            self._add_candidate(
                candidates,
                pii_type="bearer_token",
                value=value,
                span=span,
                confidence=_score_with_context(
                    text, *span, _BASE_CONFIDENCE["bearer_token"]
                ),
                metadata={"context": "bearer"},
            )

    def _detect_secret_fields(self, text: str, candidates: list[PiiCandidate]) -> None:
        for match in _SECRET_FIELD_RE.finditer(text):
            field_name = match.group("name")
            value = (
                match.group("double_value")
                or match.group("single_value")
                or match.group("bare_value")
            )
            evidence_value = match.group("quoted_value")
            if value is None:
                continue
            if len(value) < 12 and not field_name.lower().startswith("accesskey"):
                continue
            normalized_name = field_name.lower().replace("-", "_")
            compact_name = normalized_name.replace("_", "")
            if compact_name == "accesskeysecret":
                pii_type = "aliyun_access_key_secret"
            elif compact_name in {"apikey", "api_key"}:
                pii_type = "api_key"
            else:
                pii_type = "generic_secret_field"
            span = match.span("quoted_value")
            self._add_candidate(
                candidates,
                pii_type=pii_type,
                value=evidence_value,
                span=span,
                confidence=_score_with_context(
                    text, *span, _BASE_CONFIDENCE["generic_secret_field"]
                ),
                metadata={"field": field_name},
            )

    def _detect_api_keys(self, text: str, candidates: list[PiiCandidate]) -> None:
        for match in _API_KEY_RE.finditer(text):
            self._add_candidate(
                candidates,
                pii_type="api_key",
                value=match.group(0),
                span=match.span(),
                confidence=_score_with_context(
                    text, *match.span(), _BASE_CONFIDENCE["api_key"]
                ),
                metadata={"pattern": "token_prefix"},
            )

    def _detect_aliyun_access_key_ids(
        self, text: str, candidates: list[PiiCandidate]
    ) -> None:
        for match in _ALIYUN_ACCESS_KEY_ID_RE.finditer(text):
            self._add_candidate(
                candidates,
                pii_type="aliyun_access_key_id",
                value=match.group(0),
                span=match.span(),
                confidence=_score_with_context(
                    text, *match.span(), _BASE_CONFIDENCE["aliyun_access_key_id"]
                ),
            )

    def _detect_jwts(self, text: str, candidates: list[PiiCandidate]) -> None:
        if text.count(".") < 2:
            return
        for match in _JWT_RE.finditer(text):
            value = match.group(0)
            if validate_jwt(value):
                self._add_candidate(
                    candidates,
                    pii_type="jwt",
                    value=value,
                    span=match.span(),
                    confidence=_score_with_context(
                        text, *match.span(), _BASE_CONFIDENCE["jwt"]
                    ),
                    metadata={"validator": "jwt_structure"},
                )

    def _detect_credit_cards(self, text: str, candidates: list[PiiCandidate]) -> None:
        for match in _CREDIT_CARD_RE.finditer(text):
            value = match.group(0)
            if luhn_check(value):
                self._add_candidate(
                    candidates,
                    pii_type="credit_card",
                    value=value,
                    span=match.span(),
                    confidence=_score_with_context(
                        text, *match.span(), _BASE_CONFIDENCE["credit_card"]
                    ),
                    metadata={"validator": "luhn"},
                )

    def _detect_cn_ids(self, text: str, candidates: list[PiiCandidate]) -> None:
        for match in _CN_ID_RE.finditer(text):
            value = match.group(0)
            if validate_cn_id(value):
                self._add_candidate(
                    candidates,
                    pii_type="cn_id",
                    value=value,
                    span=match.span(),
                    confidence=_score_with_context(
                        text, *match.span(), _BASE_CONFIDENCE["cn_id"]
                    ),
                    metadata={"validator": "cn_id_checksum"},
                )

    def _detect_phone_numbers(self, text: str, candidates: list[PiiCandidate]) -> None:
        for match in _PHONE_CN_RE.finditer(text):
            value = match.group(0)
            self._add_candidate(
                candidates,
                pii_type="phone_cn",
                value=value,
                span=match.span(),
                confidence=_score_with_context(
                    text, *match.span(), _BASE_CONFIDENCE["phone_cn"]
                ),
            )

    def _detect_emails(self, text: str, candidates: list[PiiCandidate]) -> None:
        for match in _EMAIL_RE.finditer(text):
            value = match.group(0)
            base = _BASE_CONFIDENCE["email"]
            if value.lower().endswith(".invalid"):
                base = _BASE_CONFIDENCE["email_reserved_invalid"]
            self._add_candidate(
                candidates,
                pii_type="email",
                value=value,
                span=match.span(),
                confidence=_score_with_context(text, *match.span(), base),
            )
