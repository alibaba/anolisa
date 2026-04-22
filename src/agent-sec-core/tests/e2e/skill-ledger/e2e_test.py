#!/usr/bin/env python3
"""End-to-end tests for the ``skill-ledger`` CLI.

Exercises every subcommand through the real binary (``uv run agent-sec-cli skill-ledger``),
verifying **JSON stdout**, **exit codes**, and **filesystem side effects**.

All key material and config files are isolated via ``XDG_DATA_HOME`` and
``XDG_CONFIG_HOME`` environment variables so the host keyring is never touched.

Prerequisites: Python 3.11, uv
"""

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

# ── Paths ──────────────────────────────────────────────────────────────────

REPO_ROOT = Path(__file__).resolve().parents[3]  # agent-sec-core/
CLI_DIR = REPO_ROOT / "agent-sec-cli"

# ── Colours ────────────────────────────────────────────────────────────────

RED = "\033[0;31m"
GREEN = "\033[0;32m"
YELLOW = "\033[1;33m"
BLUE = "\033[0;34m"
BOLD = "\033[1m"
NC = "\033[0m"


# ── Result tracker ─────────────────────────────────────────────────────────


@dataclass
class Results:
    passed: int = 0
    failed: int = 0
    errors: list = field(default_factory=list)


results = Results()


# ── Helpers ────────────────────────────────────────────────────────────────


def run_skill_ledger(
    args: list[str],
    env_extra: dict | None = None,
    *,
    cwd: str | Path | None = None,
) -> subprocess.CompletedProcess:
    """Run ``uv run agent-sec-cli skill-ledger <args>`` with isolated XDG env."""
    env = os.environ.copy()
    if env_extra:
        env.update(env_extra)
    cmd = ["uv", "run", "agent-sec-cli", "skill-ledger"] + args
    return subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        env=env,
        cwd=cwd or str(CLI_DIR),
    )


def parse_json_output(stdout: str) -> dict:
    """Parse the first JSON line from CLI stdout."""
    for line in stdout.strip().splitlines():
        line = line.strip()
        if line.startswith("{") or line.startswith("["):
            return json.loads(line)
    raise ValueError(f"No JSON found in stdout:\n{stdout}")


def make_skill(parent: Path, name: str, files: dict[str, str]) -> Path:
    """Create a fake skill directory with the given files."""
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


def test(name: str, fn):
    """Run a single named test, catch exceptions, record results."""
    print(f"\n{BLUE}--- {name} ---{NC}")
    try:
        fn()
        print(f"{GREEN}✓ PASS{NC}")
        results.passed += 1
    except AssertionError as exc:
        print(f"{RED}✗ FAIL  {exc}{NC}")
        results.failed += 1
        results.errors.append((name, exc))
    except Exception as exc:
        print(f"{RED}✗ ERROR {exc}{NC}")
        results.failed += 1
        results.errors.append((name, exc))


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

        # Redirect all skill-ledger key/config I/O to temp dirs
        os.environ["XDG_DATA_HOME"] = str(self.xdg_data)
        os.environ["XDG_CONFIG_HOME"] = str(self.xdg_config)

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
        for key in ("XDG_DATA_HOME", "XDG_CONFIG_HOME"):
            os.environ.pop(key, None)
        shutil.rmtree(self.root, ignore_errors=True)


# ── Group 1: init-keys ─────────────────────────────────────────────────────


def test_init_keys_no_passphrase(ws: Workspace):
    """init-keys without passphrase → exit 0, encrypted: false."""
    r = run_skill_ledger(["init-keys"], env_extra=ws.env())
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out.get("encrypted") is False, f"expected encrypted=false, got {out}"
    assert out.get("fingerprint", "").startswith("sha256:"), f"bad fingerprint: {out}"


def test_init_keys_json_structure(ws: Workspace):
    """JSON output must contain all 4 expected fields."""
    # Keys already exist from test_init_keys_no_passphrase — re-gen with --force
    r = run_skill_ledger(["init-keys", "--force"], env_extra=ws.env())
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    for key in ("fingerprint", "publicKeyPath", "privateKeyPath", "encrypted"):
        assert key in out, f"Missing field '{key}' in output: {out}"
    assert len(out["fingerprint"]) > 10
    assert len(out["publicKeyPath"]) > 0
    assert len(out["privateKeyPath"]) > 0


def test_init_keys_reject_duplicate(ws: Workspace):
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


def test_init_keys_force_overwrite(ws: Workspace):
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


def test_init_keys_with_passphrase_env(ws: Workspace):
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


def test_full_lifecycle_pass(ws: Workspace):
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


def test_multi_version_lifecycle(ws: Workspace):
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


def test_lifecycle_with_warn_findings(ws: Workspace):
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


def test_check_no_manifest_auto_creates(ws: Workspace):
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


