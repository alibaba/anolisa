"""Input preprocessor – normalisation, decoding, and language detection.

Processing pipeline:
    1. Unicode normalisation (NFKC) – unify homoglyphs, fullwidth chars,
       and compatibility characters.
    2. Whitespace normalisation – collapse excess whitespace, strip
       zero-width and invisible control characters.
    3. Encoding detection & decoding – heuristic detection of Base64,
       ROT13, URL-encoding, hex; decoded text is appended as *variants*
       so the rule engine can scan both the original and decoded forms.
    4. Language detection – lightweight heuristic (no external deps);
       records detected language code in metadata.
"""

import base64
import re
import unicodedata
from typing import Any
from urllib.parse import unquote

from pydantic import BaseModel, Field

# ---------------------------------------------------------------------------
# Zero-width / invisible characters that should be stripped from input.
# These are detected by INJ-008 / INJ-009 rules; after rule-engine scanning
# they are also removed so downstream layers see clean text.
# ---------------------------------------------------------------------------
_ZERO_WIDTH_RE = re.compile(
    r"[\u200b\u200c\u200d\u2060\ufeff"
    r"\u2062\u2063\u2064"
    r"\u00ad"
    r"\U000e0001-\U000e007f]+"
)

# Collapse any run of horizontal whitespace / mixed newlines to a single space
_MULTI_SPACE_RE = re.compile(r"[\t\r\f\v ]+")
# Collapse multiple consecutive newlines to at most two
_MULTI_NL_RE = re.compile(r"\n{3,}")

# ---------------------------------------------------------------------------
# Base64 heuristic: at least 16 chars, valid alphabet, correct padding.
# We require a minimum decoded length of 8 bytes to avoid false triggers on
# short tokens like "dGVzdA==" ("test") which carry no attack signal.
# ---------------------------------------------------------------------------
_B64_RE = re.compile(r"(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?")
_B64_MIN_LEN = 16  # minimum raw base64 characters to attempt decode
_B64_MIN_DECODED = 8  # minimum decoded bytes

# ROT13 only makes sense for ASCII alphabetic content; check if the decoded
# result looks like meaningful English (has common short words).
_ROT13_WORDS = frozenset(
    [
        "the",
        "you",
        "and",
        "are",
        "your",
        "this",
        "that",
        "have",
        "not",
        "with",
        "from",
        "they",
        "will",
        "what",
        "ignore",
        "forget",
        "disregard",
        "bypass",
        "jailbreak",
        "system",
        "prompt",
    ]
)

# URL-encoded text: must contain at least two %XX sequences (not necessarily consecutive)
_URL_ENCODED_RE = re.compile(r"(?:%[0-9A-Fa-f]{2}.*){2,}", re.DOTALL)

# Hex-encoded text: compact run of hex digits (even length, min 16 chars)
_HEX_RE = re.compile(r"\b([0-9A-Fa-f]{16,})\b")

# ---------------------------------------------------------------------------
# Language detection: heuristic only, no external dependency.
# Sufficient for tagging common languages; edge cases fall back to None.
# ---------------------------------------------------------------------------
# Unicode block ranges
_CJK_RE = re.compile(
    r"[\u4e00-\u9fff\u3400-\u4dbf\U00020000-\U0002a6df"
    r"\u3040-\u309f\u30a0-\u30ff\uac00-\ud7af]"
)
_ARABIC_RE = re.compile(r"[\u0600-\u06ff]")
_CYRILLIC_RE = re.compile(r"[\u0400-\u04ff]")
_DEVANAGARI_RE = re.compile(r"[\u0900-\u097f]")

# Ratio of script chars required to claim that language
_SCRIPT_THRESHOLD = 0.15


class PreprocessResult(BaseModel):
    """Output of the preprocessing stage."""

    normalized_text: str  # NFKC-normalized, whitespace-cleaned text
    decoded_variants: list[str] = Field(default_factory=list)  # Base64/ROT13/… decoded
    language: str | None = None  # Detected language code (e.g. "en", "zh")
    metadata: dict[str, Any] = Field(default_factory=dict)  # Extra info for downstream


