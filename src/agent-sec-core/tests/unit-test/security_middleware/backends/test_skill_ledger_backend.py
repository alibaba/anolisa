"""Unit tests for security_middleware.backends.skill_ledger."""

import json
from copy import deepcopy

from agent_sec_cli.security_middleware.backends import (
    skill_ledger as backend_module,
)
from agent_sec_cli.security_middleware.backends.skill_ledger import (
    SkillLedgerBackend,
)
from agent_sec_cli.security_middleware.result import ActionResult


def _event_result(data: dict) -> dict:
    backend = SkillLedgerBackend()
    return backend.build_event_details(
        ActionResult(success=True, data=data),
        {"command": data["command"]},
    )["result"]


def test_check_event_result_keeps_skill_ledger_status_contract():
    warn_result = _event_result(
        {
            "command": "check",
            "status": "warn",
            "skillName": "demo",
            "versionId": "v000001",
            "createdAt": "2026-06-17T00:00:00+00:00",
            "updatedAt": "2026-06-17T00:01:00+00:00",
            "fileCount": 3,
            "manifestHash": "sha256:abc",
            "findings": [
                {
                    "rule": "hidden-file",
                    "level": "warn",
                    "message": "Hidden file detected",
                    "file": ".install-complete",
                    "metadata": {"sourceSeverity": "medium"},
                }
            ],
        }
    )
    tampered_result = _event_result(
        {
            "command": "check",
            "status": "tampered",
            "skillName": "demo",
            "versionId": "v000001",
            "createdAt": "2026-06-17T00:00:00+00:00",
            "updatedAt": "2026-06-17T00:01:00+00:00",
            "fileCount": 3,
            "manifestHash": "sha256:abc",
            "reason": "manifestHash does not match",
        }
    )

    assert warn_result == {
        "command": "check",
        "status": "warn",
        "skill_name": "demo",
        "version_id": "v000001",
        "created_at": "2026-06-17T00:00:00+00:00",
        "updated_at": "2026-06-17T00:01:00+00:00",
        "file_count": 3,
        "manifest_hash": "sha256:abc",
        "findings": [
            {
                "rule": "hidden-file",
                "level": "warn",
                "message": "Hidden file detected",
                "file": ".install-complete",
                "metadata": {"sourceSeverity": "medium"},
            }
        ],
    }
    assert tampered_result == {
        "command": "check",
        "status": "tampered",
        "skill_name": "demo",
        "version_id": "v000001",
        "created_at": "2026-06-17T00:00:00+00:00",
        "updated_at": "2026-06-17T00:01:00+00:00",
        "file_count": 3,
        "manifest_hash": "sha256:abc",
        "reason": "manifestHash does not match",
    }


def test_scan_and_certify_event_results_use_scan_verdict_contract():
    scan_result = _event_result(
        {
            "command": "scan",
            "status": "scanned",
            "versionId": "v000001",
            "scanStatus": "warn",
            "newVersion": True,
            "skillName": "demo",
            "createdAt": "2026-06-17T00:00:00+00:00",
            "updatedAt": "2026-06-17T00:01:00+00:00",
            "fileCount": 3,
            "manifestHash": "sha256:abc",
            "scannersRun": ["code-scanner", "static-scanner"],
            "skippedScanners": [],
            "keyCreated": True,
            "key": {
                "fingerprint": "sha256:key",
                "publicKeyPath": "/keys/pub",
                "privateKeyPath": "/keys/private",
                "encrypted": False,
            },
            "auditEvents": [
                {
                    "type": "tampered_recovered",
                    "operation": "scan",
                    "fromStatus": "tampered",
                    "toStatus": "warn",
                    "versionId": "v000001",
                    "manifestHash": "sha256:abc",
                    "scannersRun": ["code-scanner"],
                }
            ],
        }
    )
    certify_result = _event_result(
        {
            "command": "certify",
            "status": "scanned",
            "versionId": "v000002",
            "scanStatus": "pass",
            "newVersion": False,
            "skillName": "demo",
            "createdAt": "2026-06-17T00:00:00+00:00",
            "updatedAt": "2026-06-17T00:02:00+00:00",
            "fileCount": 3,
            "manifestHash": "sha256:def",
            "scannersRun": ["skill-vetter"],
        }
    )

    assert scan_result == {
        "command": "scan",
        "status": "scanned",
        "version_id": "v000001",
        "verdict": "warn",
        "new_version": True,
        "skill_name": "demo",
        "created_at": "2026-06-17T00:00:00+00:00",
        "updated_at": "2026-06-17T00:01:00+00:00",
        "file_count": 3,
        "manifest_hash": "sha256:abc",
        "scanners_run": ["code-scanner", "static-scanner"],
        "skipped_scanners": [],
        "key_created": True,
        "key": {
            "fingerprint": "sha256:key",
            "public_key_path": "/keys/pub",
            "private_key_path": "/keys/private",
            "encrypted": False,
        },
        "audit_events": [
            {
                "type": "tampered_recovered",
                "operation": "scan",
                "from_status": "tampered",
                "to_status": "warn",
                "version_id": "v000001",
                "manifest_hash": "sha256:abc",
                "scanners_run": ["code-scanner"],
            }
        ],
    }
    assert certify_result == {
        "command": "certify",
        "status": "scanned",
        "version_id": "v000002",
        "verdict": "pass",
        "new_version": False,
        "skill_name": "demo",
        "created_at": "2026-06-17T00:00:00+00:00",
        "updated_at": "2026-06-17T00:02:00+00:00",
        "file_count": 3,
        "manifest_hash": "sha256:def",
        "scanners_run": ["skill-vetter"],
    }