def test_check_after_file_add_drifted(ws: Workspace):
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


def test_check_after_file_modify_drifted(ws: Workspace):
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


def test_check_after_file_remove_drifted(ws: Workspace):
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


def test_check_tampered_manifest_hash(ws: Workspace):
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


def test_check_deny_exit_code_1(ws: Workspace):
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


def test_certify_external_findings_bare_array(ws: Workspace):
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


def test_certify_external_findings_wrapped(ws: Workspace):
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


def test_certify_deny_finding_produces_deny(ws: Workspace):
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


def test_certify_missing_findings_file(ws: Workspace):
    """--findings pointing to nonexistent file → exit 1."""
    skill = make_skill(ws.skills_dir, "certify-missing", {"d.txt": "d"})
    env = ws.env()

    r = run_skill_ledger(
        ["certify", str(skill), "--findings", "/tmp/nonexistent_findings.json"],
        env_extra=env,
    )
    assert r.returncode == 1, f"expected exit 1, got {r.returncode}"


def test_certify_invalid_json_findings(ws: Workspace):
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


def test_certify_no_findings_auto_invoke(ws: Workspace):
    """certify without --findings → auto-invoke mode, exit 0 (no-op in v1)."""
    skill = make_skill(ws.skills_dir, "certify-auto", {"f.txt": "f"})
    env = ws.env()

    r = run_skill_ledger(["certify", str(skill)], env_extra=env)
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    # Without findings, scanStatus stays at initial value
    assert "scanStatus" in out


def test_certify_no_skill_dir_no_all(ws: Workspace):
    """certify without skill_dir and without --all → exit 1."""
    env = ws.env()
    r = run_skill_ledger(["certify"], env_extra=env)
    assert r.returncode == 1, f"expected exit 1, got {r.returncode}"
    combined = r.stdout + r.stderr
    assert (
        "required" in combined.lower() or "skill_dir" in combined.lower()
    ), f"Expected error about missing skill_dir: {combined}"


# ── Group 5: certify --all ────────────────────────────────────────────────


def test_certify_all_multiple_skills(ws: Workspace):
    """--all certifies all skills from config.json skillDirs."""
    env = ws.env()

    # Create skills
    batch_root = ws.root / "batch_skills"
    batch_root.mkdir()
    for name in ("skill-x", "skill-y", "skill-z"):
        make_skill(batch_root, name, {"main.py": f"# {name}\n"})

    # Write config.json with skillDirs glob
    config_dir = ws.xdg_config / "skill-ledger"
    config_dir.mkdir(parents=True, exist_ok=True)
    config = {"skillDirs": [str(batch_root / "*")]}
    (config_dir / "config.json").write_text(json.dumps(config))

    findings = write_findings_file(
        ws.fixtures,
        "all-pass.json",
        [
            {"rule": "ok", "level": "pass", "message": "pass"},
        ],
    )
    r = run_skill_ledger(
        ["certify", "--all", "--findings", str(findings)],
        env_extra=env,
    )
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert "results" in out, f"Expected 'results' key: {out}"
    assert len(out["results"]) == 3, f"Expected 3 results, got {len(out['results'])}"


def test_certify_all_no_skill_dirs(ws: Workspace):
    """--all with empty skillDirs → exit 1."""
    env = ws.env()

    # Write config.json with empty skillDirs
    config_dir = ws.xdg_config / "skill-ledger"
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


def test_audit_valid_chain(ws: Workspace):
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


def test_audit_no_versions(ws: Workspace):
    """Skill with no .skill-meta → valid=true, 0 versions checked."""
    skill = make_skill(ws.skills_dir, "audit-none", {"x.txt": "x"})
    env = ws.env()

    # Do NOT run check/certify — no manifest
    r = run_skill_ledger(["audit", str(skill)], env_extra=env)
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["valid"] is True
    assert out["versions_checked"] == 0


def test_audit_tampered_version_file(ws: Workspace):
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


def test_audit_verify_snapshots(ws: Workspace):
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


def test_status_human_readable_output(ws: Workspace):
    """status shows human-readable info after certify."""
    skill = make_skill(ws.skills_dir, "status-show", {"m.txt": "main"})
    env = ws.env()

    findings = write_findings_file(
        ws.fixtures,
        "status-p.json",
        [
            {"rule": "ok", "level": "pass", "message": "pass"},
        ],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )

    r = run_skill_ledger(["status", str(skill)], env_extra=env)
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"

    output = r.stdout
    for label in ("Skill:", "Status:", "scanStatus:", "Version:", "Signed by:"):
        assert label in output, f"Missing '{label}' in status output:\n{output}"


