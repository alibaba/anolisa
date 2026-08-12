"""Shared authentication primitives for SkillFS local socket protocols."""

import base64
import hashlib
import hmac
import json
import os
import stat
from pathlib import Path
from typing import Any

AUTH_VERSION = "1"
AUTH_INIT = "auth.init"
AUTH_CHALLENGE = "auth.challenge"
AUTH_PROOF = "auth.proof"
AUTH_OK = "auth.ok"
AUTH_NONCE_BYTES = 32
AUTH_MAC_BYTES = hashlib.sha256().digest_size
MAX_AUTH_FRAME_BYTES = 4096
MIN_AUTH_SECRET_BYTES = 32
MAX_AUTH_SECRET_BYTES = 4096

CONTROL_CLIENT_DOMAIN = "anolisa.skillfs.control.client.v1"
CONTROL_SERVER_DOMAIN = "anolisa.skillfs.control.server.v1"
NOTIFY_CLIENT_DOMAIN = "anolisa.skillfs.notify.client.v1"
NOTIFY_SERVER_DOMAIN = "anolisa.skillfs.notify.server.v1"


class SkillFsAuthError(Exception):
    """Authentication configuration or protocol validation failed."""


def load_auth_secret(path: str | Path) -> bytes:
    """Read a bounded secret from a private, owner-controlled regular file."""
    secret_path = Path(path)
    if not secret_path.is_absolute():
        raise SkillFsAuthError("authentication secret file path must be absolute")
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(secret_path, flags)
    except OSError as exc:
        raise SkillFsAuthError(
            f"cannot open authentication secret file: {secret_path}"
        ) from exc

    try:
        metadata = os.fstat(fd)
        if not stat.S_ISREG(metadata.st_mode):
            raise SkillFsAuthError("authentication secret must be a regular file")
        if metadata.st_uid != os.geteuid():
            raise SkillFsAuthError(
                "authentication secret must be owned by the current user"
            )
        if stat.S_IMODE(metadata.st_mode) & 0o077:
            raise SkillFsAuthError(
                "authentication secret must not grant group or other access"
            )

        secret = bytearray()
        while len(secret) <= MAX_AUTH_SECRET_BYTES:
            chunk = os.read(fd, MAX_AUTH_SECRET_BYTES + 1 - len(secret))
            if not chunk:
                break
            secret.extend(chunk)
    except OSError as exc:
        raise SkillFsAuthError("cannot read authentication secret file") from exc
    finally:
        os.close(fd)

    if len(secret) < MIN_AUTH_SECRET_BYTES:
        raise SkillFsAuthError(
            f"authentication secret must contain at least {MIN_AUTH_SECRET_BYTES} bytes"
        )
    if len(secret) > MAX_AUTH_SECRET_BYTES:
        raise SkillFsAuthError(
            f"authentication secret must not exceed {MAX_AUTH_SECRET_BYTES} bytes"
        )
    return bytes(secret)


def auth_init_frame() -> bytes:
    """Build the first frame of an authenticated connection."""
    return _encode_frame({"authVersion": AUTH_VERSION, "type": AUTH_INIT})


def auth_challenge_frame(nonce: bytes) -> bytes:
    """Build a challenge frame for a fresh 32-byte nonce."""
    _require_length(nonce, AUTH_NONCE_BYTES, "nonce")
    return _encode_frame(
        {
            "authVersion": AUTH_VERSION,
            "type": AUTH_CHALLENGE,
            "nonce": base64.b64encode(nonce).decode("ascii"),
        }
    )


def auth_proof_frame(proof: bytes) -> bytes:
    """Build a client proof frame."""
    _require_length(proof, AUTH_MAC_BYTES, "proof")
    return _encode_frame(
        {
            "authVersion": AUTH_VERSION,
            "type": AUTH_PROOF,
            "proof": base64.b64encode(proof).decode("ascii"),
        }
    )


def auth_ok_frame(proof: bytes) -> bytes:
    """Build a server proof frame."""
    _require_length(proof, AUTH_MAC_BYTES, "proof")
    return _encode_frame(
        {
            "authVersion": AUTH_VERSION,
            "type": AUTH_OK,
            "proof": base64.b64encode(proof).decode("ascii"),
        }
    )


