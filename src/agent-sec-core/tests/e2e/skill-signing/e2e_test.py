#!/usr/bin/env python3
"""End-to-end tests for skill signing (sign-skill.sh) and verification (verifier.py).

Exercises the full pipeline:
  1. sign-skill.sh --init   → GPG key generation + public key export
  2. sign-skill.sh <dir>    → single skill signing
  3. sign-skill.sh --batch  → batch skill signing
  4. verifier.py             → signature + hash verification

All GPG operations use an isolated GNUPGHOME so the host keyring is never
touched.

Prerequisites: gpg, jq, python3
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

# ── Paths ──────────────────────────────────────────────────────────────────

REPO_ROOT = Path(__file__).resolve().parents[3]  # agent-sec-core/
SIGN_SKILL_SH = REPO_ROOT / "tools" / "sign-skill.sh"
VERIFIER_DIR = REPO_ROOT / "skill" / "scripts" / "asset-verify"
VERIFIER_PY = VERIFIER_DIR / "verifier.py"

SIGNING_DIR = ".skill-meta"

# Make verifier importable
sys.path.insert(0, str(VERIFIER_DIR))

from errors import ErrHashMismatch, ErrSigInvalid, ErrSigMissing  # noqa: E402
from verifier import load_trusted_keys, verify_skill  # noqa: E402

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


def run_sign_skill(
    args: list[str],
    env_extra: Optional[dict] = None,
) -> subprocess.CompletedProcess:
    """Run sign-skill.sh with the given arguments in the isolated env."""
    env = os.environ.copy()
    if env_extra:
        env.update(env_extra)
    cmd = ["bash", str(SIGN_SKILL_SH)] + args
    return subprocess.run(cmd, capture_output=True, text=True, env=env)


def make_skill(parent: Path, name: str, files: dict[str, str]) -> Path:
    """Create a fake skill directory with the given files.

    ``files`` maps relative path → content.
    Returns the skill directory path.
    """
    skill_dir = parent / name
    for rel, content in files.items():
        p = skill_dir / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content)
    return skill_dir


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


# We reuse a single temp workspace across all tests so the GPG key only
# needs to be generated once.


class Workspace:
    """Shared test workspace: isolated GNUPGHOME, trusted-keys dir, etc."""

    def __init__(self):
        self.root = Path(tempfile.mkdtemp(prefix="e2e_sign_"))
        self.gnupg_home = self.root / "gnupg"
        self.gnupg_home.mkdir(mode=0o700)
        self.trusted_keys = self.root / "trusted-keys"
        self.trusted_keys.mkdir()
        self.skills_dir = self.root / "skills"
        self.skills_dir.mkdir()

        # Propagate isolated GNUPGHOME to all child processes
        os.environ["GNUPGHOME"] = str(self.gnupg_home)

    def cleanup(self):
        if "GNUPGHOME" in os.environ:
            del os.environ["GNUPGHOME"]
        shutil.rmtree(self.root, ignore_errors=True)


# ── Test cases ─────────────────────────────────────────────────────────────


def test_check(ws: Workspace):
    """--check should report all prerequisites OK."""
    r = run_sign_skill(["--check"])
    assert r.returncode == 0, f"exit {r.returncode}: {r.stderr}"
    combined = r.stdout + r.stderr
    assert "All prerequisites satisfied" in combined, combined


def test_init(ws: Workspace):
    """--init generates a GPG key and exports the public key."""
    r = run_sign_skill(
        [
            "--init",
            "--trusted-keys-dir",
            str(ws.trusted_keys),
        ]
    )
    assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}\n{r.stderr}"

    # Public key file must exist
    asc_files = list(ws.trusted_keys.glob("*.asc"))
    assert len(asc_files) >= 1, (
        f"No .asc in {ws.trusted_keys}: {list(ws.trusted_keys.iterdir())}"
    )
    assert asc_files[0].stat().st_size > 0, "Exported .asc is empty"


def test_single_sign_and_verify(ws: Workspace):
    """Sign a single skill, then verify with the verifier module."""
    skill = make_skill(
        ws.skills_dir,
        "skill-a",
        {
            "main.py": "print('hello')\n",
            "README.md": "# Skill A\n",
        },
    )

    r = run_sign_skill([str(skill), "--force"])
    assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}\n{r.stderr}"

    # Manifest and signature must exist inside .skill-meta/
    signing = skill / SIGNING_DIR
    assert (signing / "Manifest.json").exists(), ".skill-meta/Manifest.json missing"
    assert (signing / ".skill.sig").exists(), ".skill-meta/.skill.sig missing"

    # Manifest must contain our files
    manifest = json.loads((signing / "Manifest.json").read_text())
    paths_in_manifest = {f["path"] for f in manifest["files"]}
    assert "main.py" in paths_in_manifest, (
        f"main.py not in manifest: {paths_in_manifest}"
    )
    assert "README.md" in paths_in_manifest, (
        f"README.md not in manifest: {paths_in_manifest}"
    )
    # .skill-meta/ contents should NOT be in manifest
    assert "Manifest.json" not in paths_in_manifest
    assert ".skill.sig" not in paths_in_manifest
    signing_paths = [p for p in paths_in_manifest if p.startswith(".skill-meta")]
    assert not signing_paths, f".skill-meta paths should be excluded: {signing_paths}"

    # Verify with verifier
    keys = load_trusted_keys(ws.trusted_keys)
    ok, name = verify_skill(str(skill), keys)
    assert ok, "verify_skill returned False"
    assert name == "skill-a"


def test_batch_sign_and_verify(ws: Workspace):
    """Batch-sign multiple skills, then verify each."""
    batch_root = ws.root / "batch_skills"
    batch_root.mkdir()
    for sname, content in [("alpha", "A"), ("beta", "B"), ("gamma", "C")]:
        make_skill(batch_root, sname, {"data.txt": content})

    r = run_sign_skill(["--batch", str(batch_root), "--force"])
    assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}\n{r.stderr}"
    assert "3/3" in r.stdout, f"Expected 3/3 in output: {r.stdout}"

    keys = load_trusted_keys(ws.trusted_keys)
    for sname in ("alpha", "beta", "gamma"):
        ok, name = verify_skill(str(batch_root / sname), keys)
        assert ok, f"verify_skill failed for {sname}"
        assert name == sname


def test_force_overwrite(ws: Workspace):
    """--force overwrites existing manifest and signature."""
    skill = make_skill(ws.skills_dir, "skill-force", {"f.txt": "v1"})

    r1 = run_sign_skill([str(skill), "--force"])
    assert r1.returncode == 0
    sig1 = (skill / SIGNING_DIR / ".skill.sig").read_text()

    # Change content and re-sign
    (skill / "f.txt").write_text("v2")
    r2 = run_sign_skill([str(skill), "--force"])
    assert r2.returncode == 0
    sig2 = (skill / SIGNING_DIR / ".skill.sig").read_text()

    assert sig1 != sig2, "Signature should differ after content change"

    # Verify new signature
    keys = load_trusted_keys(ws.trusted_keys)
    ok, _ = verify_skill(str(skill), keys)
    assert ok


def test_no_force_rejects(ws: Workspace):
    """Without --force, existing manifest/sig blocks signing."""
    skill = make_skill(ws.skills_dir, "skill-noforce", {"x.txt": "x"})

    r1 = run_sign_skill([str(skill)])
    assert r1.returncode == 0

    # Second run without --force should fail
    r2 = run_sign_skill([str(skill)])
    assert r2.returncode != 0, "Expected non-zero exit without --force"
    assert "already exists" in r2.stdout + r2.stderr


def test_export_key_default_and_custom(ws: Workspace):
    """--export-key exports to a specified directory."""
    custom_dir = ws.root / "custom_keys"
    r = run_sign_skill(["--export-key", str(custom_dir)])
    assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}\n{r.stderr}"
    asc_files = list(custom_dir.glob("*.asc"))
    assert len(asc_files) >= 1, f"No .asc in {custom_dir}"


def test_skill_name_override(ws: Workspace):
    """--skill-name overrides the skill name in the manifest."""
    skill = make_skill(ws.skills_dir, "skill-rename", {"a.txt": "a"})
    r = run_sign_skill([str(skill), "--skill-name", "custom-name", "--force"])
    assert r.returncode == 0

    manifest = json.loads((skill / SIGNING_DIR / "Manifest.json").read_text())
    assert manifest["skill_name"] == "custom-name", (
        f"Expected 'custom-name', got '{manifest['skill_name']}'"
    )


def test_hidden_files_excluded(ws: Workspace):
    """Hidden files and directories are excluded from the manifest."""
    skill = make_skill(
        ws.skills_dir,
        "skill-hidden",
        {
            "visible.txt": "ok",
            ".hidden_file": "secret",
            ".hidden_dir/inner.txt": "secret2",
        },
    )
    r = run_sign_skill([str(skill), "--force"])
    assert r.returncode == 0

    manifest = json.loads((skill / SIGNING_DIR / "Manifest.json").read_text())
    paths = {f["path"] for f in manifest["files"]}
    assert "visible.txt" in paths
    assert ".hidden_file" not in paths, f".hidden_file should be excluded: {paths}"
    assert ".hidden_dir/inner.txt" not in paths, (
        f".hidden_dir should be excluded: {paths}"
    )
    # .skill-meta dir itself should not appear
    meta_paths = [p for p in paths if p.startswith(".skill-meta")]
    assert not meta_paths, f".skill-meta paths should be excluded: {meta_paths}"


def test_tampered_file_detected(ws: Workspace):
    """Verifier detects file content tampering after signing."""
    skill = make_skill(ws.skills_dir, "skill-tamper", {"payload.txt": "original"})
    r = run_sign_skill([str(skill), "--force"])
    assert r.returncode == 0

    # Tamper with the file
    (skill / "payload.txt").write_text("TAMPERED")

    keys = load_trusted_keys(ws.trusted_keys)
    try:
        verify_skill(str(skill), keys)
        assert False, "Expected ErrHashMismatch"
    except ErrHashMismatch:
        pass  # expected


def test_missing_sig_detected(ws: Workspace):
    """Verifier raises ErrSigMissing when .skill.sig is deleted."""
    skill = make_skill(ws.skills_dir, "skill-nosig", {"f.txt": "f"})
    r = run_sign_skill([str(skill), "--force"])
    assert r.returncode == 0

    (skill / SIGNING_DIR / ".skill.sig").unlink()

    keys = load_trusted_keys(ws.trusted_keys)
    try:
        verify_skill(str(skill), keys)
        assert False, "Expected ErrSigMissing"
    except ErrSigMissing:
        pass


def test_wrong_key_rejected(ws: Workspace):
    """Signature made with key A is rejected when verified with key B only."""
    # Generate a completely separate key pair in a different GNUPGHOME
    alt_dir = ws.root / "alt_gpg"
    alt_dir.mkdir(mode=0o700)
    alt_keys = ws.root / "alt_keys"
    alt_keys.mkdir()

    # Generate alt key
    subprocess.run(
        ["gpg", "--homedir", str(alt_dir), "--batch", "--gen-key"],
        input=(
            "Key-Type: RSA\nKey-Length: 2048\nName-Real: Alt Key\n"
            "Name-Email: alt@test.local\nExpire-Date: 0\n%no-protection\n%commit\n"
        ).encode(),
        capture_output=True,
    )
    alt_pub = alt_keys / "alt.asc"
    with open(alt_pub, "w") as f:
        subprocess.run(
            ["gpg", "--homedir", str(alt_dir), "--armor", "--export", "alt@test.local"],
            stdout=f,
        )
    assert alt_pub.stat().st_size > 0, "Failed to export alt public key"

    # Skill was signed with the INIT key (ws GNUPGHOME), but verify with ALT
    # key only → should fail
    skill = make_skill(ws.skills_dir, "skill-wrongkey", {"z.txt": "z"})
    r = run_sign_skill([str(skill), "--force"])
    assert r.returncode == 0

    alt_trusted = load_trusted_keys(alt_keys)
    try:
        verify_skill(str(skill), alt_trusted)
        assert False, "Expected ErrSigInvalid"
    except ErrSigInvalid:
        pass


def test_gpg_private_key_env(ws: Workspace):
    """GPG_PRIVATE_KEY env var import + signing works end-to-end."""
    # Create a fresh GNUPGHOME with a new key
    env_dir = ws.root / "env_gpg"
    env_dir.mkdir(mode=0o700)
    subprocess.run(
        ["gpg", "--homedir", str(env_dir), "--batch", "--gen-key"],
        input=(
            "Key-Type: RSA\nKey-Length: 2048\nName-Real: Env Key\n"
            "Name-Email: env@test.local\nExpire-Date: 0\n%no-protection\n%commit\n"
        ).encode(),
        capture_output=True,
    )

    # Export private key
    priv = subprocess.run(
        [
            "gpg",
            "--homedir",
            str(env_dir),
            "--armor",
            "--export-secret-keys",
            "env@test.local",
        ],
        capture_output=True,
        text=True,
    )
    assert priv.returncode == 0 and len(priv.stdout) > 100, "Private key export failed"

    # Export public key for verification
    env_keys = ws.root / "env_keys"
    env_keys.mkdir()
    pub_path = env_keys / "env.asc"
    with open(pub_path, "w") as f:
        subprocess.run(
            ["gpg", "--homedir", str(env_dir), "--armor", "--export", "env@test.local"],
            stdout=f,
        )

    # Use a blank GNUPGHOME so the only way sign-skill.sh can sign is via import
    blank_home = ws.root / "blank_gpg"
    blank_home.mkdir(mode=0o700)

    skill = make_skill(ws.skills_dir, "skill-envkey", {"e.txt": "env"})
    r = run_sign_skill(
        [str(skill), "--force"],
        env_extra={
            "GNUPGHOME": str(blank_home),
            "GPG_PRIVATE_KEY": priv.stdout,
        },
    )
    assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}\n{r.stderr}"
    assert "imported and trusted" in r.stdout + r.stderr, (
        f"Expected import message: {r.stdout}\n{r.stderr}"
    )

    # Verify
    env_trusted = load_trusted_keys(env_keys)
    ok, _ = verify_skill(str(skill), env_trusted)
    assert ok


def test_manifest_structure(ws: Workspace):
    """Manifest JSON has the expected schema fields."""
    skill = make_skill(
        ws.skills_dir,
        "skill-schema",
        {
            "script.sh": "#!/bin/bash\necho hi\n",
        },
    )
    r = run_sign_skill([str(skill), "--force"])
    assert r.returncode == 0

    manifest = json.loads((skill / SIGNING_DIR / "Manifest.json").read_text())
    for key in ("version", "skill_name", "algorithm", "created_at", "files"):
        assert key in manifest, f"Missing field '{key}' in manifest"
    assert manifest["version"] == "0.1"
    assert manifest["algorithm"] == "SHA256"
    assert manifest["skill_name"] == "skill-schema"
    assert len(manifest["files"]) == 1
    assert manifest["files"][0]["path"] == "script.sh"
    assert len(manifest["files"][0]["hash"]) == 64  # SHA256 hex


def test_subdirectory_files(ws: Workspace):
    """Files in nested subdirectories are included in the manifest."""
    skill = make_skill(
        ws.skills_dir,
        "skill-nested",
        {
            "top.txt": "top",
            "sub/deep.txt": "deep",
            "sub/deeper/leaf.txt": "leaf",
        },
    )
    r = run_sign_skill([str(skill), "--force"])
    assert r.returncode == 0

    manifest = json.loads((skill / SIGNING_DIR / "Manifest.json").read_text())
    paths = {f["path"] for f in manifest["files"]}
    assert paths == {"top.txt", "sub/deep.txt", "sub/deeper/leaf.txt"}, paths

    keys = load_trusted_keys(ws.trusted_keys)
    ok, _ = verify_skill(str(skill), keys)
    assert ok


# ── Main ───────────────────────────────────────────────────────────────────


def main():
    # Pre-flight
    if not shutil.which("gpg"):
        print(f"{RED}ERROR: gpg not found – cannot run e2e tests{NC}")
        sys.exit(1)
    if not shutil.which("jq"):
        print(f"{RED}ERROR: jq not found – cannot run e2e tests{NC}")
        sys.exit(1)
    if not SIGN_SKILL_SH.exists():
        print(f"{RED}ERROR: {SIGN_SKILL_SH} not found{NC}")
        sys.exit(1)

    ws = Workspace()
    try:
        print("=" * 60)
        print(f"{BOLD}Skill Signing E2E Tests{NC}")
        print(f"  sign-skill.sh : {SIGN_SKILL_SH}")
        print(f"  verifier.py   : {VERIFIER_PY}")
        print(f"  workspace     : {ws.root}")
        print("=" * 60)

        # Run --init first; most subsequent tests depend on the generated key
        test("Prerequisites check (--check)", lambda: test_check(ws))
        test("Init: generate key + export (--init)", lambda: test_init(ws))

        # Signing & verification
        test("Single sign + verify", lambda: test_single_sign_and_verify(ws))
        test("Batch sign + verify", lambda: test_batch_sign_and_verify(ws))
        test("Force overwrite re-sign", lambda: test_force_overwrite(ws))
        test("No --force rejects existing", lambda: test_no_force_rejects(ws))
        test("Export key to custom dir", lambda: test_export_key_default_and_custom(ws))
        test("Skill name override", lambda: test_skill_name_override(ws))
        test("Hidden files excluded", lambda: test_hidden_files_excluded(ws))

        # Negative / security tests
        test("Tampered file detected", lambda: test_tampered_file_detected(ws))
        test("Missing .skill.sig detected", lambda: test_missing_sig_detected(ws))
        test("Wrong key rejected", lambda: test_wrong_key_rejected(ws))

        # Environment variable key import
        test("GPG_PRIVATE_KEY env import", lambda: test_gpg_private_key_env(ws))

        # Schema / structure
        test("Manifest JSON structure", lambda: test_manifest_structure(ws))
        test("Subdirectory files in manifest", lambda: test_subdirectory_files(ws))

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