def test_status_drifted_shows_details(ws: Workspace):
    """status after file modification shows added/removed/modified details."""
    skill = make_skill(
        ws.skills_dir,
        "status-drift",
        {
            "orig.txt": "original",
            "removeme.txt": "to be removed",
        },
    )
    env = ws.env()

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

    # Cause drift: add, remove, modify
    (skill / "new.txt").write_text("new")
    (skill / "removeme.txt").unlink()
    (skill / "orig.txt").write_text("MODIFIED")

    r = run_skill_ledger(["status", str(skill)], env_extra=env)
    assert r.returncode == 0
    output = r.stdout
    assert "drifted" in output.lower(), f"Expected 'drifted' in output:\n{output}"
    assert "Added:" in output, f"Expected 'Added:' in output:\n{output}"
    assert "Removed:" in output, f"Expected 'Removed:' in output:\n{output}"
    assert "Modified:" in output, f"Expected 'Modified:' in output:\n{output}"


# ── Group 8: stubs & edge cases ───────────────────────────────────────────


def test_set_policy_stub(ws: Workspace):
    """set-policy → exit 0, 'coming soon' in output."""
    skill = make_skill(ws.skills_dir, "stub-policy", {"x.txt": "x"})
    r = run_skill_ledger(
        ["set-policy", str(skill), "--policy", "allow"],
        env_extra=ws.env(),
    )
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    assert "coming soon" in r.stdout.lower()


def test_rotate_keys_stub(ws: Workspace):
    """rotate-keys → exit 0, 'coming soon' in output."""
    r = run_skill_ledger(["rotate-keys"], env_extra=ws.env())
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    assert "coming soon" in r.stdout.lower()


def test_list_scanners(ws: Workspace):
    """list-scanners → exit 0, prints header and at least skill-vetter."""
    r = run_skill_ledger(["list-scanners"], env_extra=ws.env())
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    assert "NAME" in r.stdout, f"Expected header row in output:\n{r.stdout}"
    assert "skill-vetter" in r.stdout, f"Expected skill-vetter in output:\n{r.stdout}"


def test_certify_empty_skill_dir(ws: Workspace):
    """Certify a skill dir with no files → still succeeds (empty fileHashes)."""
    skill = ws.skills_dir / "empty-skill"
    skill.mkdir(parents=True, exist_ok=True)
    env = ws.env()

    r = run_skill_ledger(["certify", str(skill)], env_extra=env)
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert "scanStatus" in out


# ── Group 9: SKILL.md contract assertions ────────────────────────────────
#
# These tests verify that the exact CLI commands, flags, output fields, and
# path conventions referenced in SKILL.md work as documented.  They form the
# contract between the Skill definition (prompt) and the CLI implementation.


def test_contract_help_available(ws: Workspace):
    """Step 0.1: `agent-sec-cli skill-ledger --help` → exit 0."""
    r = run_skill_ledger(["--help"], env_extra=ws.env())
    assert r.returncode == 0, f"--help returned {r.returncode}: {r.stderr}"
    assert (
        "skill-ledger" in r.stdout.lower()
    ), f"Expected 'skill-ledger' in help output: {r.stdout[:200]}"


def test_contract_init_keys_empty_passphrase_env(ws: Workspace):
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

    # Step 0.2 also checks: ls ~/.local/share/skill-ledger/key.pub
    key_pub = Path(alt_data) / "skill-ledger" / "key.pub"
    assert key_pub.exists(), f"key.pub not at expected path: {key_pub}"


def test_contract_check_output_schema(ws: Workspace):
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


def test_contract_certify_explicit_scanner_flags(ws: Workspace):
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


def test_contract_certify_output_fields(ws: Workspace):
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


def test_contract_manifest_path(ws: Workspace):
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


def test_contract_check_status_values_complete(ws: Workspace):
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


def test_key_rotation_old_sigs_verifiable(ws: Workspace):
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
    pub_path = Path(env["XDG_DATA_HOME"]) / "skill-ledger" / "key.pub"
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


# ── Main ───────────────────────────────────────────────────────────────────