def test_batch_event_results_keep_command_and_child_result_contracts():
    scan_all_result = _event_result(
        {
            "command": "scan",
            "keyCreated": False,
            "results": [
                {
                    "status": "scanned",
                    "skillName": "a",
                    "versionId": "v000001",
                    "scanStatus": "pass",
                },
                {
                    "status": "scanned",
                    "skillName": "b",
                    "versionId": "v000001",
                    "scanStatus": "deny",
                },
            ],
        }
    )
    init_result = _event_result(
        {
            "command": "init",
            "keyCreated": True,
            "baseline": True,
            "results": [
                {
                    "status": "scanned",
                    "skillName": "baseline",
                    "versionId": "v000001",
                    "scanStatus": "warn",
                }
            ],
        }
    )

    assert scan_all_result == {
        "command": "scan",
        "key_created": False,
        "results": [
            {
                "status": "scanned",
                "skill_name": "a",
                "version_id": "v000001",
                "verdict": "pass",
            },
            {
                "status": "scanned",
                "skill_name": "b",
                "version_id": "v000001",
                "verdict": "deny",
            },
        ],
    }
    assert init_result == {
        "command": "init",
        "key_created": True,
        "baseline": True,
        "results": [
            {
                "status": "scanned",
                "skill_name": "baseline",
                "version_id": "v000001",
                "verdict": "warn",
            }
        ],
    }


def test_non_scan_commands_normalize_names_without_changing_business_meaning():
    list_scanners_result = _event_result(
        {
            "command": "list-scanners",
            "scanners": [
                {
                    "name": "static-scanner",
                    "type": "builtin",
                    "parser": "normalized-findings",
                    "enabled": True,
                    "autoInvocable": True,
                    "description": "Static checks",
                }
            ],
        }
    )
    init_keys_result = _event_result(
        {
            "command": "init-keys",
            "fingerprint": "sha256:key",
            "publicKeyPath": "/keys/pub",
            "privateKeyPath": "/keys/private",
            "encrypted": True,
        }
    )
    status_result = _event_result(
        {
            "command": "status",
            "keys": {
                "initialized": True,
                "fingerprint": "sha256:key",
                "publicKeyPath": "/keys/pub",
                "encrypted": False,
                "keyringSize": 1,
            },
            "config": {
                "configPath": "/config.json",
                "customized": True,
                "defaultSkillDirsEnabled": False,
                "defaultSkillDirPatterns": 3,
                "managedSkillDirPatterns": 1,
                "ignoredDeprecatedSkillDirPatterns": 0,
                "effectiveSkillDirPatterns": 1,
                "registeredScanners": ["static-scanner"],
            },
            "skills": {
                "discovered": 1,
                "breakdown": {"pass": 1},
                "health": "healthy",
            },
        }
    )

    assert list_scanners_result == {
        "command": "list-scanners",
        "scanners": [
            {
                "name": "static-scanner",
                "type": "builtin",
                "parser": "normalized-findings",
                "enabled": True,
                "auto_invocable": True,
                "description": "Static checks",
            }
        ],
    }
    assert init_keys_result == {
        "command": "init-keys",
        "fingerprint": "sha256:key",
        "public_key_path": "/keys/pub",
        "private_key_path": "/keys/private",
        "encrypted": True,
    }
    assert status_result == {
        "command": "status",
        "keys": {
            "initialized": True,
            "fingerprint": "sha256:key",
            "public_key_path": "/keys/pub",
            "encrypted": False,
            "keyring_size": 1,
        },
        "config": {
            "config_path": "/config.json",
            "customized": True,
            "default_skill_dirs_enabled": False,
            "default_skill_dir_patterns": 3,
            "managed_skill_dir_patterns": 1,
            "ignored_deprecated_skill_dir_patterns": 0,
            "effective_skill_dir_patterns": 1,
            "registered_scanners": ["static-scanner"],
        },
        "skills": {
            "discovered": 1,
            "breakdown": {"pass": 1},
            "health": "healthy",
        },
    }


