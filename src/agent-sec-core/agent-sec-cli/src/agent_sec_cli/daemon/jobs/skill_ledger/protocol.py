"""Validated NDJSON protocol for the Skill Ledger worker."""

import json
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

WORKER_SCHEMA_VERSION = 1
WORKER_PROCESS_CHANGE_METHOD = "process_change"
MAX_WORKER_FRAME_BYTES = 4 * 1024 * 1024
SKILLFS_EVENT_KINDS = frozenset(
    {
        "mkdir",
        "create",
        "write",
        "rename",
        "unlink",
        "rmdir",
        "setattr",
        "truncate",
        "reconcile",
    }
)


class WorkerProtocolError(ValueError):
    """Raised when a Skill Ledger worker frame violates the protocol."""


@dataclass
class SkillFsChange:
    """Validated SkillFS change notification."""

    skill_dir: Path
    skill_name: str
    event_kinds: set[str] = field(default_factory=set)
    paths: set[str] = field(default_factory=set)

    def merge(self, other: "SkillFsChange") -> None:
        """Merge another notification for the same skill."""
        self.event_kinds.update(other.event_kinds)
        self.paths.update(other.paths)

    def to_dict(self) -> dict[str, Any]:
        """Return a JSON-serializable job/debug payload."""
        return {
            "skillDir": str(self.skill_dir),
            "skillName": self.skill_name,
            "eventKinds": sorted(self.event_kinds),
            "paths": sorted(self.paths),
        }

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> "SkillFsChange":
        """Validate and build a change received by the worker."""
        skill_dir = payload.get("skillDir")
        skill_name = payload.get("skillName")
        event_kinds = payload.get("eventKinds")
        paths = payload.get("paths")

        if not isinstance(skill_dir, str) or not skill_dir:
            raise WorkerProtocolError("change.skillDir must be a non-empty string")
        if "\x00" in skill_dir:
            raise WorkerProtocolError("change.skillDir must not contain NUL characters")
        if not Path(skill_dir).is_absolute():
            raise WorkerProtocolError("change.skillDir must be an absolute path")
        if not isinstance(skill_name, str) or not skill_name:
            raise WorkerProtocolError("change.skillName must be a non-empty string")
        if skill_name != Path(skill_dir).name:
            raise WorkerProtocolError("change.skillName must match skillDir basename")
        return cls(
            skill_dir=Path(skill_dir),
            skill_name=skill_name,
            event_kinds=_event_kind_set(event_kinds),
            paths=_relative_path_set(paths),
        )


@dataclass(frozen=True)
class WorkerRequest:
    """Validated request sent to the Skill Ledger worker."""

    request_id: str
    change: SkillFsChange


@dataclass(frozen=True)
class WorkerError:
    """Structured worker execution error."""

    error_type: str
    message: str


@dataclass(frozen=True)
class WorkerResponse:
    """Validated response returned by the Skill Ledger worker."""

    request_id: str
    ok: bool
    result: dict[str, Any] | None = None
    error: WorkerError | None = None


def new_worker_request(change: SkillFsChange) -> WorkerRequest:
    """Build a request with a daemon-owned correlation id."""
    return WorkerRequest(request_id=str(uuid.uuid4()), change=change)


def serialize_worker_request(request: WorkerRequest) -> bytes:
    """Serialize one worker request as a bounded NDJSON frame."""
    return _serialize_frame(
        {
            "schemaVersion": WORKER_SCHEMA_VERSION,
            "requestId": request.request_id,
            "method": WORKER_PROCESS_CHANGE_METHOD,
            "change": request.change.to_dict(),
        }
    )


def parse_worker_request(line: bytes) -> WorkerRequest:
    """Parse and validate one worker request frame."""
    payload = _decode_frame(line)
    _validate_schema(payload)
    request_id = _request_id(payload)
    if payload.get("method") != WORKER_PROCESS_CHANGE_METHOD:
        raise WorkerProtocolError(f"method must be {WORKER_PROCESS_CHANGE_METHOD!r}")
    change = payload.get("change")
    if not isinstance(change, dict):
        raise WorkerProtocolError("change must be a JSON object")
    return WorkerRequest(
        request_id=request_id,
        change=SkillFsChange.from_dict(change),
    )


def success_worker_response(
    request_id: str,
    result: dict[str, Any],
) -> WorkerResponse:
    """Build a successful worker response."""
    return WorkerResponse(request_id=request_id, ok=True, result=result)


def error_worker_response(
    request_id: str,
    exc: Exception,
) -> WorkerResponse:
    """Build a worker response for a processing exception."""
    return WorkerResponse(
        request_id=request_id,
        ok=False,
        error=WorkerError(error_type=type(exc).__name__, message=str(exc)),
    )


