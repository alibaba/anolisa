"""Unit tests for pii_checker validators."""

from agent_sec_cli.pii_checker.validators import (
    luhn_check,
    validate_cn_id,
    validate_jwt,
    validate_pem_private_key,
)


def test_luhn_valid_card():
    assert luhn_check("4111 1111 1111 1111")


def test_luhn_invalid_card():
    assert not luhn_check("4111 1111 1111 1112")


def test_cn_id_valid_checksum_and_date():
    assert validate_cn_id("11010519491231002X")


def test_cn_id_accepts_lowercase_x_checksum():
    assert validate_cn_id("11010519491231002x")


def test_cn_id_invalid_date():
    assert not validate_cn_id("11010519490231002X")


def test_cn_id_invalid_checksum():
    assert not validate_cn_id("110105194912310021")


def test_jwt_valid_structure():
    token = (
        "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9."
        "eyJzdWIiOiIxMjM0NTY3ODkwIn0."
        "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
    )
    assert validate_jwt(token)


def test_jwt_invalid_structure():
    assert not validate_jwt("not.a.jwt")


def test_pem_private_key_matching_markers():
    pem = """-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEA0testbody
-----END RSA PRIVATE KEY-----"""
    assert validate_pem_private_key(pem)


def test_pem_private_key_mismatched_markers():
    pem = """-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEA0testbody
-----END EC PRIVATE KEY-----"""
    assert not validate_pem_private_key(pem)