def test_event_details_are_safe_copies_with_redacted_request():
    backend = SkillLedgerBackend()
    data = {
        "command": "scan",
        "status": "scanned",
        "scanStatus": "pass",
        "skillName": "demo",
        "keyCreated": True,
    }
    original = deepcopy(data)

    details = backend.build_event_details(
        ActionResult(success=True, data=data),
        {"command": "scan", "passphrase": "secret"},
    )

    assert details["request"]["passphrase"] == "[REDACTED]"
    assert details["result"] == {
        "command": "scan",
        "status": "scanned",
        "verdict": "pass",
        "skill_name": "demo",
        "key_created": True,
    }
    assert data == original


def test_decide_backend_requires_action_unless_clear(monkeypatch):
    backend = SkillLedgerBackend()

    monkeypatch.setattr(backend, "_ensure_keys", lambda: (False, None, []))
    monkeypatch.setattr(backend_module, "NativeEd25519Backend", lambda: object())

    result = backend._do_decide(None, skill_dir="/tmp/demo")

    assert result.success is False
    assert result.exit_code == 1
    assert "--action is required" in result.error


def test_decide_backend_clear_includes_key_and_warnings(monkeypatch):
    backend = SkillLedgerBackend()
    key = {
        "fingerprint": "sha256:key",
        "publicKeyPath": "/keys/pub",
        "privateKeyPath": "/keys/private",
        "encrypted": False,
    }

    monkeypatch.setattr(
        backend,
        "_ensure_keys",
        lambda: (True, key, ["created unencrypted test key"]),
    )
    monkeypatch.setattr(backend_module, "NativeEd25519Backend", lambda: object())

    def fake_clear_decision(skill_dir, signing_backend):
        return {
            "status": "decided",
            "skillName": "demo",
            "versionId": "v000001",
            "userDecision": None,
        }

    monkeypatch.setattr(backend_module, "clear_decision", fake_clear_decision)

    result = backend._do_decide(None, skill_dir="/tmp/demo", clear=True)

    assert result.success is True
    data = json.loads(result.stdout)
    assert data["command"] == "decide"
    assert data["keyCreated"] is True
    assert data["key"] == key
    assert data["warnings"] == ["created unencrypted test key"]
    assert result.data == data


def test_decide_backend_maps_core_errors_to_action_result(monkeypatch):
    backend = SkillLedgerBackend()

    monkeypatch.setattr(backend, "_ensure_keys", lambda: (False, None, []))
    monkeypatch.setattr(backend_module, "NativeEd25519Backend", lambda: object())

    def fake_decide_skill(*args, **kwargs):
        raise RuntimeError("decision failed")

    monkeypatch.setattr(backend_module, "decide_skill", fake_decide_skill)

    result = backend._do_decide(
        None,
        skill_dir="/tmp/demo",
        decision_action="allow",
    )

    assert result.success is False
    assert result.exit_code == 1
    assert result.error == "decision failed"


def test_show_backend_maps_core_errors_to_action_result(monkeypatch):
    backend = SkillLedgerBackend()

    monkeypatch.setattr(backend_module, "NativeEd25519Backend", lambda: object())

    def fake_show_skill(*args, **kwargs):
        raise RuntimeError("show failed")

    monkeypatch.setattr(backend_module, "show_skill", fake_show_skill)

    result = backend._do_show(None, skill_dir="/tmp/demo")

    assert result.success is False
    assert result.exit_code == 1
    assert result.error == "show failed"


def test_export_backend_success_and_failure(monkeypatch):
    backend = SkillLedgerBackend()

    monkeypatch.setattr(backend_module, "NativeEd25519Backend", lambda: object())

    def fake_export_skill(skill_dir, signing_backend, *, version, output, policy):
        return {
            "skillName": "demo",
            "versionId": version,
            "output": output,
            "policy": policy,
        }

    monkeypatch.setattr(backend_module, "export_skill", fake_export_skill)

    result = backend._do_export(
        None,
        skill_dir="/tmp/demo",
        version="latest",
        output="/tmp/out",
        policy="pass_warn_only",
    )

    assert result.success is True
    data = json.loads(result.stdout)
    assert data == {
        "command": "export",
        "skillName": "demo",
        "versionId": "latest",
        "output": "/tmp/out",
        "policy": "pass_warn_only",
    }
    assert result.data == data

    def fake_export_error(*args, **kwargs):
        raise RuntimeError("export failed")

    monkeypatch.setattr(backend_module, "export_skill", fake_export_error)

    failed = backend._do_export(
        None,
        skill_dir="/tmp/demo",
        version="latest",
        output="/tmp/out",
    )
    assert failed.success is False
    assert failed.exit_code == 1
    assert failed.error == "export failed"
