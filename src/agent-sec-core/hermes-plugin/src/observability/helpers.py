from __future__ import annotations

from collections.abc import Mapping
from dataclasses import fields, is_dataclass
from datetime import datetime, timezone
from enum import Enum
from typing import Any


def compact_record(record: dict[str, Any]) -> dict[str, Any]:
    return {key: value for key, value in record.items() if value is not None}


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def non_empty_string(value: Any) -> str | None:
    if isinstance(value, str) and value.strip():
        return value.strip()
    if value is not None and not isinstance(value, (dict, list, tuple, set)):
        text = str(value).strip()
        if text:
            return text
    return None


_MAX_DEPTH = 64


def json_safe(value: Any) -> Any:
    """Return a JSON-serializable representation of Hermes hook values."""
    return _json_safe(value, set(), 0)


def _json_safe(value: Any, seen: set[int], depth: int) -> Any:
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    if isinstance(value, Enum):
        return _json_safe(value.value, seen, depth)

    if depth >= _MAX_DEPTH:
        return "<too deep>"

    value_id = id(value)
    if value_id in seen:
        return "<recursive>"

    next_depth = depth + 1

    if isinstance(value, Mapping):
        seen.add(value_id)
        try:
            return {
                _json_key(key): _json_safe(item, seen, next_depth)
                for key, item in value.items()
            }
        finally:
            seen.remove(value_id)

    if isinstance(value, (list, tuple, set, frozenset)):
        seen.add(value_id)
        try:
            return [_json_safe(item, seen, next_depth) for item in value]
        finally:
            seen.remove(value_id)

    if is_dataclass(value) and not isinstance(value, type):
        seen.add(value_id)
        try:
            return _json_safe(
                {field.name: getattr(value, field.name) for field in fields(value)},
                seen,
                next_depth,
            )
        finally:
            seen.remove(value_id)

    dumped = _call_serialization_method(value, "model_dump", {"mode": "json"})
    if dumped is not None:
        return _json_safe(dumped, seen, next_depth)

    dumped = _call_serialization_method(value, "dict", {})
    if dumped is not None:
        return _json_safe(dumped, seen, next_depth)

    attributes = _public_attributes(value)
    if attributes is not None:
        seen.add(value_id)
        try:
            return _json_safe(attributes, seen, next_depth)
        finally:
            seen.remove(value_id)

    return str(value)


def _json_key(value: Any) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    if isinstance(value, Enum):
        return _json_key(value.value)
    return str(value)


def _call_serialization_method(
    value: Any,
    method_name: str,
    kwargs: dict[str, Any],
) -> Any:
    method = getattr(value, method_name, None)
    if method is None or not callable(method):
        return None
    try:
        return method(**kwargs)
    except TypeError:
        if kwargs:
            try:
                return method()
            except Exception:
                return None
        return None
    except Exception:
        return None


def _public_attributes(value: Any) -> dict[str, Any] | None:
    try:
        attributes = vars(value)
    except TypeError:
        return None
    public = {
        key: item
        for key, item in attributes.items()
        if isinstance(key, str) and not key.startswith("_")
    }
    return public or None
