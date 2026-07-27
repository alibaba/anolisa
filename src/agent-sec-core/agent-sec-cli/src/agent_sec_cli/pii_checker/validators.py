"""Validators used to reduce false positives in PII detection."""

import base64
import binascii
import re
from datetime import datetime


def luhn_check(value: str) -> bool:
    """Validate a payment card number with the Luhn checksum."""
    digits = [int(ch) for ch in re.sub(r"\D", "", value)]
    if len(digits) < 13 or len(digits) > 19:
        return False

    total = 0
    parity = len(digits) % 2
    for idx, digit in enumerate(digits):
        current = digit
        if idx % 2 == parity:
            current *= 2
            if current > 9:
                current -= 9
        total += current
    return total % 10 == 0


def validate_cn_id(value: str) -> bool:
    """Validate an 18-digit Chinese Resident Identity Card number."""
    normalized = value.strip().upper()
    if not re.fullmatch(r"\d{17}[\dX]", normalized):
        return False

    birth_date = normalized[6:14]
    try:
        datetime.strptime(birth_date, "%Y%m%d")
    except ValueError:
        return False

    weights = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2]
    checks = "10X98765432"
    total = sum(int(normalized[i]) * weights[i] for i in range(17))
    return normalized[-1] == checks[total % 11]


def validate_jwt(value: str) -> bool:
    """Validate the structural shape of a JWT."""
    parts = value.split(".")
    if len(parts) != 3 or not all(parts):
        return False
    if not all(re.fullmatch(r"[A-Za-z0-9_-]+", part) for part in parts):
        return False

    for part in parts[:2]:
        padded = part + "=" * (-len(part) % 4)
        try:
            decoded = base64.urlsafe_b64decode(padded.encode("ascii"))
        except (binascii.Error, ValueError):
            return False
        if not decoded.strip():
            return False
    return True


def validate_pem_private_key(value: str) -> bool:
    """Validate that a PEM private key has matching BEGIN/END markers."""
    match = re.search(
        r"-----BEGIN ([A-Z0-9 ]*PRIVATE KEY)-----[\s\S]+?-----END \1-----",
        value.strip(),
    )
    return match is not None
