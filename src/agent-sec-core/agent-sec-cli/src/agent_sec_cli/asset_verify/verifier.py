#!/usr/bin/env python3
"""Skill integrity verifier - Manifest + PGP signature verification."""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

try:
    import pgpy
except ImportError:
    pgpy = None

from agent_sec_cli.asset_verify.errors import (
    ErrConfigMissing,
    ErrHashMismatch,
    ErrManifestMissing,
    ErrNoTrustedKeys,
    ErrSigInvalid,
    ErrSigMissing,
)

SCRIPT_DIR = Path(__file__).parent.resolve()
DEFAULT_CONFIG = SCRIPT_DIR / "config.conf"
DEFAULT_TRUSTED_KEYS_DIR = SCRIPT_DIR / "trusted-keys"

# Check if system gpg is available (prefer 'gpg', fall back to 'gpg2' on RHEL/Alinux)
GPG_BIN = shutil.which("gpg") or shutil.which("gpg2")

# Hidden directory inside each skill that holds signing artifacts
SIGNING_DIR = ".skill-meta"


def load_config(config_path: Path) -> dict[str, list[str] | str]:
    """Load verification config file"""
    if not config_path.exists():
        raise ErrConfigMissing(str(config_path))

    config = {"skills_dirs": []}
    in_list = False

    with open(config_path, "r") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"):
                continue

            if in_list:
                if line == "]":
                    in_list = False
                else:
                    config["skills_dirs"].append(line.rstrip(","))
            elif "=" in line:
                key, val = line.split("=", 1)
                key, val = key.strip(), val.strip()
                if key == "skills_dir":
                    if val == "[":
                        in_list = True
                    else:
                        config["skills_dirs"].append(val)
                elif key == "trusted_keys_dir":
                    config["trusted_keys_dir"] = val
    return config


def load_trusted_keys(keys_dir: Path) -> list:
    """Load all trusted public keys from directory"""
    if not keys_dir.exists():
        raise ErrNoTrustedKeys(str(keys_dir))

    key_files = list(keys_dir.glob("*.asc"))
    if not key_files:
        raise ErrNoTrustedKeys(str(keys_dir))

    # If pgpy available, load key objects
    if pgpy is not None:
        keys = []
        for key_file in key_files:
            try:
                key, _ = pgpy.PGPKey.from_file(str(key_file))
                keys.append(key)
            except Exception:
                continue
        if keys:
            return keys

    # Fallback: return key file paths for gpg command (absolute paths)
    return [str(f.resolve()) for f in key_files]


