"""Unit tests for canonical Skill Ledger root resolution."""

import json
import socket
import threading
import time
from pathlib import Path
from typing import Any

import pytest
from agent_sec_cli.skill_ledger.core import live_root as live_root_module
from agent_sec_cli.skill_ledger.core.live_root import (
    ResolvedSkillRoot,
    SkillFsResolverClient,
    SkillRootResolver,
    resolve_skill_root,
    skill_root_manageability,
)
from agent_sec_cli.skill_ledger.errors import SkillRootResolveError
from agent_sec_cli.skill_ledger.path_identity import (
    normalize_canonical_skill_dir,
    validate_canonical_skill_dir,
)


def _make_skill(parent: Path, name: str) -> Path:
    skill_dir = parent / name
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text("# Test skill\n", encoding="utf-8")
    return skill_dir


def _start_server(
    socket_path: Path,
    response: dict[str, Any] | bytes,
    *,
    delay_seconds: float = 0,
) -> tuple[list[dict[str, Any]], threading.Thread]:
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    listener.bind(str(socket_path))
    listener.listen(1)
    requests: list[dict[str, Any]] = []

    def serve() -> None:
        with listener:
            connection, _ = listener.accept()
            with connection:
                with connection.makefile("rb") as request_stream:
                    line = request_stream.readline()
                requests.append(json.loads(line.decode("utf-8")))
                if delay_seconds:
                    time.sleep(delay_seconds)
                payload = (
                    response
                    if isinstance(response, bytes)
                    else json.dumps(response).encode("utf-8") + b"\n"
                )
                try:
                    connection.sendall(payload)
                except BrokenPipeError:
                    pass

    thread = threading.Thread(target=serve, daemon=True)
    thread.start()
    return requests, thread


def _resolver(socket_path: Path, *, timeout_seconds: float = 1.0) -> SkillRootResolver:
    return SkillRootResolver(
        SkillFsResolverClient(socket_path, timeout_seconds=timeout_seconds)
    )