def parse_auth_init(frame: bytes) -> None:
    """Validate an authentication initialization frame."""
    _parse_frame(frame, AUTH_INIT, frozenset({"authVersion", "type"}))


def parse_auth_challenge(frame: bytes) -> bytes:
    """Validate a challenge frame and return its raw nonce."""
    payload = _parse_frame(
        frame,
        AUTH_CHALLENGE,
        frozenset({"authVersion", "type", "nonce"}),
    )
    return _decode_fixed_base64(payload.get("nonce"), AUTH_NONCE_BYTES, "nonce")


def parse_auth_proof(frame: bytes) -> bytes:
    """Validate a client proof frame and return its raw MAC."""
    payload = _parse_frame(
        frame,
        AUTH_PROOF,
        frozenset({"authVersion", "type", "proof"}),
    )
    return _decode_fixed_base64(payload.get("proof"), AUTH_MAC_BYTES, "proof")


def parse_auth_ok(frame: bytes) -> bytes:
    """Validate a server proof frame and return its raw MAC."""
    payload = _parse_frame(
        frame,
        AUTH_OK,
        frozenset({"authVersion", "type", "proof"}),
    )
    return _decode_fixed_base64(payload.get("proof"), AUTH_MAC_BYTES, "proof")


def auth_frame_type(frame: bytes) -> str | None:
    """Return the declared auth frame type without treating plain JSON as auth."""
    if len(frame) > MAX_AUTH_FRAME_BYTES:
        return None
    try:
        payload = json.loads(frame.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return None
    if not isinstance(payload, dict) or payload.get("authVersion") != AUTH_VERSION:
        return None
    frame_type = payload.get("type")
    if frame_type == AUTH_INIT and frozenset(payload) != frozenset(
        {"authVersion", "type"}
    ):
        return None
    return frame_type if isinstance(frame_type, str) else None


def calculate_proof(secret: bytes, domain: str, nonce: bytes) -> bytes:
    """Calculate the domain-separated proof shared with the Rust implementation."""
    _require_length(nonce, AUTH_NONCE_BYTES, "nonce")
    message = domain.encode("ascii") + b"\0" + nonce
    return hmac.new(secret, message, hashlib.sha256).digest()


def proof_matches(actual: bytes, expected: bytes) -> bool:
    """Compare authentication proofs without content-dependent timing."""
    return hmac.compare_digest(actual, expected)


def _parse_frame(
    frame: bytes,
    expected_type: str,
    expected_fields: frozenset[str],
) -> dict[str, Any]:
    if len(frame) > MAX_AUTH_FRAME_BYTES:
        raise SkillFsAuthError("authentication frame exceeds size limit")
    try:
        payload = json.loads(frame.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise SkillFsAuthError("invalid authentication frame") from exc
    if not isinstance(payload, dict) or frozenset(payload) != expected_fields:
        raise SkillFsAuthError("invalid authentication frame")
    if payload.get("authVersion") != AUTH_VERSION:
        raise SkillFsAuthError("unsupported authentication version")
    if payload.get("type") != expected_type:
        raise SkillFsAuthError("unexpected authentication frame")
    return payload


def _decode_fixed_base64(value: Any, length: int, label: str) -> bytes:
    if not isinstance(value, str):
        raise SkillFsAuthError(f"authentication {label} is invalid")
    try:
        raw = base64.b64decode(value, validate=True)
    except (ValueError, base64.binascii.Error) as exc:
        raise SkillFsAuthError(f"authentication {label} is invalid") from exc
    if base64.b64encode(raw).decode("ascii") != value:
        raise SkillFsAuthError(f"authentication {label} is not canonical base64")
    _require_length(raw, length, label)
    return raw


def _require_length(value: bytes, length: int, label: str) -> None:
    if len(value) != length:
        raise SkillFsAuthError(f"authentication {label} has an invalid length")


def _encode_frame(payload: dict[str, Any]) -> bytes:
    return json.dumps(payload, separators=(",", ":")).encode("utf-8") + b"\n"