def compute_file_hash(file_path: str) -> str:
    """Compute SHA256 hash of a file"""
    sha256 = hashlib.sha256()
    with open(file_path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            sha256.update(chunk)
    return sha256.hexdigest()


def verify_signature_gpg(
    manifest_path: str, sig_path: str, key_files: list, skill_name: str
) -> bool:
    """Verify PGP signature using system gpg command"""
    if not GPG_BIN:
        raise ErrSigInvalid(
            skill_name, "Neither pgpy nor gpg available for signature verification"
        )

    import tempfile

    with tempfile.TemporaryDirectory() as gnupg_home:
        # Set proper permissions for GNUPGHOME (GPG requires 700)
        os.chmod(gnupg_home, 0o700)

        # Use GNUPGHOME env var for proper GPG 2.x isolation
        env = os.environ.copy()
        env["GNUPGHOME"] = gnupg_home

        # Import all trusted keys into temporary keyring
        import_failed = []
        for key_file in key_files:
            result = subprocess.run(
                [GPG_BIN, "--batch", "--yes", "--import", key_file],
                capture_output=True,
                env=env,
            )
            if result.returncode != 0:
                import_failed.append(key_file)

        if import_failed and len(import_failed) == len(key_files):
            raise ErrSigInvalid(skill_name, "Failed to import any trusted keys")

        # Verify signature with trust-model always to bypass trustdb issues
        result = subprocess.run(
            [
                GPG_BIN,
                "--batch",
                "--yes",
                "--trust-model",
                "always",
                "--verify",
                sig_path,
                manifest_path,
            ],
            capture_output=True,
            env=env,
        )

        if result.returncode == 0:
            return True

        raise ErrSigInvalid(skill_name, result.stderr.decode().strip())


def verify_signature(
    manifest_path: str, sig_path: str, trusted_keys: list, skill_name: str
) -> bool:
    """Verify PGP signature of manifest"""
    # Check if trusted_keys contains pgpy key objects or file paths
    if trusted_keys and isinstance(trusted_keys[0], str):
        # File paths - use gpg command
        return verify_signature_gpg(manifest_path, sig_path, trusted_keys, skill_name)

    if pgpy is None:
        return verify_signature_gpg(manifest_path, sig_path, [], skill_name)

    with open(manifest_path, "rb") as f:
        manifest_data = f.read()

    sig = pgpy.PGPSignature.from_file(sig_path)

    for key in trusted_keys:
        try:
            verification = key.verify(manifest_data, sig)
            if verification:
                return True
        except Exception:
            continue

    raise ErrSigInvalid(skill_name, "No trusted key could verify the signature")


def verify_manifest_hashes(skill_dir: str, manifest: dict, skill_name: str) -> None:
    """Verify all file hashes in manifest"""
    for file_entry in manifest.get("files", []):
        rel_path = file_entry["path"]
        expected_hash = file_entry["hash"]

        full_path = os.path.join(skill_dir, rel_path)
        if not os.path.exists(full_path):
            raise ErrHashMismatch(skill_name, rel_path, expected_hash, "<FILE_MISSING>")

        actual_hash = compute_file_hash(full_path)
        if actual_hash != expected_hash:
            raise ErrHashMismatch(skill_name, rel_path, expected_hash, actual_hash)


def verify_skill(skill_dir: str, trusted_keys: list) -> tuple[bool, str]:
    """Verify a single skill directory"""
    skill_name = os.path.basename(skill_dir)
    signing_dir = os.path.join(skill_dir, SIGNING_DIR)
    manifest_path = os.path.join(signing_dir, "Manifest.json")
    sig_path = os.path.join(signing_dir, ".skill.sig")

    if not os.path.exists(manifest_path):
        raise ErrManifestMissing(skill_name)

    if not os.path.exists(sig_path):
        raise ErrSigMissing(skill_name)

    verify_signature(manifest_path, sig_path, trusted_keys, skill_name)

    with open(manifest_path, "r") as f:
        manifest = json.load(f)

    verify_manifest_hashes(skill_dir, manifest, skill_name)

    return True, skill_name


def verify_skills_dir(skills_dir: str, trusted_keys: list) -> dict[str, list]:
    """Verify all skills in a directory"""
    results = {"passed": [], "failed": []}

    if not os.path.isdir(skills_dir):
        print(f"[WARN] Skills directory not found: {skills_dir}")
        return results

    for entry in sorted(os.listdir(skills_dir)):
        skill_path = os.path.join(skills_dir, entry)
        if not os.path.isdir(skill_path) or entry.startswith("."):
            continue

        try:
            _, skill_name = verify_skill(skill_path, trusted_keys)
            results["passed"].append(skill_name)
        except Exception as e:
            results["failed"].append({"name": entry, "error": str(e)})

    return results


def run_verification(skill: str | None = None) -> dict[str, list]:
    """Run verification and return structured results.

    Handles the full workflow: load trusted keys, verify single skill or
    all configured directories, and aggregate results.

    Args:
        skill: Optional path to a single skill directory.  When *None*,
               all directories listed in ``config.conf`` are scanned.

    Returns:
        dict with ``passed`` (list[str]) and ``failed`` (list[dict]) keys.
    """
    trusted_keys = load_trusted_keys(DEFAULT_TRUSTED_KEYS_DIR)

    if skill is not None:
        try:
            verify_skill(skill, trusted_keys)
            return {"passed": [os.path.basename(skill)], "failed": []}
        except Exception as e:
            return {
                "passed": [],
                "failed": [{"name": os.path.basename(skill), "error": str(e)}],
            }

    config = load_config(DEFAULT_CONFIG)
    all_passed: list[str] = []
    all_failed: list[dict] = []

    for skills_dir in config.get("skills_dirs", []):
        results = verify_skills_dir(skills_dir, trusted_keys)
        all_passed.extend(results["passed"])
        all_failed.extend(results["failed"])

    return {"passed": all_passed, "failed": all_failed}


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Verify skill integrity and signatures"
    )
    parser.add_argument("--skill", "-s", help="Verify single skill directory")
    args = parser.parse_args()

    try:
        results = run_verification(args.skill)

        for name in results["passed"]:
            print(f"[OK] {name}")

        for item in results["failed"]:
            print(f"[ERROR] {item['name']}")
            print(f"  {item['error']}")

        print(f"\n{'='*50}")
        print(f"PASSED: {len(results['passed'])}")
        print(f"FAILED: {len(results['failed'])}")
        print(f"{'='*50}")

        if results["failed"]:
            print("VERIFICATION FAILED")
            return 1
        else:
            print("VERIFICATION PASSED")
            return 0

    except Exception as e:
        print(f"[ERROR] {e}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
