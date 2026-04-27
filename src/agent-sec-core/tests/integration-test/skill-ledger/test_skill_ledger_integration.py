#!/usr/bin/env python3
"""Integration tests for the ``skill-ledger`` CLI (source-tree / dev mode).

Exercises every subcommand **in-process** via Typer ``CliRunner``, verifying
**JSON stdout**, **exit codes**, and **filesystem side effects**.

Running in-process (instead of via subprocess) means ``pytest-cov`` can track
coverage of the CLI source code automatically.

This file requires the source tree — it is *not* for RPM-installed
environments.  See ``tests/e2e/skill-ledger/e2e_test.py`` for the RPM
binary end-to-end test suite.

All key material and config files are isolated via ``XDG_DATA_HOME`` and
``XDG_CONFIG_HOME`` environment variables so the host keyring is never touched.

Prerequisites: Python 3.11, source tree
"""

import hashlib
import json
import os
import shutil
import tempfile
from dataclasses import dataclass
from pathlib import Path

import pytest
from typer.testing import CliRunner

from agent_sec_cli.cli import app as cli_app


# ── Helpers ────────────────────────────────────────────────────────────────

_runner = CliRunner()


@dataclass
class _CliResult:
    """Compatibility wrapper mapping CliRunner result to subprocess-like interface."""

    returncode: int
    stdout: str
    stderr: str


def run_skill_ledger(
    args: list[str],
    env_extra: dict | None = None,
) -> _CliResult:
    """Run ``agent-sec-cli skill-ledger <args>`` in-process via Typer CliRunner.

    The *env_extra* dict is merged into ``os.environ`` for the duration of the
    invocation and automatically restored afterwards (handled by CliRunner).
    """
    result = _runner.invoke(cli_app, ["skill-ledger"] + args, env=env_extra)
    return _CliResult(
        returncode=result.exit_code,
        stdout=result.stdout,
        stderr=result.stderr,
    )


def parse_json_output(stdout: str) -> dict:
    """Parse the first JSON line from CLI stdout."""
    for line in stdout.strip().splitlines():
        line = line.strip()
        if line.startswith("{") or line.startswith("["):
            return json.loads(line)
    raise ValueError(f"No JSON found in stdout:\n{stdout}")


def make_skill(parent: Path, name: str, files: dict[str, str]) -> Path:
    """Create a fake skill directory with the given files.

    Automatically adds a minimal ``SKILL.md`` if not provided, so that
    ``validate_skill_dir()`` passes.
    """
    if "SKILL.md" not in files:
        files = {"SKILL.md": f"# {name}\nTest skill.\n", **files}
    skill_dir = parent / name
    for rel, content in files.items():
        p = skill_dir / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content)
    return skill_dir


def write_findings_file(parent: Path, name: str, findings: list | dict) -> Path:
    """Write a findings JSON file and return its path."""
    path = parent / name
    path.write_text(json.dumps(findings, ensure_ascii=False))
    return path


# ── Workspace ──────────────────────────────────────────────────────────────


class Workspace:
    """Shared test workspace: isolated XDG dirs, skills dir."""

    def __init__(self):
        self.root = Path(tempfile.mkdtemp(prefix="e2e_skill_ledger_"))
        self.xdg_data = self.root / "xdg_data"
        self.xdg_config = self.root / "xdg_config"
        self.xdg_data.mkdir()
        self.xdg_config.mkdir()
        self.skills_dir = self.root / "skills"
        self.skills_dir.mkdir()
        self.fixtures = self.root / "fixtures"
        self.fixtures.mkdir()

    def env(self, extra: dict | None = None) -> dict:
        """Return env dict with XDG isolation (for subprocess)."""
        e = {
            "XDG_DATA_HOME": str(self.xdg_data),
            "XDG_CONFIG_HOME": str(self.xdg_config),
        }
        if extra:
            e.update(extra)
        return e

    def cleanup(self):
        shutil.rmtree(self.root, ignore_errors=True)


@pytest.fixture(scope="session")
def ws():
    """Session-wide isolated workspace with keys already initialized."""
    workspace = Workspace()
    r = run_skill_ledger(["init-keys"], env_extra=workspace.env())
    assert r.returncode == 0, f"Workspace fixture init-keys failed: {r.stderr}"
    yield workspace
    workspace.cleanup()


# ── Group 1: init-keys ─────────────────────────────────────────────────────


def test_init_keys_no_passphrase(ws):
    """init-keys without passphrase → exit 0, encrypted: false."""
    alt_data = ws.root / "nopass_data"
    alt_data.mkdir()
    env = ws.env({"XDG_DATA_HOME": str(alt_data)})
    r = run_skill_ledger(["init-keys"], env_extra=env)
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out.get("encrypted") is False, f"expected encrypted=false, got {out}"
    assert out.get("fingerprint", "").startswith("sha256:"), f"bad fingerprint: {out}"


