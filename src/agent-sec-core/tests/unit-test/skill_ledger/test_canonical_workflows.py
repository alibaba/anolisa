"""Workflow tests for canonical identity with separate SkillFS I/O roots."""

import json
from pathlib import Path

import pytest
from agent_sec_cli.skill_ledger.core import certifier as certifier_core
from agent_sec_cli.skill_ledger.core import resolver as resolver_core
from agent_sec_cli.skill_ledger.core.auditor import audit
from agent_sec_cli.skill_ledger.core.certifier import (
    certify,
    scan_batch,
    scan_skill,
)
from agent_sec_cli.skill_ledger.core.checker import check
from agent_sec_cli.skill_ledger.core.decision import export_skill, show_skill
from agent_sec_cli.skill_ledger.core.live_root import (
    ResolvedSkillRoot,
    SkillRootResolver,
)
from agent_sec_cli.skill_ledger.core.resolver import resolve_activation
from agent_sec_cli.skill_ledger.errors import SkillLedgerError
from agent_sec_cli.skill_ledger.signing.ed25519 import NativeEd25519Backend


def _make_skill(parent: Path, name: str, marker: str) -> Path:
    skill_dir = parent / name
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text(
        f"---\nname: {name}\ndescription: Test skill\n---\n# {marker}\n",
        encoding="utf-8",
    )
    return skill_dir


def _write_config(tmp_path: Path, canonical_dirs: list[Path]) -> Path:
    config_path = tmp_path / "config" / "agent-sec" / "skill-ledger" / "config.json"
    config_path.parent.mkdir(parents=True)
    config_path.write_text(
        json.dumps(
            {
                "enableDefaultSkillDirs": False,
                "managedSkillDirs": [str(path) for path in canonical_dirs],
            }
        ),
        encoding="utf-8",
    )
    return config_path


def _write_findings(tmp_path: Path, name: str) -> Path:
    path = tmp_path / f"{name}.json"
    path.write_text(
        json.dumps([{"rule": "safe", "level": "pass", "message": "safe"}]),
        encoding="utf-8",
    )
    return path


def _backend(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> NativeEd25519Backend:
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path / "data"))
    backend = NativeEd25519Backend()
    backend.generate_keys()
    return backend