def serialize_worker_response(response: WorkerResponse) -> bytes:
    """Serialize one worker response as a bounded NDJSON frame."""
    payload: dict[str, Any] = {
        "schemaVersion": WORKER_SCHEMA_VERSION,
        "requestId": response.request_id,
        "ok": response.ok,
    }
    if response.ok:
        payload["result"] = response.result
    elif response.error is not None:
        payload["error"] = {
            "type": response.error.error_type,
            "message": response.error.message,
        }
    return _serialize_frame(payload)


def parse_worker_response(line: bytes) -> WorkerResponse:
    """Parse and validate one worker response frame."""
    payload = _decode_frame(line)
    _validate_schema(payload)
    request_id = _request_id(payload)
    ok = payload.get("ok")
    if not isinstance(ok, bool):
        raise WorkerProtocolError("ok must be a boolean")

    if ok:
        result = payload.get("result")
        if not isinstance(result, dict):
            raise WorkerProtocolError("result must be a JSON object")
        return WorkerResponse(request_id=request_id, ok=True, result=result)

    error = payload.get("error")
    if not isinstance(error, dict):
        raise WorkerProtocolError("error must be a JSON object")
    error_type = error.get("type")
    message = error.get("message")
    if not isinstance(error_type, str) or not error_type:
        raise WorkerProtocolError("error.type must be a non-empty string")
    if not isinstance(message, str):
        raise WorkerProtocolError("error.message must be a string")
    return WorkerResponse(
        request_id=request_id,
        ok=False,
        error=WorkerError(error_type=error_type, message=message),
    )


def _decode_frame(line: bytes) -> dict[str, Any]:
    if len(line) > MAX_WORKER_FRAME_BYTES:
        raise WorkerProtocolError(
            f"worker frame exceeds {MAX_WORKER_FRAME_BYTES} bytes"
        )
    stripped = line.strip()
    if not stripped:
        raise WorkerProtocolError("worker frame must not be empty")
    try:
        payload = json.loads(stripped.decode("utf-8"))
    except UnicodeDecodeError as exc:
        raise WorkerProtocolError("worker frame must be valid UTF-8") from exc
    except json.JSONDecodeError as exc:
        raise WorkerProtocolError("worker frame must be valid JSON") from exc
    if not isinstance(payload, dict):
        raise WorkerProtocolError("worker frame must be a JSON object")
    return payload


def _serialize_frame(payload: dict[str, Any]) -> bytes:
    try:
        frame = (
            json.dumps(payload, ensure_ascii=False, separators=(",", ":")) + "\n"
        ).encode("utf-8")
    except (TypeError, ValueError) as exc:
        raise WorkerProtocolError("worker payload must be JSON serializable") from exc
    if len(frame) > MAX_WORKER_FRAME_BYTES:
        raise WorkerProtocolError(
            f"worker frame exceeds {MAX_WORKER_FRAME_BYTES} bytes"
        )
    return frame


def _validate_schema(payload: dict[str, Any]) -> None:
    if payload.get("schemaVersion") != WORKER_SCHEMA_VERSION:
        raise WorkerProtocolError(f"schemaVersion must be {WORKER_SCHEMA_VERSION}")


def _request_id(payload: dict[str, Any]) -> str:
    request_id = payload.get("requestId")
    if not isinstance(request_id, str) or not request_id:
        raise WorkerProtocolError("requestId must be a non-empty string")
    return request_id


def _string_set(value: Any, field_name: str) -> set[str]:
    if not isinstance(value, list):
        raise WorkerProtocolError(f"{field_name} must be a list")
    if any(not isinstance(item, str) or not item for item in value):
        raise WorkerProtocolError(f"{field_name} must contain non-empty strings")
    return set(value)


def _event_kind_set(value: Any) -> set[str]:
    event_kinds = _string_set(value, "change.eventKinds")
    if not event_kinds or not event_kinds.issubset(SKILLFS_EVENT_KINDS):
        allowed = ", ".join(sorted(SKILLFS_EVENT_KINDS))
        raise WorkerProtocolError(f"change.eventKinds must contain only: {allowed}")
    return event_kinds


def _relative_path_set(value: Any) -> set[str]:
    paths = _string_set(value, "change.paths")
    for item in paths:
        if "\x00" in item:
            raise WorkerProtocolError("change.paths must not contain NUL characters")
        path = Path(item)
        if not path.parts or path.is_absolute() or ".." in path.parts:
            raise WorkerProtocolError(
                "change.paths must contain relative paths under skillDir"
            )
    return paths