def test_init_keys_json_structure(ws):
    """JSON output must contain all 4 expected fields."""
    alt_data = ws.root / "json_struct_data"
    alt_data.mkdir()
    env = ws.env({"XDG_DATA_HOME": str(alt_data)})
    r = run_skill_ledger(["init-keys"], env_extra=env)
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    for key in ("fingerprint", "publicKeyPath", "privateKeyPath", "encrypted"):
        assert key in out, f"Missing field '{key}' in output: {out}"
    assert len(out["fingerprint"]) > 10
    assert len(out["publicKeyPath"]) > 0
    assert len(out["privateKeyPath"]) > 0


def test_init_keys_reject_duplicate(ws):
    """Second init-keys without --force → exit 1."""
    # Generate fresh keys in a separate XDG
    alt_data = ws.root / "alt_data"
    alt_data.mkdir()
    env = ws.env({"XDG_DATA_HOME": str(alt_data)})
    r1 = run_skill_ledger(["init-keys"], env_extra=env)
    assert r1.returncode == 0, f"first init failed: {r1.stderr}"

    r2 = run_skill_ledger(["init-keys"], env_extra=env)
    assert r2.returncode != 0, "Expected non-zero exit without --force"
    assert (
        "already exists" in r2.stderr.lower() or "already exists" in r2.stdout.lower()
    ), f"Expected 'already exists' message: stdout={r2.stdout}, stderr={r2.stderr}"


def test_init_keys_force_overwrite(ws):
    """--force overwrites existing keys and produces a new fingerprint."""
    alt_data = ws.root / "force_data"
    alt_data.mkdir()
    env = ws.env({"XDG_DATA_HOME": str(alt_data)})
    r1 = run_skill_ledger(["init-keys"], env_extra=env)
    assert r1.returncode == 0
    fp1 = parse_json_output(r1.stdout)["fingerprint"]

    r2 = run_skill_ledger(["init-keys", "--force"], env_extra=env)
    assert r2.returncode == 0, f"exit {r2.returncode}: {r2.stderr}"
    fp2 = parse_json_output(r2.stdout)["fingerprint"]

    # New key pair → almost certainly different fingerprint
    assert fp1 != fp2, f"Fingerprint should change after --force: {fp1}"


def test_init_keys_with_passphrase_env(ws):
    """SKILL_LEDGER_PASSPHRASE env var → encrypted: true."""
    alt_data = ws.root / "pass_data"
    alt_data.mkdir()
    env = ws.env(
        {
            "XDG_DATA_HOME": str(alt_data),
            "SKILL_LEDGER_PASSPHRASE": "test-passphrase-123",
        }
    )
    r = run_skill_ledger(["init-keys", "--passphrase"], env_extra=env)
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out.get("encrypted") is True, f"expected encrypted=true, got {out}"


# ── Group 2: Happy path lifecycle ──────────────────────────────────────────


def test_full_lifecycle_pass(ws):
    """init-keys → check (none) → certify --findings (pass) → check (pass) → audit (valid)."""
    skill = make_skill(
        ws.skills_dir,
        "lifecycle-pass",
        {
            "main.py": "print('hello')\n",
            "README.md": "# Test\n",
        },
    )
    env = ws.env()

    # check → auto-create → status=none
    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 0, f"check exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["status"] == "none", f"expected none, got {out}"

    # certify with pass findings
    findings = write_findings_file(
        ws.fixtures,
        "pass.json",
        [
            {"rule": "no-sudo", "level": "pass", "message": "No sudo found"},
        ],
    )
    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)],
        env_extra=env,
    )
    assert r.returncode == 0, f"certify exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["scanStatus"] == "pass", f"expected pass, got {out}"

    # check → pass
    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 0, f"check exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["status"] == "pass", f"expected pass, got {out}"

    # audit → valid
    r = run_skill_ledger(["audit", str(skill)], env_extra=env)
    assert r.returncode == 0, f"audit exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["valid"] is True, f"expected valid=true, got {out}"