class Preprocessor:
    """Preprocess raw input before feeding it into the detection pipeline."""

    def __init__(self, *, detect_encoding: bool = True) -> None:
        self._detect_encoding = detect_encoding

    def preprocess(self, text: str) -> PreprocessResult:
        """Run all preprocessing steps on *text*.

        Args:
            text: Raw prompt string from user input or external source.

        Returns:
            A :class:`PreprocessResult` containing the normalised text,
            any decoded variants, the detected language, and metadata.
        """
        normalized = self._normalize_unicode(text)
        normalized = self._normalize_whitespace(normalized)

        decoded_variants: list[str] = []
        if self._detect_encoding:
            decoded_variants = self._detect_and_decode(normalized)

        language = self._detect_language(normalized)

        return PreprocessResult(
            normalized_text=normalized,
            decoded_variants=decoded_variants,
            language=language,
            metadata={
                "original_length": len(text),
                "normalized_length": len(normalized),
                "encoding_variants": len(decoded_variants),
            },
        )

    # ------------------------------------------------------------------
    # Step 1 – Unicode normalisation
    # ------------------------------------------------------------------

    @staticmethod
    def _normalize_unicode(text: str) -> str:
        """Apply NFKC normalisation to unify compatibility characters.

        NFKC converts fullwidth letters (ａｂｃ → abc), ligatures (ﬁ → fi),
        superscripts, and other compatibility variants to their canonical
        ASCII equivalents, making regex matching reliable.
        """
        return unicodedata.normalize("NFKC", text)

    # ------------------------------------------------------------------
    # Step 2 – Whitespace normalisation
    # ------------------------------------------------------------------

    @staticmethod
    def _normalize_whitespace(text: str) -> str:
        """Collapse redundant whitespace and remove invisible characters.

        - Strips zero-width and invisible Unicode control characters.
        - Collapses runs of horizontal whitespace to a single space.
        - Collapses 3+ consecutive newlines to two (preserve paragraphs).
        - Strips leading/trailing whitespace.
        """
        text = _ZERO_WIDTH_RE.sub("", text)
        text = _MULTI_SPACE_RE.sub(" ", text)
        text = _MULTI_NL_RE.sub("\n\n", text)
        return text.strip()

    # ------------------------------------------------------------------
    # Step 3 – Encoding detection & decoding
    # ------------------------------------------------------------------

    def _detect_and_decode(self, text: str) -> list[str]:
        """Heuristically detect and decode obfuscated encodings.

        Returns a list of decoded text variants (may be empty).  Each
        candidate is normalised and de-duplicated before returning.
        The original *text* itself is never included in the list.
        """
        variants: list[str] = []
        seen: set[str] = {text}

        def _add(candidate: str) -> None:
            c = unicodedata.normalize("NFKC", candidate).strip()
            if c and c not in seen and _is_printable_text(c):
                seen.add(c)
                variants.append(c)

        _add(self._try_decode_base64(text))
        _add(self._try_decode_rot13(text))
        _add(self._try_decode_url(text))
        for candidate in self._try_decode_hex(text):
            _add(candidate)

        return variants

    @staticmethod
    def _try_decode_base64(text: str) -> str:
        """Attempt Base64 decoding; return decoded string or empty string.

        Strategy:
        - Find the longest contiguous Base64-looking token in the text.
        - Require at least ``_B64_MIN_LEN`` characters and ``_B64_MIN_DECODED``
          decoded bytes to avoid matching short random tokens.
        - The decoded bytes must be valid UTF-8.
        """
        # Find all Base64 candidate substrings
        candidates = [
            m.group(0)
            for m in _B64_RE.finditer(text)
            if len(m.group(0)) >= _B64_MIN_LEN
        ]
        if not candidates:
            return ""
        # Try the longest candidate first
        candidates.sort(key=len, reverse=True)
        for token in candidates:
            try:
                # Ensure correct padding
                padded = token + "=" * (-len(token) % 4)
                decoded_bytes = base64.b64decode(padded, validate=True)
                if len(decoded_bytes) < _B64_MIN_DECODED:
                    continue
                decoded_str = decoded_bytes.decode("utf-8")
                if not _is_printable_text(decoded_str):
                    continue
                return decoded_str
            except Exception:
                continue
        return ""

    @staticmethod
    def _try_decode_rot13(text: str) -> str:
        """Attempt ROT13 decoding; return decoded string only if it looks meaningful.

        ROT13 is only applied to purely ASCII alphabetic content.  The
        decoded result must contain at least one known English word from
        ``_ROT13_WORDS`` to avoid generating noise.
        """
        decoded = (
            text.encode("ascii", errors="ignore")
            .decode()
            .translate(
                str.maketrans(
                    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
                    "NOPQRSTUVWXYZABCDEFGHIJKLMnopqrstuvwxyzabcdefghijklm",
                )
            )
        )
        if decoded == text:
            return ""
        words = set(re.findall(r"[a-z]+", decoded.lower()))
        if words & _ROT13_WORDS:
            return decoded
        return ""

    @staticmethod
    def _try_decode_url(text: str) -> str:
        """Attempt URL-percent decoding; return decoded string if different.

        Only triggered when the text contains at least two ``%XX`` sequences
        to avoid false positives on literal percent signs.
        """
        if not _URL_ENCODED_RE.search(text):
            return ""
        decoded = unquote(text)
        return decoded if decoded != text else ""

    @staticmethod
    def _try_decode_hex(text: str) -> list[str]:
        """Attempt hex decoding on all compact hex runs found in text.

        Each run must be an even number of hex characters (≥ 16) to be
        treated as a hex-encoded byte string.  The decoded bytes must be
        valid UTF-8.
        """
        results: list[str] = []
        for m in _HEX_RE.finditer(text):
            token = m.group(1)
            if len(token) % 2 != 0:
                continue
            try:
                decoded = bytes.fromhex(token).decode("utf-8")
                if len(decoded) >= 4:
                    results.append(decoded)
            except Exception:
                continue
        return results

    # ------------------------------------------------------------------
    # Step 4 – Language detection
    # ------------------------------------------------------------------

    @staticmethod
    def _detect_language(text: str) -> str | None:
        """Lightweight heuristic language detection (no external deps).

        Checks the proportion of characters belonging to common Unicode
        script blocks.  Returns an ISO 639-1 language code or ``None``
        when confidence is insufficient.

        Supported scripts and return values:
            - CJK (Chinese/Japanese/Korean) → "zh" (default CJK tag)
            - Arabic → "ar"
            - Cyrillic → "ru"
            - Devanagari → "hi"
            - Latin (fallback for ASCII-dominant text) → "en"
        """
        if not text:
            return None

        total = len(text)

        def _ratio(pattern: re.Pattern[str]) -> float:
            return len(pattern.findall(text)) / total

        if _ratio(_CJK_RE) >= _SCRIPT_THRESHOLD:
            return "zh"
        if _ratio(_ARABIC_RE) >= _SCRIPT_THRESHOLD:
            return "ar"
        if _ratio(_CYRILLIC_RE) >= _SCRIPT_THRESHOLD:
            return "ru"
        if _ratio(_DEVANAGARI_RE) >= _SCRIPT_THRESHOLD:
            return "hi"

        # Assume Latin/English for ASCII-dominant text
        ascii_count = sum(1 for c in text if ord(c) < 128)
        if ascii_count / total >= 0.8:
            return "en"

        return None


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _is_printable_text(text: str) -> bool:
    """Return True if *text* is mostly printable and not just binary noise.

    Rejects decoded results where more than 20 % of characters are
    non-printable (control codes, null bytes, etc.).
    """
    if not text:
        return False
    non_printable = sum(1 for c in text if unicodedata.category(c).startswith("C"))
    return (non_printable / len(text)) < 0.2
