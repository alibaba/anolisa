"""Resolve canonical Skill Ledger paths to per-operation filesystem roots."""

import json
import logging
import os
import re
import socket
from collections.abc import Callable
from dataclasses import dataclass
from functools import wraps
from pathlib import Path
from typing import Any, Literal, TypeAlias

from agent_sec_cli.skill_ledger.config import is_managed_covered
from agent_sec_cli.skill_ledger.errors import (
    SkillLedgerError,
    SkillRootResolveError,
)
from agent_sec_cli.skill_ledger.path_identity import (
    normalize_canonical_skill_dir,
    validate_canonical_skill_dir,
)

CONTROL_SCHEMA_VERSION = "1"
RESOLVE_LIVE_SOURCE_METHOD = "skill.resolveLiveSource"
SKILLFS_RUNTIME_ROOT = Path("/run/user")
SKILLFS_CONTROL_SOCKET_RELATIVE_PATH = Path("skillfs/control.sock")
DEFAULT_RESOLVER_TIMEOUT_SECONDS = 1.0
MAX_CONTROL_RESPONSE_BYTES = 64 * 1024

logger = logging.getLogger(__name__)

SkillRootSource: TypeAlias = Literal["host", "skillfs"]


def _path_root_pattern(root: Path) -> re.Pattern[str]:
    """Match a complete POSIX path token, excluding sibling-name continuations."""
    return re.compile(rf"(?<![\w.@~+-]){re.escape(str(root))}(?![\w.@~+-])")


class _ResolverSocketMissing(Exception):
    """The default SkillFS socket does not exist in this host environment."""


class _ResolverProtocolError(Exception):
    """SkillFS returned a response that violates the resolver contract."""


@dataclass(frozen=True)
class ResolvedSkillRoot:
    """Canonical identity and the matching path used for this operation's I/O."""

    canonical_dir: Path
    io_dir: Path
    source: SkillRootSource

    @property
    def skill_name(self) -> str:
        """Return the canonical leaf name retained by manifest schema v1."""
        return self.canonical_dir.name

    def canonical_path(self, io_path: str | Path) -> Path:
        """Map an internal path below ``io_dir`` back to canonical display space."""
        path = Path(io_path)
        for alias in self._io_aliases():
            try:
                relative = path.relative_to(alias)
            except ValueError:
                continue
            return self.canonical_dir / relative
        raise ValueError("I/O path is outside the resolved skill root")

    def canonicalize_message(self, message: str) -> str:
        """Project internal root prefixes in diagnostic text to canonical space."""
        if self.io_dir == self.canonical_dir:
            return message

        projected = message
        for alias in self._io_aliases():
            if alias == self.canonical_dir:
                continue
            projected = _path_root_pattern(alias).sub(
                str(self.canonical_dir), projected
            )
        return projected

    def canonicalize_payload(self, payload: Any) -> Any:
        """Recursively project string values in a JSON-like payload."""
        if isinstance(payload, str):
            return self.canonicalize_message(payload)
        if isinstance(payload, list):
            return [self.canonicalize_payload(value) for value in payload]
        if isinstance(payload, tuple):
            return tuple(self.canonicalize_payload(value) for value in payload)
        if isinstance(payload, dict):
            return {
                key: self.canonicalize_payload(value) for key, value in payload.items()
            }
        return payload

    def contains_io_path(self, payload: Any) -> bool:
        """Return whether a JSON-like payload still exposes a known I/O root."""
        if self.io_dir == self.canonical_dir:
            return False
        if isinstance(payload, str):
            return any(
                alias != self.canonical_dir
                and _path_root_pattern(alias).search(payload) is not None
                for alias in self._io_aliases()
            )
        if isinstance(payload, (list, tuple)):
            return any(self.contains_io_path(value) for value in payload)
        if isinstance(payload, dict):
            return any(
                self.contains_io_path(key) or self.contains_io_path(value)
                for key, value in payload.items()
            )
        return False

    def _io_aliases(self) -> tuple[Path, ...]:
        aliases = {self.io_dir}
        try:
            aliases.add(self.io_dir.resolve(strict=False))
        except (OSError, RuntimeError):
            pass
        return tuple(sorted(aliases, key=lambda path: len(str(path)), reverse=True))