def test_multi_version_lifecycle(ws):
    """certify → modify file → certify → audit validates 2-version chain."""
    skill = make_skill(ws.skills_dir, "multi-ver", {"data.txt": "v1"})
    env = ws.env()

    # First certify
    findings = write_findings_file(
        ws.fixtures,
        "mv-pass.json",
        [
            {"rule": "safe", "level": "pass", "message": "OK"},
        ],
    )
    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)],
        env_extra=env,
    )
    assert r.returncode == 0, f"certify1 exit {r.returncode}: {r.stderr}"
    out1 = parse_json_output(r.stdout)
    assert out1["newVersion"] is True

    # Modify file → new version
    (skill / "data.txt").write_text("v2")
    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)],
        env_extra=env,
    )
    assert r.returncode == 0, f"certify2 exit {r.returncode}: {r.stderr}"
    out2 = parse_json_output(r.stdout)
    assert out2["newVersion"] is True
    assert out2["versionId"] != out1["versionId"], "Expected different versionId"

    # audit → valid, 2 versions
    r = run_skill_ledger(["audit", str(skill)], env_extra=env)
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["valid"] is True
    assert out["versions_checked"] == 2, f"expected 2, got {out['versions_checked']}"


def test_lifecycle_with_warn_findings(ws):
    """certify with warn findings → check returns warn, exit 0."""
    skill = make_skill(ws.skills_dir, "lifecycle-warn", {"app.sh": "#!/bin/bash\n"})
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "warn.json",
        [
            {
                "rule": "shell-warning",
                "level": "warn",
                "message": "Script lacks set -e",
            },
            {"rule": "no-sudo", "level": "pass", "message": "No sudo found"},
        ],
    )
    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)],
        env_extra=env,
    )
    assert r.returncode == 0, f"certify exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["scanStatus"] == "warn", f"expected warn, got {out}"

    # check → warn (exit 0 — warn does NOT block)
    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 0, f"check should exit 0 for warn: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["status"] == "warn"


# ── Group 3: check state machine ──────────────────────────────────────────


def test_check_no_manifest_auto_creates(ws):
    """First check on new skill → auto-create manifest, status=none."""
    skill = make_skill(ws.skills_dir, "check-new", {"f.txt": "hello"})
    env = ws.env()

    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["status"] == "none"

    # .skill-meta/latest.json must exist
    latest = skill / ".skill-meta" / "latest.json"
    assert latest.exists(), f"latest.json not created: {list(skill.rglob('*'))}"


def test_check_after_file_add_drifted(ws):
    """Adding a file after certify → status=drifted."""
    skill = make_skill(ws.skills_dir, "check-add", {"original.txt": "content"})
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "add-pass.json",
        [
            {"rule": "ok", "level": "pass", "message": "pass"},
        ],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )

    # Add a new file
    (skill / "new_file.txt").write_text("I am new")

    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["status"] == "drifted", f"expected drifted, got {out}"
    assert "new_file.txt" in out.get("added", [])


def test_check_after_file_modify_drifted(ws):
    """Modifying a file after certify → status=drifted."""
    skill = make_skill(ws.skills_dir, "check-modify", {"data.txt": "original"})
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "mod-pass.json",
        [
            {"rule": "ok", "level": "pass", "message": "pass"},
        ],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )

    # Modify existing file
    (skill / "data.txt").write_text("CHANGED")

    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["status"] == "drifted"
    assert "data.txt" in out.get("modified", [])


def test_check_after_file_remove_drifted(ws):
    """Removing a file after certify → status=drifted."""
    skill = make_skill(
        ws.skills_dir,
        "check-remove",
        {
            "keep.txt": "keep",
            "delete_me.txt": "gone",
        },
    )
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "rm-pass.json",
        [
            {"rule": "ok", "level": "pass", "message": "pass"},
        ],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )

    # Remove a file
    (skill / "delete_me.txt").unlink()

    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["status"] == "drifted"
    assert "delete_me.txt" in out.get("removed", [])


def test_check_tampered_manifest_hash(ws):
    """Tamper with latest.json without re-hashing → status=tampered, exit 1."""
    skill = make_skill(ws.skills_dir, "check-tamper", {"f.txt": "safe"})
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "tamper-pass.json",
        [
            {"rule": "ok", "level": "pass", "message": "pass"},
        ],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )

    # Tamper: modify a field in latest.json without re-hashing
    latest = skill / ".skill-meta" / "latest.json"
    data = json.loads(latest.read_text())
    data["scanStatus"] = "deny"  # tamper without re-hashing
    latest.write_text(json.dumps(data))

    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 1, f"expected exit 1 for tampered, got {r.returncode}"
    out = parse_json_output(r.stdout)
    assert out["status"] == "tampered", f"expected tampered, got {out}"


def test_check_deny_exit_code_1(ws):
    """Certify with deny findings → check returns deny with exit 1."""
    skill = make_skill(ws.skills_dir, "check-deny", {"danger.sh": "rm -rf /"})
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "deny.json",
        [
            {"rule": "dangerous-cmd", "level": "deny", "message": "rm -rf detected"},
        ],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )

    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 1, f"expected exit 1 for deny, got {r.returncode}"
    out = parse_json_output(r.stdout)
    assert out["status"] == "deny", f"expected deny, got {out}"