def main():
    # Pre-flight
    uv = shutil.which("uv")
    if not uv:
        print(f"{RED}ERROR: uv not found — cannot run e2e tests{NC}")
        sys.exit(1)
    if not CLI_DIR.exists():
        print(f"{RED}ERROR: {CLI_DIR} not found{NC}")
        sys.exit(1)

    ws = Workspace()
    try:
        print("=" * 60)
        print(f"{BOLD}skill-ledger CLI E2E Tests{NC}")
        print(f"  CLI dir   : {CLI_DIR}")
        print(f"  workspace : {ws.root}")
        print("=" * 60)

        # Group 1: init-keys (run first — all subsequent tests need keys)
        test("init-keys: no passphrase", lambda: test_init_keys_no_passphrase(ws))
        test("init-keys: JSON structure", lambda: test_init_keys_json_structure(ws))
        test("init-keys: reject duplicate", lambda: test_init_keys_reject_duplicate(ws))
        test("init-keys: --force overwrite", lambda: test_init_keys_force_overwrite(ws))
        test(
            "init-keys: passphrase env var",
            lambda: test_init_keys_with_passphrase_env(ws),
        )

        # Group 2: happy path lifecycles
        test("Lifecycle: full pass flow", lambda: test_full_lifecycle_pass(ws))
        test("Lifecycle: multi-version chain", lambda: test_multi_version_lifecycle(ws))
        test("Lifecycle: warn findings", lambda: test_lifecycle_with_warn_findings(ws))

        # Group 3: check state machine
        test(
            "Check: no manifest → auto-create",
            lambda: test_check_no_manifest_auto_creates(ws),
        )
        test(
            "Check: file added → drifted", lambda: test_check_after_file_add_drifted(ws)
        )
        test(
            "Check: file modified → drifted",
            lambda: test_check_after_file_modify_drifted(ws),
        )
        test(
            "Check: file removed → drifted",
            lambda: test_check_after_file_remove_drifted(ws),
        )
        test(
            "Check: tampered manifest → exit 1",
            lambda: test_check_tampered_manifest_hash(ws),
        )
        test("Check: deny status → exit 1", lambda: test_check_deny_exit_code_1(ws))

        # Group 4: certify command
        test(
            "Certify: bare array findings",
            lambda: test_certify_external_findings_bare_array(ws),
        )
        test(
            "Certify: wrapped findings",
            lambda: test_certify_external_findings_wrapped(ws),
        )
        test(
            "Certify: deny finding", lambda: test_certify_deny_finding_produces_deny(ws)
        )
        test(
            "Certify: missing findings file",
            lambda: test_certify_missing_findings_file(ws),
        )
        test("Certify: invalid JSON", lambda: test_certify_invalid_json_findings(ws))
        test(
            "Certify: auto-invoke mode",
            lambda: test_certify_no_findings_auto_invoke(ws),
        )
        test(
            "Certify: no skill_dir no --all",
            lambda: test_certify_no_skill_dir_no_all(ws),
        )

        # Group 5: certify --all
        test(
            "Certify --all: multiple skills",
            lambda: test_certify_all_multiple_skills(ws),
        )
        test("Certify --all: no skill dirs", lambda: test_certify_all_no_skill_dirs(ws))

        # Group 6: audit
        test("Audit: valid chain", lambda: test_audit_valid_chain(ws))
        test("Audit: no versions", lambda: test_audit_no_versions(ws))
        test(
            "Audit: tampered version file", lambda: test_audit_tampered_version_file(ws)
        )
        test("Audit: --verify-snapshots", lambda: test_audit_verify_snapshots(ws))

        # Group 7: status
        test(
            "Status: human-readable output",
            lambda: test_status_human_readable_output(ws),
        )
        test("Status: drifted details", lambda: test_status_drifted_shows_details(ws))

        # Group 8: stubs & edge cases
        test("set-policy stub", lambda: test_set_policy_stub(ws))
        test("rotate-keys stub", lambda: test_rotate_keys_stub(ws))
        test("list-scanners", lambda: test_list_scanners(ws))
        test("Certify: empty skill dir", lambda: test_certify_empty_skill_dir(ws))

        # Group 9: SKILL.md contract assertions
        test(
            "Contract: --help available",
            lambda: test_contract_help_available(ws),
        )
        test(
            "Contract: empty passphrase env → unencrypted",
            lambda: test_contract_init_keys_empty_passphrase_env(ws),
        )
        test(
            "Contract: check output always has status",
            lambda: test_contract_check_output_schema(ws),
        )
        test(
            "Contract: certify --scanner --scanner-version flags",
            lambda: test_contract_certify_explicit_scanner_flags(ws),
        )
        test(
            "Contract: certify output has versionId + scanStatus",
            lambda: test_contract_certify_output_fields(ws),
        )
        test(
            "Contract: manifest at .skill-meta/latest.json",
            lambda: test_contract_manifest_path(ws),
        )
        test(
            "Contract: all 6 triage statuses reachable",
            lambda: test_contract_check_status_values_complete(ws),
        )

        # Group 10: Key rotation
        test(
            "Key rotation: old signatures still verifiable",
            lambda: test_key_rotation_old_sigs_verifiable(ws),
        )

    finally:
        ws.cleanup()

    # Summary
    print()
    print("=" * 60)
    total = results.passed + results.failed
    print(f"{BOLD}Results: {results.passed}/{total} passed{NC}")
    if results.errors:
        for name, exc in results.errors:
            print(f"  {RED}FAIL{NC} {name}: {exc}")
    print("=" * 60)

    if results.failed:
        print(f"{RED}{results.failed} test(s) failed{NC}")
        sys.exit(1)
    else:
        print(f"{GREEN}All tests passed!{NC}")
        sys.exit(0)


if __name__ == "__main__":
    main()
