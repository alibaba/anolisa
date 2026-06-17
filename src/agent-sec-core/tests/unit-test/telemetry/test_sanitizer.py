"""Unit tests for telemetry sanitizer helpers."""

import json
from datetime import datetime

from agent_sec_cli.telemetry.sanitizer import (
    details_dict,
    error_type_value,
    error_value,
    now_iso,
    request_value,
    result_dict,
    result_value,
    to_json_safe,
    value_or_none,
)


class ModelLike:
    def model_dump(self):
        return {"items": ("a", "b"), "bad": float("nan")}


class NotJsonSerializable:
    def __str__(self):
        return "fallback-string"


def test_now_iso_returns_parseable_timestamp() -> None:
    datetime.fromisoformat(now_iso())


def test_to_json_safe_normalizes_nested_values() -> None:
    value = {
        1: ("x", float("nan")),
        "set": {"z", "a"},
        "list": [float("inf"), float("-inf"), 1.25],
    }

    safe = to_json_safe(value)

    assert safe == {
        "1": ["x", None],
        "set": ["a", "z"],
        "list": [None, None, 1.25],
    }
    json.dumps(safe, allow_nan=False)


def test_to_json_safe_uses_model_dump_and_string_fallback() -> None:
    assert to_json_safe(ModelLike()) == {"items": ["a", "b"], "bad": None}
    assert to_json_safe(NotJsonSerializable()) == "fallback-string"


def test_details_dict_returns_dict_or_empty_dict() -> None:
    details = {"request": {"source": "manual"}}

    assert details_dict(details) is details
    assert details_dict(("not", "a", "dict")) == {}
    assert details_dict(None) == {}


def test_value_or_none_converts_empty_string_only() -> None:
    assert value_or_none("") is None
    assert value_or_none("value") == "value"
    assert value_or_none(False) is False
    assert value_or_none(0) == 0


def test_result_dict_returns_nested_result_dict_or_empty_dict() -> None:
    result = {"verdict": "deny"}

    assert result_dict({"result": result}) is result
    assert result_dict({}) == {}
    assert result_dict({"result": "not-a-dict"}) == {}


def test_request_value_handles_missing_and_json_safe_values() -> None:
    assert request_value({}) is None
    assert request_value({"request": {"items": ("a", "b")}}) == {"items": ["a", "b"]}


def test_error_values_handle_missing_and_json_safe_values() -> None:
    assert error_value({}) is None
    assert error_type_value({}) is None
    assert error_value({"error": float("nan")}) is None
    assert error_type_value({"error_type": ("RuntimeError",)}) == ["RuntimeError"]


def test_result_value_handles_missing_and_json_safe_values() -> None:
    result = {"summary": {"values": {"z", "a"}}, "elapsed_ms": float("inf")}

    assert result_value(result, "missing") is None
    assert result_value(result, "summary") == {"values": ["a", "z"]}
    assert result_value(result, "elapsed_ms") is None