# ── Group 4: certify command ──────────────────────────────────────────────


def test_certify_external_findings_bare_array(ws):
    """--findings with bare JSON array → exit 0, correct scanStatus."""
    skill = make_skill(ws.skills_dir, "certify-bare", {"a.txt": "a"})
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "bare.json",
        [
            {"rule": "r1", "level": "pass", "message": "ok"},
            {"rule": "r2", "level": "warn", "message": "caution"},
        ],
    )
    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)],
        env_extra=env,
    )
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["scanStatus"] == "warn"  # warn dominates pass


def test_certify_external_findings_wrapped(ws):
    """--findings with {"findings": [...]} wrapper → exit 0."""
    skill = make_skill(ws.skills_dir, "certify-wrap", {"b.txt": "b"})
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "wrapped.json",
        {
            "findings": [
                {"rule": "r1", "level": "pass", "message": "ok"},
            ]
        },
    )
    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)],
        env_extra=env,
    )
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["scanStatus"] == "pass"


def test_certify_deny_finding_produces_deny(ws):
    """deny-level finding → scanStatus=deny."""
    skill = make_skill(ws.skills_dir, "certify-deny", {"c.txt": "c"})
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "deny-f.json",
        [
            {"rule": "r-pass", "level": "pass", "message": "ok"},
            {"rule": "r-deny", "level": "deny", "message": "blocked"},
        ],
    )
    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)],
        env_extra=env,
    )
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["scanStatus"] == "deny"  # deny dominates all


def test_certify_missing_findings_file(ws):
    """--findings pointing to nonexistent file → exit 1."""
    skill = make_skill(ws.skills_dir, "certify-missing", {"d.txt": "d"})
    env = ws.env()

    r = run_skill_ledger(
        ["certify", str(skill), "--findings", "/tmp/nonexistent_findings.json"],
        env_extra=env,
    )
    assert r.returncode == 1, f"expected exit 1, got {r.returncode}"


def test_certify_invalid_json_findings(ws):
    """--findings with invalid JSON → exit 1."""
    skill = make_skill(ws.skills_dir, "certify-badjson", {"e.txt": "e"})
    env = ws.env()

    bad_file = ws.fixtures / "bad.json"
    bad_file.write_text("{not valid json!!!")

    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(bad_file)],
        env_extra=env,
    )
    assert r.returncode == 1, f"expected exit 1 for invalid JSON, got {r.returncode}"


def test_certify_no_findings_auto_invoke(ws):
    """certify without --findings → auto-invoke mode, exit 0 (no-op in v1)."""
    skill = make_skill(ws.skills_dir, "certify-auto", {"f.txt": "f"})
    env = ws.env()

    r = run_skill_ledger(["certify", str(skill)], env_extra=env)
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    # Without findings, scanStatus stays at initial value
    assert "scanStatus" in out


def test_certify_no_skill_dir_no_all(ws):
    """certify without skill_dir and without --all → exit 1."""
    env = ws.env()
    r = run_skill_ledger(["certify"], env_extra=env)
    assert r.returncode == 1, f"expected exit 1, got {r.returncode}"
    combined = r.stdout + r.stderr
    assert (
        "required" in combined.lower() or "skill_dir" in combined.lower()
    ), f"Expected error about missing skill_dir: {combined}"


# ── Group 5: certify --all ────────────────────────────────────────────────


def test_certify_all_multiple_skills(ws):
    """--all certifies all skills from config.json skillDirs (auto-invoke mode)."""
    env = ws.env()

    # Create skills
    batch_root = ws.root / "batch_skills"
    batch_root.mkdir()
    for name in ("skill-x", "skill-y", "skill-z"):
        make_skill(batch_root, name, {"main.py": f"# {name}\n"})

    # Write config.json with skillDirs glob
    config_dir = ws.xdg_config / "agent-sec" / "skill-ledger"
    config_dir.mkdir(parents=True, exist_ok=True)
    config = {"skillDirs": [str(batch_root / "*")]}
    (config_dir / "config.json").write_text(json.dumps(config))

    # --all without --findings (auto-invoke mode)
    r = run_skill_ledger(
        ["certify", "--all"],
        env_extra=env,
    )
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert "results" in out, f"Expected 'results' key: {out}"
    assert len(out["results"]) == 3, f"Expected 3 results, got {len(out['results'])}"


