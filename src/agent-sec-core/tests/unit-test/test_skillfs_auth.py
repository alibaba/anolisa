"""Tests for the cross-language SkillFS HMAC authentication contract."""

import base64
import os
from pathlib import Path

import pytest

from agent_sec_cli.skillfs_auth import (
    CONTROL_CLIENT_DOMAIN,
    CONTROL_SERVER_DOMAIN,
    MAX_AUTH_SECRET_BYTES,
    NOTIFY_CLIENT_DOMAIN,
    NOTIFY_SERVER_DOMAIN,
    SkillFsAuthError,
    calculate_proof,
    load_auth_secret,
)


@pytest.mark.parametrize(
    ("domain", "expected"),
    [
        (CONTROL_CLIENT_DOMAIN, "pqaSiunq07XWqMvQ8xSiSLi6dsLEy5iaCEF3md04AVI="),
        (CONTROL_SERVER_DOMAIN, "naSgjgOT+Zs71EytW6byhJMCkfek2sGmK+CDqHmDsas="),
        (NOTIFY_CLIENT_DOMAIN, "aFcVadTie7FrVTYOjk1OOjBpoQZ6LUvnLGC6stiqt6M="),
        (NOTIFY_SERVER_DOMAIN, "F22J+ua0Pmha2dPyTMmTQNtKjcmed59Mo8FKgdcgBOc="),
    ],
)
def test_hmac_vectors_match_rust_contract(domain: str, expected: str) -> None:
    secret = bytes(range(32))
    nonce = bytes(range(32, 64))

    proof = calculate_proof(secret, domain, nonce)

    assert base64.b64encode(proof).decode("ascii") == expected


def test_load_auth_secret_preserves_raw_bytes(tmp_path: Path) -> None:
    secret_path = tmp_path / "secret"
    expected = b"x" * 31 + b"\n"
    secret_path.write_bytes(expected)
    secret_path.chmod(0o600)

    assert load_auth_secret(secret_path) == expected


@pytest.mark.parametrize("mode", [0o640, 0o604, 0o666])
def test_load_auth_secret_rejects_group_or_other_access(
    tmp_path: Path,
    mode: int,
) -> None:
    secret_path = tmp_path / "secret"
    secret_path.write_bytes(b"x" * 32)
    secret_path.chmod(mode)

    with pytest.raises(SkillFsAuthError, match="group or other"):
        load_auth_secret(secret_path)


def test_load_auth_secret_rejects_symlink(tmp_path: Path) -> None:
    target = tmp_path / "target"
    target.write_bytes(b"x" * 32)
    target.chmod(0o600)
    secret_path = tmp_path / "secret"
    secret_path.symlink_to(target)

    with pytest.raises(SkillFsAuthError, match="cannot open"):
        load_auth_secret(secret_path)


def test_load_auth_secret_rejects_directory(tmp_path: Path) -> None:
    secret_path = tmp_path / "secret"
    secret_path.mkdir(mode=0o700)

    with pytest.raises(SkillFsAuthError, match="regular file"):
        load_auth_secret(secret_path)


@pytest.mark.parametrize("size", [0, 31, MAX_AUTH_SECRET_BYTES + 1])
def test_load_auth_secret_rejects_invalid_size(tmp_path: Path, size: int) -> None:
    secret_path = tmp_path / "secret"
    secret_path.write_bytes(b"x" * size)
    secret_path.chmod(0o600)

    with pytest.raises(SkillFsAuthError, match="authentication secret"):
        load_auth_secret(secret_path)


def test_load_auth_secret_rejects_wrong_owner(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    secret_path = tmp_path / "secret"
    secret_path.write_bytes(b"x" * 32)
    secret_path.chmod(0o600)
    monkeypatch.setattr(os, "geteuid", lambda: secret_path.stat().st_uid + 1)

    with pytest.raises(SkillFsAuthError, match="current user"):
        load_auth_secret(secret_path)


def test_load_auth_secret_rejects_relative_path() -> None:
    with pytest.raises(SkillFsAuthError, match="must be absolute"):
        load_auth_secret("relative.secret")