SkillRootInput: TypeAlias = str | Path | ResolvedSkillRoot


def default_skillfs_control_socket() -> Path:
    """Return SkillFS's per-effective-user control socket endpoint."""
    return (
        SKILLFS_RUNTIME_ROOT / str(os.geteuid()) / SKILLFS_CONTROL_SOCKET_RELATIVE_PATH
    )


class SkillFsResolverClient:
    """One-shot client for SkillFS's read-only live-source resolver."""

    def __init__(
        self,
        socket_path: str | Path | None = None,
        *,
        timeout_seconds: float = DEFAULT_RESOLVER_TIMEOUT_SECONDS,
    ) -> None:
        if timeout_seconds <= 0:
            raise ValueError("resolver timeout must be positive")
        self.socket_path = (
            Path(socket_path)
            if socket_path is not None
            else default_skillfs_control_socket()
        )
        self.timeout_seconds = timeout_seconds

    def resolve(self, canonical_dir: Path) -> Path | None:
        """Return SkillFS's live directory, or ``None`` when it is not managed."""
        request = {
            "schemaVersion": CONTROL_SCHEMA_VERSION,
            "method": RESOLVE_LIVE_SOURCE_METHOD,
            "canonicalSkillDir": str(canonical_dir),
        }
        payload = json.dumps(request, separators=(",", ":")).encode("utf-8") + b"\n"

        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
                connection.settimeout(self.timeout_seconds)
                connection.connect(str(self.socket_path))
                connection.sendall(payload)
                with connection.makefile("rb") as response_stream:
                    response_line = response_stream.readline(
                        MAX_CONTROL_RESPONSE_BYTES + 1
                    )
        except FileNotFoundError as exc:
            raise _ResolverSocketMissing from exc

        if not response_line:
            raise _ResolverProtocolError("empty response")
        if len(response_line) > MAX_CONTROL_RESPONSE_BYTES:
            raise _ResolverProtocolError("response exceeds size limit")

        try:
            response = json.loads(response_line.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise _ResolverProtocolError("response is not valid UTF-8 JSON") from exc
        if not isinstance(response, dict):
            raise _ResolverProtocolError("response must be a JSON object")
        if response.get("schemaVersion") != CONTROL_SCHEMA_VERSION:
            raise _ResolverProtocolError("unsupported response schemaVersion")

        ok = response.get("ok")
        if ok is False:
            error = response.get("error")
            code = error.get("code") if isinstance(error, dict) else None
            label = code if isinstance(code, str) and code else "unknown_error"
            raise _ResolverProtocolError(f"resolver returned {label}")
        if ok is not True:
            raise _ResolverProtocolError("response ok must be a boolean")

        result = response.get("result")
        if not isinstance(result, dict):
            raise _ResolverProtocolError("successful response must contain result")

        managed = result.get("managed")
        if not isinstance(managed, bool):
            raise _ResolverProtocolError("result managed must be a boolean")
        try:
            echoed_canonical = validate_canonical_skill_dir(
                result.get("canonicalSkillDir")
            )
        except ValueError as exc:
            raise _ResolverProtocolError("invalid canonical path echo") from exc
        if echoed_canonical != canonical_dir:
            raise _ResolverProtocolError("canonical path echo mismatch")

        if not managed:
            return None

        try:
            live_dir = validate_canonical_skill_dir(result.get("liveSkillDir"))
        except ValueError as exc:
            raise _ResolverProtocolError("invalid live skill directory") from exc
        try:
            is_directory = live_dir.is_dir()
        except OSError as exc:
            raise _ResolverProtocolError(
                "live skill directory is inaccessible"
            ) from exc
        if not is_directory:
            raise _ResolverProtocolError("live skill directory is inaccessible")
        return live_dir


class SkillRootResolver:
    """Resolve canonical paths through SkillFS, with host-only fallback rules."""

    def __init__(self, client: SkillFsResolverClient | None = None) -> None:
        self.client = client or SkillFsResolverClient()

    def resolve(self, canonical_skill_dir: str | Path) -> ResolvedSkillRoot:
        """Resolve once without caching or retrying."""
        canonical_dir = normalize_canonical_skill_dir(canonical_skill_dir)
        try:
            live_dir = self.client.resolve(canonical_dir)
        except _ResolverSocketMissing:
            return ResolvedSkillRoot(canonical_dir, canonical_dir, "host")
        except Exception as exc:
            logger.debug(
                "SkillFS resolver failed for canonical path %s",
                canonical_dir,
                exc_info=True,
            )
            raise SkillRootResolveError(
                canonical_dir,
                _public_resolver_failure_reason(exc),
            ) from exc

        if live_dir is None:
            return ResolvedSkillRoot(canonical_dir, canonical_dir, "host")
        return ResolvedSkillRoot(canonical_dir, live_dir, "skillfs")


def resolve_skill_root(
    skill_root: SkillRootInput,
    *,
    resolver: SkillRootResolver | None = None,
) -> ResolvedSkillRoot:
    """Return an existing operation context or resolve a canonical path once."""
    if isinstance(skill_root, ResolvedSkillRoot):
        return skill_root
    return (resolver or SkillRootResolver()).resolve(skill_root)


def canonical_skill_operation(operation: Callable[..., Any]) -> Callable[..., Any]:
    """Resolve once and prevent internal I/O paths from escaping in errors."""

    @wraps(operation)
    def wrapped(
        skill_root: SkillRootInput,
        *args: Any,
        **kwargs: Any,
    ) -> Any:
        root = resolve_skill_root(skill_root)
        try:
            return operation(root, *args, **kwargs)
        except SkillRootResolveError:
            raise
        except Exception as exc:
            message = root.canonicalize_message(str(exc))
            if message == str(exc):
                raise
            raise SkillLedgerError(message) from exc

    return wrapped


def validate_resolved_skill_root(root: ResolvedSkillRoot) -> None:
    """Validate the I/O tree while keeping failures anchored to canonical identity."""
    try:
        is_directory = root.io_dir.is_dir()
    except OSError as exc:
        raise ValueError(
            f"skill directory is not accessible: {root.canonical_dir}"
        ) from exc
    if not is_directory:
        raise ValueError(
            f"skill directory does not exist or is not a directory: {root.canonical_dir}"
        )
    try:
        has_manifest = (root.io_dir / "SKILL.md").is_file()
    except OSError as exc:
        raise ValueError(
            f"SKILL.md is not accessible in skill directory: {root.canonical_dir}"
        ) from exc
    if not has_manifest:
        raise ValueError(f"SKILL.md not found in skill directory: {root.canonical_dir}")


def skill_root_manageability(root: ResolvedSkillRoot) -> tuple[bool, str]:
    """Return whether Ledger may update this canonical skill's state."""
    if not is_managed_covered(root.canonical_dir):
        return False, "canonical skill root is not configured in managedSkillDirs"
    return ledger_update_access(root)


def ledger_update_access(root: ResolvedSkillRoot) -> tuple[bool, str]:
    """Return whether the current process can update resolved ledger state."""
    meta = root.io_dir / ".skill-meta"
    writable_target = meta if meta.exists() else root.io_dir
    canonical_target = root.canonical_path(writable_target)
    if os.access(writable_target, os.W_OK):
        return True, "canonical skill root is managed and ledger state is writable"
    if meta.exists():
        return False, f"ledger metadata is not writable: {canonical_target}"
    return (
        False,
        f"skill root is not writable for ledger bootstrap: {root.canonical_dir}",
    )


def _public_resolver_failure_reason(exc: Exception) -> str:
    if isinstance(exc, (TimeoutError, socket.timeout)):
        return "SkillFS resolver timed out"
    if isinstance(exc, PermissionError):
        return "SkillFS resolver access was denied"
    if isinstance(exc, ConnectionRefusedError):
        return "SkillFS resolver refused the connection"
    if isinstance(exc, _ResolverProtocolError):
        return str(exc)
    return "SkillFS resolver request failed"