def test_certify_all_no_skill_dirs(ws):
    """--all with empty skillDirs → exit 1."""
    env = ws.env()

    # Write config.json with empty skillDirs
    config_dir = ws.xdg_config / "agent-sec" / "skill-ledger"
    config_dir.mkdir(parents=True, exist_ok=True)
    config = {"skillDirs": []}
    (config_dir / "config.json").write_text(json.dumps(config))

    r = run_skill_ledger(["certify", "--all"], env_extra=env)
    assert r.returncode == 1, f"expected exit 1, got {r.returncode}"
    combined = r.stdout + r.stderr
    assert (
        "no skill directories" in combined.lower()
    ), f"Expected no-dirs message: {combined}"


# ── Group 6: audit command ────────────────────────────────────────────────


def test_audit_valid_chain(ws):
    """Multi-version audit → valid=true, exit 0."""
    skill = make_skill(ws.skills_dir, "audit-valid", {"a.txt": "a"})
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "audit-p.json",
        [
            {"rule": "ok", "level": "pass", "message": "pass"},
        ],
    )
    # Version 1
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    # Version 2
    (skill / "a.txt").write_text("a-v2")
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )

    r = run_skill_ledger(["audit", str(skill)], env_extra=env)
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["valid"] is True
    assert out["versions_checked"] >= 2


def test_audit_no_versions(ws):
    """Skill with no .skill-meta → valid=true, 0 versions checked."""
    skill = make_skill(ws.skills_dir, "audit-none", {"x.txt": "x"})
    env = ws.env()

    # Do NOT run check/certify — no manifest
    r = run_skill_ledger(["audit", str(skill)], env_extra=env)
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["valid"] is True
    assert out["versions_checked"] == 0


def test_audit_tampered_version_file(ws):
    """Tamper with a version JSON → valid=false, exit 1."""
    skill = make_skill(ws.skills_dir, "audit-tamper", {"f.txt": "f"})
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "audit-t.json",
        [
            {"rule": "ok", "level": "pass", "message": "pass"},
        ],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )

    # Tamper with the version file
    versions_dir = skill / ".skill-meta" / "versions"
    version_files = sorted(versions_dir.glob("v*.json"))
    assert (
        len(version_files) >= 1
    ), f"No version files found: {list(versions_dir.iterdir())}"
    vf = version_files[0]
    data = json.loads(vf.read_text())
    data["scanStatus"] = "deny"  # tamper without re-hashing
    vf.write_text(json.dumps(data))

    r = run_skill_ledger(["audit", str(skill)], env_extra=env)
    assert r.returncode == 1, f"expected exit 1 for tampered audit, got {r.returncode}"
    out = parse_json_output(r.stdout)
    assert out["valid"] is False
    assert len(out["errors"]) > 0


def test_audit_verify_snapshots(ws):
    """--verify-snapshots validates snapshot file hashes match manifest."""
    skill = make_skill(ws.skills_dir, "audit-snap", {"s.txt": "snapshot-test"})
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "audit-s.json",
        [
            {"rule": "ok", "level": "pass", "message": "pass"},
        ],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )

    r = run_skill_ledger(
        ["audit", str(skill), "--verify-snapshots"],
        env_extra=env,
    )
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["valid"] is True


# ── Group 7: status command ───────────────────────────────────────────────


def test_status_human_readable_output(ws):
    """status returns ledger-wide overview with keys, config, skills sections."""
    env = ws.env()

    batch_root = ws.root / "status_batch_skills"
    batch_root.mkdir()
    for name in ("sa-skill-1", "sa-skill-2"):
        make_skill(batch_root, name, {"run.sh": f"echo {name}\n"})

    config_dir = ws.xdg_config / "agent-sec" / "skill-ledger"
    config_dir.mkdir(parents=True, exist_ok=True)
    config = {"skillDirs": [str(batch_root / "*")]}
    (config_dir / "config.json").write_text(json.dumps(config))

    r = run_skill_ledger(["status"], env_extra=env)
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["command"] == "status"

    # keys section
    assert "keys" in out, f"Missing 'keys' section: {out}"
    assert out["keys"]["initialized"] is True

    # config section
    assert "config" in out, f"Missing 'config' section: {out}"
    assert out["config"]["customized"] is True

    # skills section with breakdown
    skills = out["skills"]
    assert skills["discovered"] == 2, f"Expected 2 discovered, got {skills}"
    assert skills["breakdown"]["none"] == 2
    assert skills["health"] == "unscanned"

    # no results by default (requires --verbose)
    assert "results" not in out, f"results should not appear without --verbose: {out}"