def test_nested_same_basename_skills_keep_canonical_identity_and_live_io(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    apple_canonical = tmp_path / "mount" / "apple" / "notes"
    google_canonical = tmp_path / "mount" / "google" / "notes"
    apple_live = _make_skill(tmp_path / "backing" / "apple", "notes", "apple")
    google_live = _make_skill(tmp_path / "backing" / "google", "notes", "google")
    apple_root = ResolvedSkillRoot(apple_canonical, apple_live, "skillfs")
    google_root = ResolvedSkillRoot(google_canonical, google_live, "skillfs")
    config_path = _write_config(tmp_path, [apple_canonical, google_canonical])
    backend = _backend(tmp_path, monkeypatch)

    apple_result = certify(
        apple_root,
        backend,
        findings_path=str(_write_findings(tmp_path, "apple")),
    )
    google_result = certify(
        google_root,
        backend,
        findings_path=str(_write_findings(tmp_path, "google")),
    )
    apple_check = check(apple_root, backend)
    google_check = check(google_root, backend)
    activation = resolve_activation(apple_root, backend)
    shown = show_skill(apple_root, backend)

    assert apple_result["skillName"] == google_result["skillName"] == "notes"
    assert apple_check["canonicalSkillDir"] == str(apple_canonical)
    assert google_check["canonicalSkillDir"] == str(google_canonical)
    assert activation["activationPath"] == str(
        apple_canonical / ".skill-meta" / "activation.json"
    )
    assert shown["canonicalSkillDir"] == str(apple_canonical)
    assert (apple_live / ".skill-meta" / "activation.json").is_file()
    assert not apple_canonical.exists()
    assert not google_canonical.exists()

    config = json.loads(config_path.read_text(encoding="utf-8"))
    assert config["managedSkillDirs"] == [
        str(apple_canonical),
        str(google_canonical),
    ]
    public_results = json.dumps(
        [apple_result, google_result, apple_check, google_check, activation, shown]
    )
    assert str(apple_live) not in public_results
    assert str(google_live) not in public_results

    apple_manifest = json.loads(
        (apple_live / ".skill-meta" / "latest.json").read_text(encoding="utf-8")
    )
    google_manifest = json.loads(
        (google_live / ".skill-meta" / "latest.json").read_text(encoding="utf-8")
    )
    assert apple_manifest["skillName"] == google_manifest["skillName"] == "notes"
    assert "canonicalSkillDir" not in apple_manifest


def test_activation_resolves_once_and_reuses_context(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    canonical = tmp_path / "mount" / "apple" / "notes"
    live = _make_skill(tmp_path / "backing" / "apple", "notes", "apple")
    root = ResolvedSkillRoot(canonical, live, "skillfs")
    _write_config(tmp_path, [canonical])
    backend = _backend(tmp_path, monkeypatch)
    certify(
        root,
        backend,
        findings_path=str(_write_findings(tmp_path, "notes")),
    )
    calls: list[Path] = []

    def fake_resolve(
        _resolver: SkillRootResolver,
        canonical_skill_dir: str | Path,
    ) -> ResolvedSkillRoot:
        calls.append(Path(canonical_skill_dir))
        return root

    monkeypatch.setattr(SkillRootResolver, "resolve", fake_resolve)

    result = resolve_activation(str(canonical), backend)

    assert calls == [canonical]
    assert result["canonicalSkillDir"] == str(canonical)


def test_batch_error_exposes_only_canonical_path(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    canonical = tmp_path / "mount" / "apple" / "notes"
    live = _make_skill(tmp_path / "backing" / "apple", "notes", "apple")
    root = ResolvedSkillRoot(canonical, live, "skillfs")
    backend = _backend(tmp_path, monkeypatch)
    calls: list[Path] = []

    def fake_resolve(
        _resolver: SkillRootResolver,
        canonical_skill_dir: str | Path,
    ) -> ResolvedSkillRoot:
        calls.append(Path(canonical_skill_dir))
        return root

    def fail_hashing(_skill_dir: str | Path) -> dict[str, str]:
        raise OSError(f"cannot read {live / 'secret.txt'}")

    monkeypatch.setattr(SkillRootResolver, "resolve", fake_resolve)
    monkeypatch.setattr(certifier_core, "compute_file_hashes", fail_hashing)

    result = scan_batch([canonical], backend)

    assert calls == [canonical]
    assert result[0]["status"] == "error"
    assert result[0]["canonicalSkillDir"] == str(canonical)
    assert str(canonical / "secret.txt") in result[0]["error"]
    assert str(live) not in json.dumps(result)


def test_scanner_error_paths_are_canonicalized_before_manifest_signing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    canonical = tmp_path / "mount" / "weather"
    live = _make_skill(tmp_path / "backing", "weather", "weather")
    code_path = live / "__init__.py"
    code_path.write_text("print('weather')\n", encoding="utf-8")
    root = ResolvedSkillRoot(canonical, live, "skillfs")
    _write_config(tmp_path, [canonical])
    backend = _backend(tmp_path, monkeypatch)
    original_read_text = Path.read_text

    def read_text_with_permission_error(path: Path, *args, **kwargs):
        if path == code_path:
            raise PermissionError(13, "Permission denied", str(code_path))
        return original_read_text(path, *args, **kwargs)

    monkeypatch.setattr(Path, "read_text", read_text_with_permission_error)

    result = scan_skill(
        root,
        backend,
        scanner_names=["code-scanner"],
        force=True,
    )
    checked = check(root, backend)
    shown = show_skill(root, backend)
    export_dir = tmp_path / "export"
    export_skill(
        root,
        backend,
        version=result["versionId"],
        output=str(export_dir),
    )

    latest_text = (live / ".skill-meta" / "latest.json").read_text(encoding="utf-8")
    version_text = (
        live / ".skill-meta" / "versions" / f"{result['versionId']}.json"
    ).read_text(encoding="utf-8")
    exported_manifest = (export_dir / "manifest.json").read_text(encoding="utf-8")
    exported_findings = (export_dir / "findings.json").read_text(encoding="utf-8")
    public_payload = json.dumps([checked, shown])

    for content in (
        latest_text,
        version_text,
        exported_manifest,
        exported_findings,
        public_payload,
    ):
        assert str(live) not in content
        assert str(canonical / "__init__.py") in content
    assert checked["status"] == "warn"


def test_signed_findings_project_diagnostic_path_contexts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    canonical = tmp_path / "mount" / "weather"
    live = _make_skill(tmp_path / "backing", "weather", "weather")
    root = ResolvedSkillRoot(canonical, live, "skillfs")
    _write_config(tmp_path, [canonical])
    backend = _backend(tmp_path, monkeypatch)
    findings_path = tmp_path / "findings.json"
    findings_path.write_text(
        json.dumps(
            [
                {
                    "rule": "diagnostic-paths",
                    "level": "warn",
                    "message": f"path '{live}' failed",
                    "metadata": {
                        "colon": f"{live}: permission denied",
                        "uri": f"file://{live}/secret.txt",
                    },
                }
            ]
        ),
        encoding="utf-8",
    )

    result = certify(root, backend, findings_path=str(findings_path))
    checked = check(root, backend)
    shown = show_skill(root, backend)
    export_dir = tmp_path / "export"
    export_skill(
        root,
        backend,
        version=result["versionId"],
        output=str(export_dir),
    )

    persisted_payloads = [
        (live / ".skill-meta" / "latest.json").read_text(encoding="utf-8"),
        (live / ".skill-meta" / "versions" / f"{result['versionId']}.json").read_text(
            encoding="utf-8"
        ),
        (export_dir / "manifest.json").read_text(encoding="utf-8"),
        (export_dir / "findings.json").read_text(encoding="utf-8"),
        json.dumps([checked, shown]),
    ]
    for payload in persisted_payloads:
        assert str(live) not in payload
        assert f"path '{canonical}' failed" in payload
        assert f"{canonical}: permission denied" in payload
        assert f"file://{canonical}/secret.txt" in payload
    assert checked["status"] == "warn"


def test_activation_xattr_error_projects_exact_live_root(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    canonical = tmp_path / "mount" / "weather"
    live = _make_skill(tmp_path / "backing", "weather", "weather")
    root = ResolvedSkillRoot(canonical, live, "skillfs")
    _write_config(tmp_path, [canonical])
    backend = _backend(tmp_path, monkeypatch)
    certify(root, backend, findings_path=str(_write_findings(tmp_path, "weather")))

    def fail_setxattr(_path: str, _name: str, _payload: bytes) -> None:
        raise PermissionError(13, "Permission denied", str(live))

    monkeypatch.setattr(resolver_core.os, "setxattr", fail_setxattr, raising=False)

    result = resolve_activation(root, backend)

    error = result["activationXattr"]["error"]
    assert str(canonical) in error
    assert str(live) not in error


def test_unprojectable_io_path_fails_before_snapshot_creation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    canonical = tmp_path / "mount" / "weather"
    live = _make_skill(tmp_path / "backing", "weather", "weather")
    root = ResolvedSkillRoot(canonical, live, "skillfs")
    _write_config(tmp_path, [canonical])
    backend = _backend(tmp_path, monkeypatch)
    findings_path = tmp_path / "findings.json"
    findings_path.write_text(
        json.dumps(
            [
                {
                    "rule": "path-key",
                    "level": "warn",
                    "message": "scanner returned an unsupported path key",
                    "metadata": {str(live): "cannot safely rewrite metadata keys"},
                }
            ]
        ),
        encoding="utf-8",
    )

    with pytest.raises(SkillLedgerError, match="internal I/O path") as exc_info:
        certify(root, backend, findings_path=str(findings_path))

    assert str(canonical) in str(exc_info.value)
    assert str(live) not in str(exc_info.value)
    assert not (live / ".skill-meta" / "latest.json").exists()
    assert not list((live / ".skill-meta" / "versions").glob("*.snapshot"))


@pytest.mark.parametrize(
    "corruption",
    [
        "exact-alias-action",
        "resolved-physical-version",
        "file-hash-value-type",
        "latest-version-id",
    ],
)
def test_audit_projects_io_paths_from_corrupted_manifests(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    corruption: str,
) -> None:
    canonical = tmp_path / "mount" / "weather"
    physical = _make_skill(tmp_path / "backing", "weather", "weather")
    live_alias = tmp_path / "resolved" / "weather"
    live_alias.parent.mkdir()
    live_alias.symlink_to(physical, target_is_directory=True)
    root = ResolvedSkillRoot(canonical, live_alias, "skillfs")
    _write_config(tmp_path, [canonical])
    backend = _backend(tmp_path, monkeypatch)
    certified = certify(
        root,
        backend,
        findings_path=str(_write_findings(tmp_path, "weather")),
    )
    version_path = (
        live_alias / ".skill-meta" / "versions" / f"{certified['versionId']}.json"
    )
    manifest_path = (
        live_alias / ".skill-meta" / "latest.json"
        if corruption == "latest-version-id"
        else version_path
    )
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))

    if corruption == "exact-alias-action":
        manifest["userDecision"] = {"action": str(live_alias)}
    elif corruption == "resolved-physical-version":
        manifest["version"] = str(physical.resolve())
    elif corruption == "file-hash-value-type":
        manifest["fileHashes"] = {str(physical.resolve()): 7}
    else:
        manifest["versionId"] = str(physical.resolve())
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

    result = audit(root, backend)

    serialized = json.dumps(result)
    serialized_errors = json.dumps(result["errors"])
    assert result["valid"] is False
    assert result["errors"]
    assert str(canonical) in serialized
    if corruption == "latest-version-id":
        assert str(canonical) in serialized_errors
    assert str(live_alias) not in serialized
    assert str(physical.resolve()) not in serialized
    assert "resolved/weather" not in serialized
    assert "backing/weather" not in serialized
    assert not root.contains_io_path(result)