def test_default_socket_follows_effective_uid(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(live_root_module.os, "geteuid", lambda: 1234)

    assert live_root_module.default_skillfs_control_socket() == Path(
        "/run/user/1234/skillfs/control.sock"
    )


@pytest.mark.parametrize("value", ["//skills/weather", "///skills/weather"])
def test_canonical_paths_reject_multiple_leading_separators(value: str) -> None:
    with pytest.raises(ValueError, match="single leading"):
        normalize_canonical_skill_dir(value)
    with pytest.raises(ValueError, match="single leading"):
        validate_canonical_skill_dir(value)


def test_resolved_root_projects_nested_payload_paths_with_boundaries(
    tmp_path: Path,
) -> None:
    canonical = tmp_path / "mount" / "weather"
    live = tmp_path / "backing" / "weather"
    sibling_paths = {
        "numeric": Path(f"{live}2") / "data.json",
        "dash": Path(f"{live}-archive") / "data.json",
        "underscore": Path(f"{live}_archive") / "data.json",
        "dot": Path(f"{live}.old") / "data.json",
    }
    outside = tmp_path / "other" / "data.json"
    root = ResolvedSkillRoot(canonical, live, "skillfs")
    payload = {
        "exact": str(live),
        "quoted": f"path '{live}' failed",
        "colon": f"{live}: permission denied",
        "uri": f"file://{live}/secret.txt",
        "child": str(live / "scripts" / "run.py"),
        "nested": [
            f"failed to read '{live / 'scripts' / 'run.py'}'",
            {
                "siblings": {name: str(path) for name, path in sibling_paths.items()},
                "outside": str(outside),
            },
        ],
    }

    assert root.contains_io_path(payload)
    projected = root.canonicalize_payload(payload)

    assert projected == {
        "exact": str(canonical),
        "quoted": f"path '{canonical}' failed",
        "colon": f"{canonical}: permission denied",
        "uri": f"file://{canonical}/secret.txt",
        "child": str(canonical / "scripts" / "run.py"),
        "nested": [
            f"failed to read '{canonical / 'scripts' / 'run.py'}'",
            {
                "siblings": {name: str(path) for name, path in sibling_paths.items()},
                "outside": str(outside),
            },
        ],
    }
    assert not root.contains_io_path(projected)
    assert not root.contains_io_path(
        {name: str(path) for name, path in sibling_paths.items()}
    )
    assert root.contains_io_path({str(live): "path used as a metadata key"})


def test_resolved_root_projects_symlink_resolved_io_alias(tmp_path: Path) -> None:
    canonical = tmp_path / "mount" / "weather"
    physical = _make_skill(tmp_path / "backing", "weather")
    live_alias = tmp_path / "live-weather"
    live_alias.symlink_to(physical, target_is_directory=True)
    root = ResolvedSkillRoot(canonical, live_alias, "skillfs")
    physical_file = physical / "scripts" / "run.py"

    projected = root.canonicalize_payload(
        {"error": f"failed to read '{physical_file}'"}
    )

    assert projected == {
        "error": f"failed to read '{canonical / 'scripts' / 'run.py'}'"
    }
    assert root.canonical_path(physical_file) == canonical / "scripts" / "run.py"
    assert not root.contains_io_path(projected)


def test_resolver_returns_skillfs_live_source(tmp_path: Path) -> None:
    canonical = tmp_path / "mount" / "apple" / "notes"
    live = _make_skill(tmp_path / "backing" / "apple", "notes")
    socket_path = tmp_path / "skillfs.sock"
    requests, thread = _start_server(
        socket_path,
        {
            "schemaVersion": "1",
            "ok": True,
            "result": {
                "managed": True,
                "canonicalSkillDir": str(canonical),
                "liveSkillDir": str(live),
            },
        },
    )

    root = _resolver(socket_path).resolve(canonical)
    thread.join(timeout=1)

    assert root == ResolvedSkillRoot(canonical, live, "skillfs")
    assert requests == [
        {
            "schemaVersion": "1",
            "method": "skill.resolveLiveSource",
            "canonicalSkillDir": str(canonical),
        }
    ]


def test_resolver_uses_host_when_socket_is_missing(tmp_path: Path) -> None:
    canonical = _make_skill(tmp_path, "weather")

    root = _resolver(tmp_path / "missing.sock").resolve(canonical)

    assert root == ResolvedSkillRoot(canonical, canonical, "host")


def test_resolver_uses_host_for_explicit_not_managed(tmp_path: Path) -> None:
    canonical = _make_skill(tmp_path, "weather")
    socket_path = tmp_path / "skillfs.sock"
    _, thread = _start_server(
        socket_path,
        {
            "schemaVersion": "1",
            "ok": True,
            "result": {
                "managed": False,
                "canonicalSkillDir": str(canonical),
                "reason": "not_managed",
            },
        },
    )

    root = _resolver(socket_path).resolve(canonical)
    thread.join(timeout=1)

    assert root == ResolvedSkillRoot(canonical, canonical, "host")


def test_resolver_does_not_fallback_on_connection_refused(tmp_path: Path) -> None:
    socket_path = tmp_path / "skillfs.sock"
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    listener.bind(str(socket_path))
    listener.close()

    with pytest.raises(SkillRootResolveError, match="refused") as exc_info:
        _resolver(socket_path).resolve(tmp_path / "weather")

    assert exc_info.value.reason_code == "skill_root_resolve_failed"


@pytest.mark.parametrize(
    ("error", "message"),
    [
        (PermissionError("denied"), "access was denied"),
        (OSError("broken"), "request failed"),
    ],
)
def test_resolver_does_not_fallback_on_client_errors(
    tmp_path: Path,
    error: OSError,
    message: str,
) -> None:
    class FailingClient:
        def resolve(self, _canonical_dir: Path) -> Path | None:
            raise error

    with pytest.raises(SkillRootResolveError, match=message):
        SkillRootResolver(FailingClient()).resolve(tmp_path / "weather")  # type: ignore[arg-type]


def test_resolver_does_not_fallback_on_timeout(tmp_path: Path) -> None:
    canonical = tmp_path / "weather"
    socket_path = tmp_path / "skillfs.sock"
    _, thread = _start_server(
        socket_path,
        {"schemaVersion": "1", "ok": True, "result": {}},
        delay_seconds=0.1,
    )

    with pytest.raises(SkillRootResolveError, match="timed out"):
        _resolver(socket_path, timeout_seconds=0.01).resolve(canonical)
    thread.join(timeout=1)


@pytest.mark.parametrize(
    "response",
    [
        b"not-json\n",
        {"schemaVersion": "2", "ok": True, "result": {}},
        {"schemaVersion": "1", "ok": "yes", "result": {}},
        {
            "schemaVersion": "1",
            "ok": False,
            "error": {"code": "permission_denied", "message": "denied"},
        },
        {
            "schemaVersion": "1",
            "ok": False,
            "error": {"code": "not_managed", "message": "not managed"},
        },
    ],
)
def test_resolver_rejects_invalid_or_error_response(
    tmp_path: Path,
    response: dict[str, Any] | bytes,
) -> None:
    socket_path = tmp_path / "skillfs.sock"
    _, thread = _start_server(socket_path, response)

    with pytest.raises(SkillRootResolveError):
        _resolver(socket_path).resolve(tmp_path / "weather")
    thread.join(timeout=1)


@pytest.mark.parametrize(
    ("include_managed", "managed"),
    [
        (False, None),
        (True, "true"),
        (True, 1),
    ],
)
def test_resolver_rejects_malformed_managed(
    tmp_path: Path,
    include_managed: bool,
    managed: Any,
) -> None:
    canonical = tmp_path / "mount" / "weather"
    result: dict[str, Any] = {"canonicalSkillDir": str(canonical)}
    if include_managed:
        result["managed"] = managed
    socket_path = tmp_path / "skillfs.sock"
    _, thread = _start_server(
        socket_path,
        {"schemaVersion": "1", "ok": True, "result": result},
    )

    with pytest.raises(SkillRootResolveError, match="managed must be a boolean"):
        _resolver(socket_path).resolve(canonical)
    thread.join(timeout=1)


@pytest.mark.parametrize(
    "case",
    [
        "echo",
        "relative_live",
        "missing_live",
        "double_slash_echo",
        "double_slash_live",
    ],
)
def test_resolver_validates_success_paths(tmp_path: Path, case: str) -> None:
    canonical = tmp_path / "mount" / "weather"
    live = _make_skill(tmp_path / "backing", "weather")
    echoed = canonical if case != "echo" else tmp_path / "other"
    live_value = str(live)
    if case == "relative_live":
        live_value = "relative/live"
    elif case == "missing_live":
        live_value = str(tmp_path / "missing")
    elif case == "double_slash_echo":
        echoed = Path("//skills/weather")
    elif case == "double_slash_live":
        live_value = "//backing/weather"
    socket_path = tmp_path / "skillfs.sock"
    _, thread = _start_server(
        socket_path,
        {
            "schemaVersion": "1",
            "ok": True,
            "result": {
                "managed": True,
                "canonicalSkillDir": str(echoed),
                "liveSkillDir": live_value,
            },
        },
    )

    with pytest.raises(SkillRootResolveError):
        _resolver(socket_path).resolve(canonical)
    thread.join(timeout=1)


def test_resolver_validates_not_managed_echo(tmp_path: Path) -> None:
    canonical = tmp_path / "mount" / "weather"
    socket_path = tmp_path / "skillfs.sock"
    _, thread = _start_server(
        socket_path,
        {
            "schemaVersion": "1",
            "ok": True,
            "result": {
                "managed": False,
                "canonicalSkillDir": str(tmp_path / "other"),
                "reason": "not_managed",
            },
        },
    )

    with pytest.raises(SkillRootResolveError, match="canonical path echo mismatch"):
        _resolver(socket_path).resolve(canonical)
    thread.join(timeout=1)


def test_nested_same_basename_roots_keep_canonical_identity(tmp_path: Path) -> None:
    roots: list[ResolvedSkillRoot] = []

    for category in ("apple", "google"):
        canonical = tmp_path / "mount" / category / "notes"
        live = _make_skill(tmp_path / "backing" / category, "notes")
        socket_path = tmp_path / f"{category}.sock"
        _, thread = _start_server(
            socket_path,
            {
                "schemaVersion": "1",
                "ok": True,
                "result": {
                    "managed": True,
                    "canonicalSkillDir": str(canonical),
                    "liveSkillDir": str(live),
                },
            },
        )

        roots.append(_resolver(socket_path).resolve(canonical))
        thread.join(timeout=1)

    assert roots[0].canonical_dir != roots[1].canonical_dir
    assert roots[0].skill_name == roots[1].skill_name == "notes"
    assert roots[0].io_dir != roots[1].io_dir


def test_resolver_preserves_symlink_canonical_root(tmp_path: Path) -> None:
    physical_root = tmp_path / "physical"
    physical_root.mkdir()
    canonical_root = tmp_path / "skills"
    canonical_root.symlink_to(physical_root, target_is_directory=True)
    canonical = canonical_root / "mlops" / "axolotl"
    live = _make_skill(tmp_path / "backing" / "mlops", "axolotl")
    socket_path = tmp_path / "skillfs.sock"
    requests, thread = _start_server(
        socket_path,
        {
            "schemaVersion": "1",
            "ok": True,
            "result": {
                "managed": True,
                "canonicalSkillDir": str(canonical),
                "liveSkillDir": str(live),
            },
        },
    )

    root = _resolver(socket_path).resolve(canonical)
    thread.join(timeout=1)

    assert root.canonical_dir == canonical
    assert root.canonical_dir != canonical.resolve()
    assert requests[0]["canonicalSkillDir"] == str(canonical)


def test_existing_context_is_reused_without_resolving(tmp_path: Path) -> None:
    canonical = tmp_path / "mount" / "weather"
    live = _make_skill(tmp_path / "backing", "weather")
    root = ResolvedSkillRoot(canonical, live, "skillfs")

    assert resolve_skill_root(root) is root


def test_manageability_matches_hidden_canonical_path(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    canonical = tmp_path / "mount" / "apple" / "notes"
    live = _make_skill(tmp_path / "backing" / "apple", "notes")
    config_dir = tmp_path / "config" / "agent-sec" / "skill-ledger"
    config_dir.mkdir(parents=True)
    (config_dir / "config.json").write_text(
        json.dumps(
            {
                "enableDefaultSkillDirs": False,
                "managedSkillDirs": [str(canonical)],
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))

    managed, reason = skill_root_manageability(
        ResolvedSkillRoot(canonical, live, "skillfs")
    )

    assert managed is True
    assert "writable" in reason