def test_status_drifted_shows_details(ws):
    """status health reflects drifted when a certified skill is modified."""
    env = ws.env()

    batch_root = ws.root / "status_drift_skills"
    batch_root.mkdir()
    skill = make_skill(
        batch_root,
        "drift-test",
        {"orig.txt": "original"},
    )

    config_dir = ws.xdg_config / "agent-sec" / "skill-ledger"
    config_dir.mkdir(parents=True, exist_ok=True)
    config = {"skillDirs": [str(batch_root / "*")]}
    (config_dir / "config.json").write_text(json.dumps(config))

    findings = write_findings_file(
        ws.fixtures,
        "status-d.json",
        [
            {"rule": "ok", "level": "pass", "message": "pass"},
        ],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )

    # Cause drift
    (skill / "orig.txt").write_text("MODIFIED")

    r = run_skill_ledger(["status"], env_extra=env)
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert (
        out["skills"]["health"] == "attention"
    ), f"Expected health 'attention' after drift: {out['skills']}"


# ── Group 8: stubs & edge cases ───────────────────────────────────────────


def test_set_policy_stub(ws):
    """set-policy → exit 0, 'coming soon' in output."""
    skill = make_skill(ws.skills_dir, "stub-policy", {"x.txt": "x"})
    r = run_skill_ledger(
        ["set-policy", str(skill), "--policy", "allow"],
        env_extra=ws.env(),
    )
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    assert "coming soon" in r.stdout.lower()


def test_rotate_keys_stub(ws):
    """rotate-keys → exit 0, 'coming soon' in output."""
    r = run_skill_ledger(["rotate-keys"], env_extra=ws.env())
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    assert "coming soon" in r.stdout.lower()


def test_list_scanners(ws):
    """list-scanners → exit 0, JSON with scanners array including skill-vetter."""
    r = run_skill_ledger(["list-scanners"], env_extra=ws.env())
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert "scanners" in out, f"Expected 'scanners' key in JSON output: {out}"
    names = [s["name"] for s in out["scanners"]]
    assert "skill-vetter" in names, f"Expected skill-vetter in scanners: {names}"


def test_certify_empty_skill_dir(ws):
    """Certify a skill dir with no SKILL.md → exit 1, status=error."""
    skill = ws.skills_dir / "empty-skill"
    skill.mkdir(parents=True, exist_ok=True)
    env = ws.env()

    r = run_skill_ledger(["certify", str(skill)], env_extra=env)
    assert r.returncode == 1, f"expected exit 1 for empty dir, got {r.returncode}"


# ── Group 9: SKILL.md contract assertions ────────────────────────────────
#
# These tests verify that the exact CLI commands, flags, output fields, and
# path conventions referenced in SKILL.md work as documented.  They form the
# contract between the Skill definition (prompt) and the CLI implementation.


def test_contract_help_available(ws):
    """Step 0.1: `agent-sec-cli skill-ledger --help` → exit 0."""
    r = run_skill_ledger(["--help"], env_extra=ws.env())
    assert r.returncode == 0, f"--help returned {r.returncode}: {r.stderr}"
    assert (
        "skill-ledger" in r.stdout.lower()
    ), f"Expected 'skill-ledger' in help output: {r.stdout[:200]}"


def test_contract_init_keys_empty_passphrase_env(ws):
    """Step 0.2: SKILL_LEDGER_PASSPHRASE=\"\" → passphrase-free init.

    This is the exact invocation SKILL.md uses for first-time auto-init.
    """
    alt_data = ws.root / "contract_keys"
    alt_data.mkdir()
    env = ws.env(
        {
            "XDG_DATA_HOME": str(alt_data),
            "SKILL_LEDGER_PASSPHRASE": "",  # empty string, NOT absent
        }
    )
    r = run_skill_ledger(["init-keys"], env_extra=env)
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert (
        out.get("encrypted") is False
    ), f"Empty passphrase should produce unencrypted keys, got {out}"

    # Step 0.2 also checks: ls ~/.local/share/agent-sec/skill-ledger/key.pub
    key_pub = Path(alt_data) / "agent-sec" / "skill-ledger" / "key.pub"
    assert key_pub.exists(), f"key.pub not at expected path: {key_pub}"


def test_contract_check_output_schema(ws):
    """Step 0.4: check output is JSON with `status` field for every outcome.

    SKILL.md parses `status` from JSON output to build the triage table.
    This test verifies the contract across all reachable statuses.
    """
    env = ws.env()

    # status: none (fresh skill)
    skill_none = make_skill(ws.skills_dir, "schema-none", {"a.txt": "a"})
    r = run_skill_ledger(["check", str(skill_none)], env_extra=env)
    out = parse_json_output(r.stdout)
    assert "status" in out, f"Missing 'status' field for none: {out}"
    assert out["status"] == "none"

    # status: pass (after certify)
    findings = write_findings_file(
        ws.fixtures,
        "schema-p.json",
        [{"rule": "ok", "level": "pass", "message": "pass"}],
    )
    run_skill_ledger(
        ["certify", str(skill_none), "--findings", str(findings)], env_extra=env
    )
    r = run_skill_ledger(["check", str(skill_none)], env_extra=env)
    out = parse_json_output(r.stdout)
    assert "status" in out, f"Missing 'status' field for pass: {out}"
    assert out["status"] == "pass"

    # status: drifted (file changed) — also verify diff fields
    (skill_none / "new.txt").write_text("new")
    r = run_skill_ledger(["check", str(skill_none)], env_extra=env)
    out = parse_json_output(r.stdout)
    assert "status" in out, f"Missing 'status' field for drifted: {out}"
    assert out["status"] == "drifted"
    for diff_key in ("added", "removed", "modified"):
        assert (
            diff_key in out
        ), f"drifted output missing '{diff_key}' — SKILL.md Step 0.4 needs this: {out}"


