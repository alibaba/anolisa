#!/usr/bin/env python3
"""End-to-end tests for the ``skill-ledger`` CLI on RPM-installed environments.

Tests exercise the installed ``agent-sec-cli`` binary directly (no ``uv run``,
no ``python -m``).  Designed to run after RPM installation to validate the
full CLI pipeline, cosh hook integration, and passphrase-protected key flows.

Test groups:
   G1  Pre-flight & help
   G2  init-keys
   G3  Happy-path lifecycle (check → certify → check → audit)
   G4  check state machine
   G5  certify command
   G6  certify --all
   G7  audit
   G8  status (human-readable)
   G9  stubs & edge cases
   G10 SKILL.md contract assertions
   G11 Passphrase-protected key lifecycle
   G12 cosh hook integration
   G13 Full vetter → ledger → hook pipeline
   G14 Key rotation

Usage::

    python3 e2e_test.py            # normal run
    python3 e2e_test.py --verbose  # show CLI stdout/stderr
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

# ── Colours ────────────────────────────────────────────────────────────────

RED = "\033[0;31m"
GREEN = "\033[0;32m"
YELLOW = "\033[1;33m"
BLUE = "\033[0;34m"
BOLD = "\033[1m"
NC = "\033[0m"

# ── Globals ────────────────────────────────────────────────────────────────

CLI_BIN = shutil.which("agent-sec-cli")
VERBOSE = False


# ── Result tracker ─────────────────────────────────────────────────────────


@dataclass
class Results:
    passed: int = 0
    failed: int = 0
    errors: list = field(default_factory=list)


results = Results()


# ── Constants ─────────────────────────────────────────────────────────────

CLI_TIMEOUT_S = 60  # seconds — safety net against unexpected hangs


# ── Helpers ────────────────────────────────────────────────────────────────


def run_skill_ledger(
    args: list[str],
    env_extra: dict | None = None,
    *,
    cwd: str | Path | None = None,
) -> subprocess.CompletedProcess:
    """Run ``agent-sec-cli skill-ledger <args>`` with isolated XDG env."""
    env = os.environ.copy()
    if env_extra:
        env.update(env_extra)
    cmd = [CLI_BIN, "skill-ledger"] + args
    result = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        env=env,
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        timeout=CLI_TIMEOUT_S,
    )
    if VERBOSE:
        print(f"  cmd: {' '.join(cmd[:5])} ... {' '.join(args)}")
        if result.stdout.strip():
            print(f"  stdout: {result.stdout.strip()[:200]}")
        if result.stderr.strip():
            print(f"  stderr: {result.stderr.strip()[:200]}")
        print(f"  exit: {result.returncode}")
    return result


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
        self.root = Path(tempfile.mkdtemp(prefix="e2e_rpm_skill_ledger_"))
        self.xdg_data = self.root / "xdg_data"
        self.xdg_config = self.root / "xdg_config"
        self.xdg_data.mkdir()
        self.xdg_config.mkdir()
        self.skills_dir = self.root / "skills"
        self.skills_dir.mkdir()
        self.fixtures = self.root / "fixtures"
        self.fixtures.mkdir()
        # Hook-visible skills: the cosh hook resolves via {cwd}/.copilot-shell/skills/
        self.hook_skills_dir = self.root / ".copilot-shell" / "skills"
        self.hook_skills_dir.mkdir(parents=True)

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


# ── G1: Pre-flight & help ─────────────────────────────────────────────────


def test_help_available(ws: Workspace):
    """``agent-sec-cli skill-ledger --help`` → exit 0."""
    r = run_skill_ledger(["--help"], env_extra=ws.env())
    assert r.returncode == 0, f"--help returned {r.returncode}: {r.stderr}"
    assert (
        "skill-ledger" in r.stdout.lower()
    ), f"Expected 'skill-ledger' in help output: {r.stdout[:200]}"


# ── G2: init-keys ─────────────────────────────────────────────────────────


def test_init_keys_no_passphrase(ws: Workspace):
    """init-keys without passphrase → exit 0, encrypted: false."""
    r = run_skill_ledger(["init-keys"], env_extra=ws.env())
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out.get("encrypted") is False, f"expected encrypted=false, got {out}"
    assert out.get("fingerprint", "").startswith("sha256:"), f"bad fingerprint: {out}"


def test_init_keys_json_structure(ws: Workspace):
    """JSON output must contain all 4 expected fields."""
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


# ── G3: Happy-path lifecycle ──────────────────────────────────────────────


def test_full_lifecycle_pass(ws: Workspace):
    """init-keys → check (none) → certify (pass) → check (pass) → audit (valid)."""
    skill = make_skill(
        ws.skills_dir,
        "lifecycle-pass",
        {"main.py": "print('hello')\n", "README.md": "# Test\n"},
    )
    env = ws.env()

    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 0, f"check exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["status"] == "none", f"expected none, got {out}"

    findings = write_findings_file(
        ws.fixtures,
        "pass.json",
        [{"rule": "no-sudo", "level": "pass", "message": "No sudo found"}],
    )
    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    assert r.returncode == 0, f"certify exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["scanStatus"] == "pass", f"expected pass, got {out}"

    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 0, f"check exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["status"] == "pass", f"expected pass, got {out}"

    r = run_skill_ledger(["audit", str(skill)], env_extra=env)
    assert r.returncode == 0, f"audit exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["valid"] is True, f"expected valid=true, got {out}"


def test_multi_version_lifecycle(ws: Workspace):
    """certify → modify file → certify → audit validates 2-version chain."""
    skill = make_skill(ws.skills_dir, "multi-ver", {"data.txt": "v1"})
    env = ws.env()
    findings = write_findings_file(
        ws.fixtures,
        "mv-pass.json",
        [{"rule": "safe", "level": "pass", "message": "OK"}],
    )

    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    assert r.returncode == 0, f"certify1 exit {r.returncode}: {r.stderr}"
    out1 = parse_json_output(r.stdout)
    assert out1["newVersion"] is True

    (skill / "data.txt").write_text("v2")
    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    assert r.returncode == 0, f"certify2 exit {r.returncode}: {r.stderr}"
    out2 = parse_json_output(r.stdout)
    assert out2["newVersion"] is True
    assert out2["versionId"] != out1["versionId"], "Expected different versionId"

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
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    assert r.returncode == 0, f"certify exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["scanStatus"] == "warn", f"expected warn, got {out}"

    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 0, f"check should exit 0 for warn: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["status"] == "warn"


# ── G4: check state machine ──────────────────────────────────────────────


def test_check_no_manifest_auto_creates(ws: Workspace):
    """First check on new skill → auto-create manifest, status=none."""
    skill = make_skill(ws.skills_dir, "check-new", {"f.txt": "hello"})
    env = ws.env()
    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["status"] == "none"
    latest = skill / ".skill-meta" / "latest.json"
    assert latest.exists(), f"latest.json not created: {list(skill.rglob('*'))}"


def test_check_after_file_add_drifted(ws: Workspace):
    """Adding a file after certify → status=drifted."""
    skill = make_skill(ws.skills_dir, "check-add", {"original.txt": "content"})
    env = ws.env()
    findings = write_findings_file(
        ws.fixtures,
        "add-pass.json",
        [{"rule": "ok", "level": "pass", "message": "pass"}],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
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
        [{"rule": "ok", "level": "pass", "message": "pass"}],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
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
        {"keep.txt": "keep", "delete_me.txt": "gone"},
    )
    env = ws.env()
    findings = write_findings_file(
        ws.fixtures,
        "rm-pass.json",
        [{"rule": "ok", "level": "pass", "message": "pass"}],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
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
        [{"rule": "ok", "level": "pass", "message": "pass"}],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    latest = skill / ".skill-meta" / "latest.json"
    data = json.loads(latest.read_text())
    data["scanStatus"] = "deny"
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
        [{"rule": "dangerous-cmd", "level": "deny", "message": "rm -rf detected"}],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 1, f"expected exit 1 for deny, got {r.returncode}"
    out = parse_json_output(r.stdout)
    assert out["status"] == "deny", f"expected deny, got {out}"


# ── G5: certify command ──────────────────────────────────────────────────


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
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["scanStatus"] == "warn"


def test_certify_external_findings_wrapped(ws: Workspace):
    """--findings with {"findings": [...]} wrapper → exit 0."""
    skill = make_skill(ws.skills_dir, "certify-wrap", {"b.txt": "b"})
    env = ws.env()
    findings = write_findings_file(
        ws.fixtures,
        "wrapped.json",
        {"findings": [{"rule": "r1", "level": "pass", "message": "ok"}]},
    )
    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
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
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["scanStatus"] == "deny"


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
        ["certify", str(skill), "--findings", str(bad_file)], env_extra=env
    )
    assert r.returncode == 1, f"expected exit 1 for invalid JSON, got {r.returncode}"


def test_certify_no_findings_auto_invoke(ws: Workspace):
    """certify without --findings → auto-invoke mode, exit 0."""
    skill = make_skill(ws.skills_dir, "certify-auto", {"f.txt": "f"})
    env = ws.env()
    r = run_skill_ledger(["certify", str(skill)], env_extra=env)
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
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


# ── G6: certify --all ────────────────────────────────────────────────────


def test_certify_all_multiple_skills(ws: Workspace):
    """--all certifies all skills from config.json skillDirs (auto-invoke mode)."""
    env = ws.env()
    batch_root = ws.root / "batch_skills"
    batch_root.mkdir()
    for name in ("skill-x", "skill-y", "skill-z"):
        make_skill(batch_root, name, {"main.py": f"# {name}\n"})

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


def test_certify_all_no_skill_dirs(ws: Workspace):
    """--all with empty skillDirs → exit 1."""
    env = ws.env()
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


# ── G7: audit command ────────────────────────────────────────────────────


def test_audit_valid_chain(ws: Workspace):
    """Multi-version audit → valid=true, exit 0."""
    skill = make_skill(ws.skills_dir, "audit-valid", {"a.txt": "a"})
    env = ws.env()
    findings = write_findings_file(
        ws.fixtures,
        "audit-p.json",
        [{"rule": "ok", "level": "pass", "message": "pass"}],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
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
        [{"rule": "ok", "level": "pass", "message": "pass"}],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    versions_dir = skill / ".skill-meta" / "versions"
    version_files = sorted(versions_dir.glob("v*.json"))
    assert len(version_files) >= 1, f"No version files: {list(versions_dir.iterdir())}"
    vf = version_files[0]
    data = json.loads(vf.read_text())
    data["scanStatus"] = "deny"
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
        [{"rule": "ok", "level": "pass", "message": "pass"}],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    r = run_skill_ledger(["audit", str(skill), "--verify-snapshots"], env_extra=env)
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["valid"] is True


# ── G8: status command ───────────────────────────────────────────────────


def test_status_human_readable_output(ws: Workspace):
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


def test_status_drifted_shows_details(ws: Workspace):
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
        [{"rule": "ok", "level": "pass", "message": "pass"}],
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


# ── G9: stubs & edge cases ───────────────────────────────────────────────


def test_set_policy_stub(ws: Workspace):
    """set-policy → exit 0, 'coming soon' in output."""
    skill = make_skill(ws.skills_dir, "stub-policy", {"x.txt": "x"})
    r = run_skill_ledger(
        ["set-policy", str(skill), "--policy", "allow"], env_extra=ws.env()
    )
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    assert "coming soon" in r.stdout.lower()


def test_rotate_keys_stub(ws: Workspace):
    """rotate-keys → exit 0, 'coming soon' in output."""
    r = run_skill_ledger(["rotate-keys"], env_extra=ws.env())
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    assert "coming soon" in r.stdout.lower()


def test_list_scanners(ws: Workspace):
    """list-scanners → exit 0, JSON with scanners array including skill-vetter."""
    r = run_skill_ledger(["list-scanners"], env_extra=ws.env())
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert "scanners" in out, f"Expected 'scanners' key in JSON output: {out}"
    names = [s["name"] for s in out["scanners"]]
    assert "skill-vetter" in names, f"Expected skill-vetter in scanners: {names}"


def test_certify_empty_skill_dir(ws: Workspace):
    """Certify a skill dir with no SKILL.md → exit 1."""
    skill = ws.skills_dir / "empty-skill"
    skill.mkdir(parents=True, exist_ok=True)
    env = ws.env()
    r = run_skill_ledger(["certify", str(skill)], env_extra=env)
    assert r.returncode == 1, f"expected exit 1 for empty dir, got {r.returncode}"


# ── G10: SKILL.md contract assertions ────────────────────────────────────


def test_contract_init_keys_empty_passphrase_env(ws: Workspace):
    """SKILL_LEDGER_PASSPHRASE="" → passphrase-free init."""
    alt_data = ws.root / "contract_keys"
    alt_data.mkdir()
    env = ws.env({"XDG_DATA_HOME": str(alt_data), "SKILL_LEDGER_PASSPHRASE": ""})
    r = run_skill_ledger(["init-keys"], env_extra=env)
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert (
        out.get("encrypted") is False
    ), f"Empty passphrase should produce unencrypted keys, got {out}"
    key_pub = Path(alt_data) / "agent-sec" / "skill-ledger" / "key.pub"
    assert key_pub.exists(), f"key.pub not at expected path: {key_pub}"


def test_contract_check_output_schema(ws: Workspace):
    """check output is JSON with ``status`` field for every outcome."""
    env = ws.env()

    skill_none = make_skill(ws.skills_dir, "schema-none", {"a.txt": "a"})
    r = run_skill_ledger(["check", str(skill_none)], env_extra=env)
    out = parse_json_output(r.stdout)
    assert "status" in out, f"Missing 'status' field for none: {out}"
    assert out["status"] == "none"

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
    assert "status" in out and out["status"] == "pass"

    (skill_none / "new.txt").write_text("new")
    r = run_skill_ledger(["check", str(skill_none)], env_extra=env)
    out = parse_json_output(r.stdout)
    assert out["status"] == "drifted"
    for diff_key in ("added", "removed", "modified"):
        assert diff_key in out, f"drifted output missing '{diff_key}': {out}"


def test_contract_certify_explicit_scanner_flags(ws: Workspace):
    """certify with explicit --scanner and --scanner-version flags."""
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
    """certify output JSON contains versionId and scanStatus."""
    skill = make_skill(ws.skills_dir, "contract-output", {"data.py": "x = 1"})
    env = ws.env()
    findings = write_findings_file(
        ws.fixtures,
        "out.json",
        [{"rule": "r1", "level": "warn", "message": "caution"}],
    )
    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert "versionId" in out, f"Missing 'versionId': {out}"
    assert "scanStatus" in out, f"Missing 'scanStatus': {out}"
    vid = out["versionId"]
    assert (
        len(vid) == 7 and vid[0] == "v" and vid[1:].isdigit()
    ), f"Bad versionId: {vid}"
    assert out["scanStatus"] in (
        "pass",
        "warn",
        "deny",
        "none",
    ), f"Unexpected scanStatus '{out['scanStatus']}'"


def test_contract_manifest_path(ws: Workspace):
    """After certify, manifest exists at <SKILL_DIR>/.skill-meta/latest.json."""
    skill = make_skill(ws.skills_dir, "contract-path", {"f.txt": "content"})
    env = ws.env()
    findings = write_findings_file(
        ws.fixtures,
        "path.json",
        [{"rule": "r1", "level": "pass", "message": "ok"}],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    latest = skill / ".skill-meta" / "latest.json"
    assert latest.exists(), f"Manifest not at expected path: {list(skill.rglob('*'))}"
    data = json.loads(latest.read_text())
    for field in ("versionId", "fileHashes", "scanStatus", "signature"):
        assert field in data, f"Missing '{field}' in manifest"


def test_contract_check_status_values_complete(ws: Workspace):
    """All 6 triage statuses are reachable: none, pass, drifted, warn, deny, tampered."""
    env = ws.env()
    observed: set[str] = set()

    s = make_skill(ws.skills_dir, "sv-none", {"x.txt": "x"})
    r = run_skill_ledger(["check", str(s)], env_extra=env)
    observed.add(parse_json_output(r.stdout)["status"])

    fp = write_findings_file(
        ws.fixtures,
        "sv-pass.json",
        [{"rule": "r", "level": "pass", "message": "ok"}],
    )
    run_skill_ledger(["certify", str(s), "--findings", str(fp)], env_extra=env)
    r = run_skill_ledger(["check", str(s)], env_extra=env)
    observed.add(parse_json_output(r.stdout)["status"])

    (s / "x.txt").write_text("changed")
    r = run_skill_ledger(["check", str(s)], env_extra=env)
    observed.add(parse_json_output(r.stdout)["status"])

    sw = make_skill(ws.skills_dir, "sv-warn", {"w.txt": "w"})
    fpw = write_findings_file(
        ws.fixtures,
        "sv-warn.json",
        [{"rule": "r", "level": "warn", "message": "w"}],
    )
    run_skill_ledger(["certify", str(sw), "--findings", str(fpw)], env_extra=env)
    r = run_skill_ledger(["check", str(sw)], env_extra=env)
    observed.add(parse_json_output(r.stdout)["status"])

    sd = make_skill(ws.skills_dir, "sv-deny", {"d.txt": "d"})
    fpd = write_findings_file(
        ws.fixtures,
        "sv-deny.json",
        [{"rule": "r", "level": "deny", "message": "d"}],
    )
    run_skill_ledger(["certify", str(sd), "--findings", str(fpd)], env_extra=env)
    r = run_skill_ledger(["check", str(sd)], env_extra=env)
    observed.add(parse_json_output(r.stdout)["status"])

    st = make_skill(ws.skills_dir, "sv-tamper", {"t.txt": "t"})
    fpt = write_findings_file(
        ws.fixtures,
        "sv-t.json",
        [{"rule": "r", "level": "pass", "message": "ok"}],
    )
    run_skill_ledger(["certify", str(st), "--findings", str(fpt)], env_extra=env)
    latest = st / ".skill-meta" / "latest.json"
    data = json.loads(latest.read_text())
    data["scanStatus"] = "deny"
    latest.write_text(json.dumps(data))
    r = run_skill_ledger(["check", str(st)], env_extra=env)
    observed.add(parse_json_output(r.stdout)["status"])

    expected = {"none", "pass", "drifted", "warn", "deny", "tampered"}
    assert observed == expected, (
        f"Not all triage statuses reachable.\n"
        f"  Expected: {expected}\n  Observed: {observed}\n"
        f"  Missing:  {expected - observed}"
    )


# ── G11: Passphrase-protected key lifecycle ──────────────────────────────


def test_passphrase_full_lifecycle(ws: Workspace):
    """Encrypted key: init → check → certify → check → audit — all work."""
    pp_data = ws.root / "pp_data"
    pp_data.mkdir()
    env = ws.env(
        {"XDG_DATA_HOME": str(pp_data), "SKILL_LEDGER_PASSPHRASE": "s3cret-test"}
    )

    r = run_skill_ledger(["init-keys", "--passphrase"], env_extra=env)
    assert r.returncode == 0, f"init-keys exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["encrypted"] is True

    skill = make_skill(ws.skills_dir, "pp-life", {"app.py": "pass\n"})

    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 0, f"check exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["status"] == "none"

    findings = write_findings_file(
        ws.fixtures,
        "pp.json",
        [{"rule": "ok", "level": "pass", "message": "ok"}],
    )
    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    assert r.returncode == 0, f"certify exit {r.returncode}: {r.stderr}"
    out = parse_json_output(r.stdout)
    assert out["scanStatus"] == "pass"

    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["status"] == "pass"

    r = run_skill_ledger(["audit", str(skill)], env_extra=env)
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["valid"] is True


def test_passphrase_missing_env_fails(ws: Workspace):
    """Encrypted key without SKILL_LEDGER_PASSPHRASE → certify fails gracefully."""
    pp_data = ws.root / "pp_noenv"
    pp_data.mkdir()
    env_with = ws.env(
        {"XDG_DATA_HOME": str(pp_data), "SKILL_LEDGER_PASSPHRASE": "my-pass"}
    )
    r = run_skill_ledger(["init-keys", "--passphrase"], env_extra=env_with)
    assert r.returncode == 0

    skill = make_skill(ws.skills_dir, "pp-noenv", {"f.txt": "data"})
    findings = write_findings_file(
        ws.fixtures,
        "pp-noenv.json",
        [{"rule": "ok", "level": "pass", "message": "ok"}],
    )

    # Remove passphrase from env — should fail.
    # start_new_session=True detaches the child from the controlling terminal,
    # so getpass.getpass() cannot open /dev/tty and falls back to stdin.
    # Piping "\n" gives it an empty (wrong) passphrase → decryption fails → exit != 0.
    env_without = ws.env({"XDG_DATA_HOME": str(pp_data)})
    env_without.pop("SKILL_LEDGER_PASSPHRASE", None)
    cmd = [CLI_BIN, "skill-ledger", "certify", str(skill), "--findings", str(findings)]
    r = subprocess.run(
        cmd,
        input="\n",
        capture_output=True,
        text=True,
        env=env_without,
        start_new_session=True,
        timeout=CLI_TIMEOUT_S,
    )
    if VERBOSE:
        print(f"  cmd: {' '.join(cmd[:5])} ... certify (no passphrase)")
        if r.stderr.strip():
            print(f"  stderr: {r.stderr.strip()[:200]}")
        print(f"  exit: {r.returncode}")
    assert (
        r.returncode != 0
    ), f"Expected failure without passphrase, got exit {r.returncode}"


# ── G12: cosh hook integration ───────────────────────────────────────────

# The cosh hook script location varies; try common paths.
_HOOK_SEARCH_PATHS = [
    # Relative to RPM install
    Path("/usr/share/anolisa/extensions/agent-sec-core/hooks/skill_ledger_hook.py"),
    # Relative to source tree (if running during development)
    Path(__file__).resolve().parents[3]
    / "cosh-extension"
    / "hooks"
    / "skill_ledger_hook.py",
]
HOOK_SCRIPT: str | None = None
for _p in _HOOK_SEARCH_PATHS:
    if _p.is_file():
        HOOK_SCRIPT = str(_p)
        break


def _run_hook(input_data, env_extra=None):
    """Pipe cosh event JSON into the hook script, return parsed output."""
    env = os.environ.copy()
    if env_extra:
        env.update(env_extra)
    proc = subprocess.run(
        [sys.executable, HOOK_SCRIPT],
        input=(
            json.dumps(input_data) if isinstance(input_data, dict) else str(input_data)
        ),
        capture_output=True,
        text=True,
        timeout=CLI_TIMEOUT_S,
        env=env,
    )
    if VERBOSE:
        print(f"  hook stdout: {proc.stdout.strip()[:200]}")
        if proc.stderr.strip():
            print(f"  hook stderr: {proc.stderr.strip()[:200]}")
    return json.loads(proc.stdout)


def _make_cosh_event(skill_name: str, cwd: str) -> dict:
    """Build a minimal cosh PreToolUse JSON event."""
    return {
        "session_id": "test-session",
        "hook_event_name": "PreToolUse",
        "tool_name": "skill",
        "tool_input": {"skill": skill_name},
        "cwd": cwd,
    }


def test_hook_invalid_json_allows():
    """Malformed stdin → fail-open allow."""
    output = _run_hook("not-json")
    assert output == {"decision": "allow"}


def test_hook_wrong_tool_allows():
    """Non-skill tool → allow."""
    output = _run_hook(
        {
            "tool_name": "run_shell_command",
            "tool_input": {"command": "echo hi"},
        }
    )
    assert output == {"decision": "allow"}


def test_hook_unknown_skill_warns():
    """Skill not found on disk → allow with warning."""
    output = _run_hook(_make_cosh_event("nonexistent-skill-xyz", "/tmp"))
    assert output["decision"] == "allow"
    assert "not found" in output.get("reason", "").lower()


def test_hook_pass_status_silent(ws: Workspace):
    """Hook on a pass-status skill → silent allow (no reason)."""
    skill = make_skill(ws.hook_skills_dir, "hook-pass", {"m.txt": "main"})
    env = ws.env()
    findings = write_findings_file(
        ws.fixtures,
        "hook-p.json",
        [{"rule": "ok", "level": "pass", "message": "ok"}],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    output = _run_hook(
        _make_cosh_event("hook-pass", str(ws.root)),
        env_extra=env,
    )
    assert output["decision"] == "allow"
    assert "reason" not in output, f"Expected silent allow, got reason: {output}"


def test_hook_drifted_warns(ws: Workspace):
    """Hook on a drifted skill → allow with warning."""
    skill = make_skill(ws.hook_skills_dir, "hook-drift", {"f.txt": "original"})
    env = ws.env()
    findings = write_findings_file(
        ws.fixtures,
        "hook-d.json",
        [{"rule": "ok", "level": "pass", "message": "ok"}],
    )
    run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    (skill / "f.txt").write_text("MODIFIED")
    output = _run_hook(
        _make_cosh_event("hook-drift", str(ws.root)),
        env_extra=env,
    )
    assert output["decision"] == "allow"
    assert "reason" in output, f"Expected warning reason for drifted: {output}"
    assert (
        "drifted" in output["reason"].lower() or "changed" in output["reason"].lower()
    )


def test_hook_path_traversal_rejected(ws: Workspace):
    """Path traversal in skill name → rejected with reason."""
    output = _run_hook(
        _make_cosh_event("../../etc/passwd", "/tmp"),
        env_extra=ws.env(),
    )
    assert output["decision"] == "allow"
    assert "reason" in output
    assert "traversal" in output["reason"].lower()


# ── G13: Full pipeline (vetter → ledger → hook) ─────────────────────────


def test_full_pipeline_vetter_to_hook(ws: Workspace):
    """End-to-end: create → check(none) → certify(pass) → hook(silent allow)."""
    skill = make_skill(ws.hook_skills_dir, "pipeline-full", {"app.py": "print(1)\n"})
    env = ws.env()

    # check → none
    r = run_skill_ledger(["check", str(skill)], env_extra=env)
    out = parse_json_output(r.stdout)
    assert out["status"] == "none"

    # certify → pass
    findings = write_findings_file(
        ws.fixtures,
        "pipe.json",
        [{"rule": "no-exec", "level": "pass", "message": "ok"}],
    )
    r = run_skill_ledger(
        ["certify", str(skill), "--findings", str(findings)], env_extra=env
    )
    assert r.returncode == 0
    out = parse_json_output(r.stdout)
    assert out["scanStatus"] == "pass"

    # hook → silent allow
    if HOOK_SCRIPT:
        output = _run_hook(
            _make_cosh_event("pipeline-full", str(ws.root)),
            env_extra=env,
        )
        assert output == {"decision": "allow"}, f"Expected silent allow: {output}"

    # Modify file → drifted
    (skill / "app.py").write_text("print(2)\n")

    # hook → allow with warning
    if HOOK_SCRIPT:
        output = _run_hook(
            _make_cosh_event("pipeline-full", str(ws.root)),
            env_extra=env,
        )
        assert output["decision"] == "allow"
        assert "reason" in output, f"Expected drift warning: {output}"


# ── G14: Key rotation ────────────────────────────────────────────────────


def test_key_rotation_old_sigs_verifiable(ws: Workspace):
    """After init-keys --force, old signatures must still pass ``check``."""
    env = ws.env()

    s = make_skill(ws.skills_dir, "rotate-test", {"a.txt": "a"})
    fp = write_findings_file(
        ws.fixtures,
        "rotate.json",
        [{"rule": "r", "level": "pass", "message": "ok"}],
    )
    r = run_skill_ledger(["certify", str(s), "--findings", str(fp)], env_extra=env)
    assert r.returncode == 0, f"certify failed: {r.stderr}"

    pub_path = Path(env["XDG_DATA_HOME"]) / "agent-sec" / "skill-ledger" / "key.pub"
    old_fp = "sha256:" + hashlib.sha256(pub_path.read_bytes()).hexdigest()

    r = run_skill_ledger(["check", str(s)], env_extra=env)
    out = parse_json_output(r.stdout)
    assert out["status"] == "pass", f"Expected pass before rotation, got {out}"

    r = run_skill_ledger(["init-keys", "--force"], env_extra=env)
    assert r.returncode == 0, f"init-keys --force failed: {r.stderr}"
    new_fp = parse_json_output(r.stdout)["fingerprint"]
    assert (
        new_fp != old_fp
    ), f"Key rotation must produce a different fingerprint: old={old_fp}, new={new_fp}"
    assert new_fp.startswith("sha256:")

    r = run_skill_ledger(["check", str(s)], env_extra=env)
    out = parse_json_output(r.stdout)
    assert (
        out["status"] != "tampered"
    ), f"Old signature should still verify after key rotation, got {out['status']}"
    assert (
        out["status"] == "pass"
    ), f"Expected 'pass' for unchanged skill after key rotation, got '{out['status']}'"


# ── Main ───────────────────────────────────────────────────────────────────


def main():
    # Pre-flight
    if not CLI_BIN:
        print(f"{RED}ERROR: agent-sec-cli not found on PATH{NC}")
        print(
            "Install the RPM package or ensure the binary is on PATH.\n"
            "  rpm -q agent-sec-core  # check installation"
        )
        sys.exit(1)

    hook_available = HOOK_SCRIPT is not None

    ws = Workspace()
    try:
        print("=" * 60)
        print(f"{BOLD}skill-ledger CLI E2E Tests (RPM binary){NC}")
        print(f"  CLI binary : {CLI_BIN}")
        print(f"  Hook script: {HOOK_SCRIPT or 'NOT FOUND (hook tests skipped)'}")
        print(f"  workspace  : {ws.root}")
        print("=" * 60)

        # G1: Pre-flight & help
        test("G1: --help available", lambda: test_help_available(ws))

        # G2: init-keys (run first — all subsequent tests need keys)
        test("G2: init-keys no passphrase", lambda: test_init_keys_no_passphrase(ws))
        test("G2: init-keys JSON structure", lambda: test_init_keys_json_structure(ws))
        test(
            "G2: init-keys reject duplicate",
            lambda: test_init_keys_reject_duplicate(ws),
        )
        test(
            "G2: init-keys --force overwrite",
            lambda: test_init_keys_force_overwrite(ws),
        )
        test(
            "G2: init-keys passphrase env",
            lambda: test_init_keys_with_passphrase_env(ws),
        )

        # G3: Happy-path lifecycle
        test("G3: full pass lifecycle", lambda: test_full_lifecycle_pass(ws))
        test("G3: multi-version chain", lambda: test_multi_version_lifecycle(ws))
        test(
            "G3: warn findings lifecycle", lambda: test_lifecycle_with_warn_findings(ws)
        )

        # G4: check state machine
        test(
            "G4: no manifest → auto-create",
            lambda: test_check_no_manifest_auto_creates(ws),
        )
        test("G4: file added → drifted", lambda: test_check_after_file_add_drifted(ws))
        test(
            "G4: file modified → drifted",
            lambda: test_check_after_file_modify_drifted(ws),
        )
        test(
            "G4: file removed → drifted",
            lambda: test_check_after_file_remove_drifted(ws),
        )
        test("G4: tampered → exit 1", lambda: test_check_tampered_manifest_hash(ws))
        test("G4: deny → exit 1", lambda: test_check_deny_exit_code_1(ws))

        # G5: certify command
        test(
            "G5: bare array findings",
            lambda: test_certify_external_findings_bare_array(ws),
        )
        test("G5: wrapped findings", lambda: test_certify_external_findings_wrapped(ws))
        test("G5: deny finding", lambda: test_certify_deny_finding_produces_deny(ws))
        test(
            "G5: missing findings file", lambda: test_certify_missing_findings_file(ws)
        )
        test("G5: invalid JSON", lambda: test_certify_invalid_json_findings(ws))
        test("G5: auto-invoke mode", lambda: test_certify_no_findings_auto_invoke(ws))
        test("G5: no skill_dir no --all", lambda: test_certify_no_skill_dir_no_all(ws))

        # G6: certify --all
        test("G6: --all multiple skills", lambda: test_certify_all_multiple_skills(ws))
        test("G6: --all no skill dirs", lambda: test_certify_all_no_skill_dirs(ws))

        # G7: audit
        test("G7: valid chain", lambda: test_audit_valid_chain(ws))
        test("G7: no versions", lambda: test_audit_no_versions(ws))
        test("G7: tampered version file", lambda: test_audit_tampered_version_file(ws))
        test("G7: --verify-snapshots", lambda: test_audit_verify_snapshots(ws))

        # G8: status
        test("G8: human-readable output", lambda: test_status_human_readable_output(ws))
        test("G8: drifted details", lambda: test_status_drifted_shows_details(ws))

        # G9: stubs & edge cases
        test("G9: set-policy stub", lambda: test_set_policy_stub(ws))
        test("G9: rotate-keys stub", lambda: test_rotate_keys_stub(ws))
        test("G9: list-scanners", lambda: test_list_scanners(ws))
        test("G9: certify empty skill dir", lambda: test_certify_empty_skill_dir(ws))

        # G10: contract assertions
        test(
            "G10: empty passphrase env",
            lambda: test_contract_init_keys_empty_passphrase_env(ws),
        )
        test("G10: check output schema", lambda: test_contract_check_output_schema(ws))
        test(
            "G10: certify --scanner flags",
            lambda: test_contract_certify_explicit_scanner_flags(ws),
        )
        test(
            "G10: certify output fields",
            lambda: test_contract_certify_output_fields(ws),
        )
        test("G10: manifest path", lambda: test_contract_manifest_path(ws))
        test(
            "G10: all 6 statuses reachable",
            lambda: test_contract_check_status_values_complete(ws),
        )

        # G11: passphrase-protected lifecycle
        test(
            "G11: passphrase full lifecycle", lambda: test_passphrase_full_lifecycle(ws)
        )
        test(
            "G11: missing passphrase fails",
            lambda: test_passphrase_missing_env_fails(ws),
        )

        # G12: cosh hook integration
        if hook_available:
            test(
                "G12: hook invalid JSON → allow",
                lambda: test_hook_invalid_json_allows(),
            )
            test("G12: hook wrong tool → allow", lambda: test_hook_wrong_tool_allows())
            test(
                "G12: hook unknown skill warns", lambda: test_hook_unknown_skill_warns()
            )
            test(
                "G12: hook pass → silent allow",
                lambda: test_hook_pass_status_silent(ws),
            )
            test("G12: hook drifted → warning", lambda: test_hook_drifted_warns(ws))
            test(
                "G12: hook path traversal",
                lambda: test_hook_path_traversal_rejected(ws),
            )
        else:
            print(f"\n{YELLOW}SKIP G12: cosh hook script not found{NC}")

        # G13: full pipeline
        test(
            "G13: vetter→ledger→hook pipeline",
            lambda: test_full_pipeline_vetter_to_hook(ws),
        )

        # G14: key rotation
        test(
            "G14: old sigs verifiable after rotation",
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
    import argparse

    parser = argparse.ArgumentParser(description="skill-ledger CLI E2E tests (RPM)")
    parser.add_argument("-v", "--verbose", action="store_true", help="Show CLI output")
    args = parser.parse_args()
    VERBOSE = args.verbose
    main()