def test_contract_certify_explicit_scanner_flags(ws):
    """Phase 2.1: certify with explicit --scanner and --scanner-version flags.

    SKILL.md invocation:
      agent-sec-cli skill-ledger certify <DIR> \\
        --findings ... --scanner skill-vetter

    --scanner-version is optional (defaults to 'unknown' if omitted).
    This test verifies that explicit values are accepted.
    """
    skill = make_skill(ws.skills_dir, "contract-flags", {"run.sh": "echo hi"})
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "flags.json",
        [{"rule": "r1", "level": "pass", "message": "ok"}],
    )
    r = run_skill_ledger(
        [
            "certify",
            str(skill),
            "--findings",
            str(findings),
            "--scanner",
            "skill-vetter",
            "--scanner-version",
            "0.1.0",
        ],
        env_extra=env,
    )
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out.get("scanStatus") == "pass"


def test_contract_certify_output_fields(ws):
    """Phase 2.2: certify output JSON contains versionId and scanStatus.

    SKILL.md parses exactly these two fields to build the final summary table.
    """
    skill = make_skill(ws.skills_dir, "contract-output", {"data.py": "x = 1"})
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "out.json",
        [{"rule": "r1", "level": "warn", "message": "caution"}],
    )
    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)],
        env_extra=env,
    )
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)

    assert (
        "versionId" in out
    ), f"Missing 'versionId' — SKILL.md Phase 2.2 needs this: {out}"
    assert (
        "scanStatus" in out
    ), f"Missing 'scanStatus' — SKILL.md Phase 2.2 needs this: {out}"

    # versionId format: v + 6 digits (e.g. v000001)
    vid = out["versionId"]
    assert len(vid) == 7, f"versionId length should be 7 (vNNNNNN), got '{vid}'"
    assert vid[0] == "v", f"versionId should start with 'v', got '{vid}'"
    assert vid[1:].isdigit(), f"versionId suffix should be digits, got '{vid}'"

    # scanStatus must be one of the 4 documented values
    assert out["scanStatus"] in (
        "pass",
        "warn",
        "deny",
        "none",
    ), f"Unexpected scanStatus '{out['scanStatus']}' — SKILL.md documents pass/warn/deny/none"


def test_contract_manifest_path(ws):
    """Phase 2.3: after certify, manifest exists at <SKILL_DIR>/.skill-meta/latest.json."""
    skill = make_skill(ws.skills_dir, "contract-path", {"f.txt": "content"})
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "path.json",
        [{"rule": "r1", "level": "pass", "message": "ok"}],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)],
        env_extra=env,
    )

    latest = skill / ".skill-meta" / "latest.json"
    assert latest.exists(), (
        f"Manifest not at expected path — SKILL.md Phase 2.3 references "
        f"<SKILL_DIR>/.skill-meta/latest.json: {list(skill.rglob('*'))}"
    )

    # Verify it's valid JSON with expected fields
    data = json.loads(latest.read_text())
    assert "versionId" in data
    assert "fileHashes" in data
    assert "scanStatus" in data
    assert "signature" in data


def test_contract_check_status_values_complete(ws):
    """SKILL.md Step 0.4 triage table lists 6 statuses. Verify all are reachable.

    Statuses: none, pass, drifted, warn, deny, tampered.
    """
    env = ws.env()
    observed: set[str] = set()

    # none
    s = make_skill(ws.skills_dir, "sv-none", {"x.txt": "x"})
    r = run_skill_ledger(["check", str(s)], env_extra=env)
    observed.add(parse_json_output(r.stdout)["status"])

    # pass
    fp = write_findings_file(
        ws.fixtures,
        "sv-pass.json",
        [{"rule": "r", "level": "pass", "message": "ok"}],
    )
    run_skill_ledger(["certify", str(s), "--findings", str(fp)], env_extra=env)
    r = run_skill_ledger(["check", str(s)], env_extra=env)
    observed.add(parse_json_output(r.stdout)["status"])

    # drifted
    (s / "x.txt").write_text("changed")
    r = run_skill_ledger(["check", str(s)], env_extra=env)
    observed.add(parse_json_output(r.stdout)["status"])

    # warn
    sw = make_skill(ws.skills_dir, "sv-warn", {"w.txt": "w"})
    fpw = write_findings_file(
        ws.fixtures,
        "sv-warn.json",
        [{"rule": "r", "level": "warn", "message": "w"}],
    )
    run_skill_ledger(["certify", str(sw), "--findings", str(fpw)], env_extra=env)
    r = run_skill_ledger(["check", str(sw)], env_extra=env)
    observed.add(parse_json_output(r.stdout)["status"])

    # deny
    sd = make_skill(ws.skills_dir, "sv-deny", {"d.txt": "d"})
    fpd = write_findings_file(
        ws.fixtures,
        "sv-deny.json",
        [{"rule": "r", "level": "deny", "message": "d"}],
    )
    run_skill_ledger(["certify", str(sd), "--findings", str(fpd)], env_extra=env)
    r = run_skill_ledger(["check", str(sd)], env_extra=env)
    observed.add(parse_json_output(r.stdout)["status"])

    # tampered
    st = make_skill(ws.skills_dir, "sv-tamper", {"t.txt": "t"})
    fpt = write_findings_file(
        ws.fixtures,
        "sv-t.json",
        [{"rule": "r", "level": "pass", "message": "ok"}],
    )
    run_skill_ledger(["certify", str(st), "--findings", str(fpt)], env_extra=env)
    latest = st / ".skill-meta" / "latest.json"
    data = json.loads(latest.read_text())
    data["scanStatus"] = "deny"  # tamper without re-hashing
    latest.write_text(json.dumps(data))
    r = run_skill_ledger(["check", str(st)], env_extra=env)
    observed.add(parse_json_output(r.stdout)["status"])

    expected = {"none", "pass", "drifted", "warn", "deny", "tampered"}
    assert observed == expected, (
        f"Not all SKILL.md triage statuses are reachable.\n"
        f"  Expected: {expected}\n  Observed: {observed}\n"
        f"  Missing:  {expected - observed}"
    )


# ── Group 10: Key rotation ────────────────────────────────────────────────


def test_key_rotation_old_sigs_verifiable(ws):
    """After init-keys --force, old signatures must still pass `check`.

    The old public key should be archived into the keyring so that
    `verify()` can fall back to it for manifests signed with the
    previous key.
    """
    env = ws.env()

    # --- Sign a skill with the *original* key ---
    s = make_skill(ws.skills_dir, "rotate-test", {"a.txt": "a"})
    fp = write_findings_file(
        ws.fixtures,
        "rotate.json",
        [{"rule": "r", "level": "pass", "message": "ok"}],
    )
    r = run_skill_ledger(["certify", str(s), "--findings", str(fp)], env_extra=env)
    assert r.returncode == 0, f"certify failed: {r.stderr}"

    # Capture the old key fingerprint from the public key file
    pub_path = Path(env["XDG_DATA_HOME"]) / "agent-sec" / "skill-ledger" / "key.pub"
    old_fp = "sha256:" + hashlib.sha256(pub_path.read_bytes()).hexdigest()

    # check passes with original key
    r = run_skill_ledger(["check", str(s)], env_extra=env)
    out = parse_json_output(r.stdout)
    assert out["status"] == "pass", f"Expected pass before rotation, got {out}"

    # --- Rotate the key ---
    r = run_skill_ledger(["init-keys", "--force"], env_extra=env)
    assert r.returncode == 0, f"init-keys --force failed: {r.stderr}"
    new_fp = parse_json_output(r.stdout)["fingerprint"]
    assert new_fp != old_fp, (
        f"Key rotation must produce a different fingerprint: "
        f"old={old_fp}, new={new_fp}"
    )
    assert new_fp.startswith("sha256:"), f"Fingerprint format unexpected: {new_fp}"

    # --- Old manifest must still verify via keyring fallback ---
    r = run_skill_ledger(["check", str(s)], env_extra=env)
    out = parse_json_output(r.stdout)
    # The skill files haven't changed, so status should NOT be tampered.
    # It may be 'pass' (keyring verified) or 'drifted' if something else
    # changed, but it must NOT be 'tampered'.
    assert out["status"] != "tampered", (
        f"Old signature should still verify after key rotation, "
        f"but got status={out['status']}. Keyring archival may be broken."
    )
    # Specifically expect 'pass' since files are unchanged:
    assert out["status"] == "pass", (
        f"Expected 'pass' for unchanged skill after key rotation, "
        f"got '{out['status']}'"
    )



