"""End-to-end workflow tests for check, certify, and audit.

These tests use real Ed25519 cryptography with temp directories — no mocks
for the signing layer.  They protect the actual security-critical paths:

1. **Check state machine** — the security gate called on every skill invocation.
   Every state (none/drifted/tampered/deny/warn/pass) must be correct.
2. **Certify scan merge** — scanner results are accumulated correctly, old
   entries for the same scanner replaced (not duplicated).
3. **Audit chain integrity** — broken previousManifestSignature chain and
   tampered manifestHash must both be detected.
"""

import base64
import hashlib
import json
import os
import shutil
import tempfile
import unittest

from agent_sec_cli.skill_ledger.core.auditor import audit
from agent_sec_cli.skill_ledger.core.certifier import certify
from agent_sec_cli.skill_ledger.core.checker import check
from agent_sec_cli.skill_ledger.core.file_hasher import (
    compute_file_hashes,
    diff_file_hashes,
)
from agent_sec_cli.skill_ledger.errors import SignatureInvalidError
from agent_sec_cli.skill_ledger.signing.base import SigningBackend
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import (
    Encoding,
    NoEncryption,
    PrivateFormat,
    PublicFormat,
)

# ---------------------------------------------------------------------------
# In-memory Ed25519 backend for testing (no filesystem key storage)
# ---------------------------------------------------------------------------


class InMemoryEd25519Backend(SigningBackend):
    """A test-only signing backend that holds keys in memory."""

    def __init__(self):
        self._private_key = Ed25519PrivateKey.generate()
        raw_pub = self._private_key.public_key().public_bytes(
            Encoding.Raw, PublicFormat.Raw
        )
        self._fingerprint = f"sha256:{hashlib.sha256(raw_pub).hexdigest()}"

    @property
    def name(self) -> str:
        return "ed25519"

    def generate_keys(self, passphrase=None):
        """No-op for in-memory backend — keys are generated in __init__."""
        return {"fingerprint": self._fingerprint}

    def sign(self, data: bytes) -> tuple[str, str]:
        raw_sig = self._private_key.sign(data)
        return base64.b64encode(raw_sig).decode("ascii"), self._fingerprint

    def verify(self, data: bytes, signature_b64: str, fingerprint: str) -> bool:
        if fingerprint != self._fingerprint:
            raise SignatureInvalidError(f"Unknown fingerprint {fingerprint}")
        from cryptography.exceptions import InvalidSignature

        raw_sig = base64.b64decode(signature_b64)
        try:
            self._private_key.public_key().verify(raw_sig, data)
            return True
        except InvalidSignature:
            raise SignatureInvalidError("Signature verification failed")

    def get_public_key_fingerprint(self) -> str:
        return self._fingerprint


# ---------------------------------------------------------------------------
# Test helper: manage a temp skill directory
# ---------------------------------------------------------------------------


class SkillDirTestCase(unittest.TestCase):
    """Base class that creates a temp skill directory with sample files."""

    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.skill_dir = os.path.join(self.tmpdir, "test-skill")
        os.makedirs(self.skill_dir)
        # Create sample skill files
        self._write_file("run.sh", "#!/bin/bash\necho hello\n")
        self._write_file("SKILL.md", "# Test Skill\n")
        self.backend = InMemoryEd25519Backend()
        # Patch config to avoid touching user's real config
        self._patch_config()

    def tearDown(self):
        shutil.rmtree(self.tmpdir, ignore_errors=True)

    def _write_file(self, name: str, content: str):
        path = os.path.join(self.skill_dir, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(content)

    def _write_findings(self, findings: list[dict]) -> str:
        path = os.path.join(self.tmpdir, "findings.json")
        with open(path, "w") as f:
            json.dump(findings, f)
        return path

    def _patch_config(self):
        """Point config to a temp config dir so tests don't touch real user config."""
        config_dir = os.path.join(self.tmpdir, "config")
        os.makedirs(config_dir, exist_ok=True)
        os.environ["XDG_CONFIG_HOME"] = self.tmpdir
        self.addCleanup(lambda: os.environ.pop("XDG_CONFIG_HOME", None))


# ---------------------------------------------------------------------------
# Check state machine
# ---------------------------------------------------------------------------


class TestCheckStateMachine(SkillDirTestCase):
    """Tests for the check command — the security gate.

    The check state machine has 6 possible outputs:
    none, drifted, tampered, deny, warn, pass.
    Each represents a distinct security posture.
    """

    def test_no_manifest_creates_one_returns_none(self):
        """First check on a fresh skill → auto-create manifest, status=none."""
        result = check(self.skill_dir, self.backend)
        self.assertEqual(result["status"], "none")
        # .skill-meta/latest.json should now exist
        latest = os.path.join(self.skill_dir, ".skill-meta", "latest.json")
        self.assertTrue(os.path.isfile(latest))

    def test_unchanged_after_certify_pass(self):
        """certify with all-pass findings → check returns pass."""
        findings_path = self._write_findings(
            [
                {"rule": "r1", "level": "pass", "message": "ok"},
            ]
        )
        certify(self.skill_dir, self.backend, findings_path=findings_path)
        result = check(self.skill_dir, self.backend)
        self.assertEqual(result["status"], "pass")

    def test_drifted_after_file_change(self):
        """Modifying a skill file → check returns drifted."""
        # First, establish a signed manifest
        check(self.skill_dir, self.backend)
        # Modify a file
        self._write_file("run.sh", "#!/bin/bash\necho MODIFIED\n")
        result = check(self.skill_dir, self.backend)
        self.assertEqual(result["status"], "drifted")
        self.assertIn("modified", result)

    def test_drifted_on_file_added(self):
        """Adding a new file → check returns drifted with added list."""
        check(self.skill_dir, self.backend)
        self._write_file("new_file.py", "print('hello')\n")
        result = check(self.skill_dir, self.backend)
        self.assertEqual(result["status"], "drifted")
        self.assertIn("new_file.py", result["added"])

    def test_drifted_on_file_removed(self):
        """Removing a file → check returns drifted with removed list."""
        check(self.skill_dir, self.backend)
        os.remove(os.path.join(self.skill_dir, "run.sh"))
        result = check(self.skill_dir, self.backend)
        self.assertEqual(result["status"], "drifted")
        self.assertIn("run.sh", result["removed"])

    def test_tampered_manifest_hash(self):
        """Directly editing the manifest JSON → tampered (hash mismatch)."""
        check(self.skill_dir, self.backend)  # creates signed manifest
        latest = os.path.join(self.skill_dir, ".skill-meta", "latest.json")
        with open(latest, "r") as f:
            data = json.load(f)
        # Tamper: change scanStatus without re-hashing
        data["scanStatus"] = "pass"
        with open(latest, "w") as f:
            json.dump(data, f)
        result = check(self.skill_dir, self.backend)
        self.assertEqual(result["status"], "tampered")

    def test_tampered_wrong_key_signature(self):
        """Signing with a different key → tampered (signature mismatch)."""
        check(self.skill_dir, self.backend)
        # Re-sign the manifest with a different key
        other_backend = InMemoryEd25519Backend()
        latest = os.path.join(self.skill_dir, ".skill-meta", "latest.json")
        with open(latest, "r") as f:
            data = json.load(f)
        # Recompute hash and sign with wrong key
        from agent_sec_cli.skill_ledger.models.manifest import SignedManifest

        m = SignedManifest.from_json(json.dumps(data))
        m.manifestHash = m.compute_manifest_hash()
        sig_val, fp = other_backend.sign(m.manifestHash.encode("utf-8"))
        data["manifestHash"] = m.manifestHash
        data["signature"]["value"] = sig_val
        data["signature"]["keyFingerprint"] = fp
        with open(latest, "w") as f:
            json.dump(data, f)
        result = check(self.skill_dir, self.backend)
        self.assertEqual(result["status"], "tampered")

    def test_deny_status_passthrough(self):
        """certify with deny findings → check returns deny."""
        findings_path = self._write_findings(
            [
                {"rule": "dangerous-exec", "level": "deny", "message": "exec found"},
            ]
        )
        certify(self.skill_dir, self.backend, findings_path=findings_path)
        result = check(self.skill_dir, self.backend)
        self.assertEqual(result["status"], "deny")

    def test_warn_status_passthrough(self):
        """certify with warn findings → check returns warn."""
        findings_path = self._write_findings(
            [
                {"rule": "obfuscated", "level": "warn", "message": "hex encoded"},
            ]
        )
        certify(self.skill_dir, self.backend, findings_path=findings_path)
        result = check(self.skill_dir, self.backend)
        self.assertEqual(result["status"], "warn")

    def test_check_exit_code_pass_is_zero(self):
        """check returning pass should indicate success (exit code 0)."""
        findings_path = self._write_findings(
            [
                {"rule": "r1", "level": "pass", "message": "ok"},
            ]
        )
        certify(self.skill_dir, self.backend, findings_path=findings_path)
        result = check(self.skill_dir, self.backend)
        self.assertEqual(result["status"], "pass")
        # status "pass" should NOT be in the failure set
        self.assertNotIn(result["status"], ("tampered", "deny"))

    def test_check_tampered_is_security_critical(self):
        """check returning tampered is a security-critical state."""
        check(self.skill_dir, self.backend)
        latest = os.path.join(self.skill_dir, ".skill-meta", "latest.json")
        with open(latest, "r") as f:
            data = json.load(f)
        data["scanStatus"] = "pass"  # tamper without re-hashing
        with open(latest, "w") as f:
            json.dump(data, f)
        result = check(self.skill_dir, self.backend)
        self.assertEqual(result["status"], "tampered")
        self.assertIn(result["status"], ("tampered", "deny"))

    def test_check_deny_is_security_critical(self):
        """check returning deny is a security-critical state."""
        findings_path = self._write_findings(
            [
                {"rule": "dangerous-exec", "level": "deny", "message": "exec found"},
            ]
        )
        certify(self.skill_dir, self.backend, findings_path=findings_path)
        result = check(self.skill_dir, self.backend)
        self.assertEqual(result["status"], "deny")
        self.assertIn(result["status"], ("tampered", "deny"))


# ---------------------------------------------------------------------------
# Certify workflow
# ---------------------------------------------------------------------------


class TestCertifyWorkflow(SkillDirTestCase):
    """Tests for the certify command — manifest creation and scan merging."""

    def test_certify_creates_version_and_snapshot(self):
        """First certify → creates v000001 manifest + snapshot."""
        findings_path = self._write_findings(
            [
                {"rule": "r1", "level": "pass", "message": "clean"},
            ]
        )
        result = certify(self.skill_dir, self.backend, findings_path=findings_path)
        self.assertEqual(result["versionId"], "v000001")
        self.assertTrue(result["newVersion"])
        self.assertEqual(result["scanStatus"], "pass")
        # Version file and snapshot should exist
        v_file = os.path.join(self.skill_dir, ".skill-meta", "versions", "v000001.json")
        v_snap = os.path.join(
            self.skill_dir, ".skill-meta", "versions", "v000001.snapshot"
        )
        self.assertTrue(os.path.isfile(v_file))
        self.assertTrue(os.path.isdir(v_snap))

    def test_recertify_same_files_no_new_version(self):
        """Certifying again without file changes → same versionId, no new version."""
        findings_path = self._write_findings(
            [
                {"rule": "r1", "level": "pass", "message": "clean"},
            ]
        )
        r1 = certify(self.skill_dir, self.backend, findings_path=findings_path)
        r2 = certify(self.skill_dir, self.backend, findings_path=findings_path)
        self.assertEqual(r1["versionId"], r2["versionId"])
        self.assertFalse(r2["newVersion"])

    def test_certify_after_file_change_creates_new_version(self):
        """File change between certifies → new version created."""
        findings_path = self._write_findings(
            [
                {"rule": "r1", "level": "pass", "message": "clean"},
            ]
        )
        r1 = certify(self.skill_dir, self.backend, findings_path=findings_path)
        self._write_file("run.sh", "#!/bin/bash\necho modified\n")
        r2 = certify(self.skill_dir, self.backend, findings_path=findings_path)
        self.assertEqual(r1["versionId"], "v000001")
        self.assertEqual(r2["versionId"], "v000002")
        self.assertTrue(r2["newVersion"])

    def test_scan_entry_merge_replaces_same_scanner(self):
        """Re-certifying with the same scanner replaces the old entry, not appends."""
        findings_warn = self._write_findings(
            [
                {"rule": "r1", "level": "warn", "message": "first scan"},
            ]
        )
        certify(self.skill_dir, self.backend, findings_path=findings_warn)

        findings_pass = self._write_findings(
            [
                {"rule": "r1", "level": "pass", "message": "fixed"},
            ]
        )
        result = certify(self.skill_dir, self.backend, findings_path=findings_pass)
        self.assertEqual(result["scanStatus"], "pass")  # was warn, now pass

        # Verify only one scan entry in manifest (not two)
        latest = os.path.join(self.skill_dir, ".skill-meta", "latest.json")
        with open(latest, "r") as f:
            data = json.load(f)
        self.assertEqual(len(data["scans"]), 1)

    def test_deny_finding_produces_deny_status(self):
        findings_path = self._write_findings(
            [
                {"rule": "r1", "level": "pass", "message": "ok"},
                {"rule": "r2", "level": "deny", "message": "bad"},
            ]
        )
        result = certify(self.skill_dir, self.backend, findings_path=findings_path)
        self.assertEqual(result["scanStatus"], "deny")

    def test_auto_invoke_mode_no_crash(self):
        """Certify without --findings (auto-invoke) should not crash in v1."""
        # First create a manifest
        check(self.skill_dir, self.backend)
        # Auto-invoke mode — no invocable scanners, should succeed gracefully
        result = certify(self.skill_dir, self.backend)
        self.assertIn("versionId", result)


# ---------------------------------------------------------------------------
# Audit chain verification
# ---------------------------------------------------------------------------


class TestAuditChainIntegrity(SkillDirTestCase):
    """Tests for the audit command — version chain integrity verification."""

    def test_valid_single_version_passes(self):
        findings_path = self._write_findings(
            [
                {"rule": "r1", "level": "pass", "message": "ok"},
            ]
        )
        certify(self.skill_dir, self.backend, findings_path=findings_path)
        result = audit(self.skill_dir, self.backend)
        self.assertTrue(result["valid"])
        self.assertEqual(result["versions_checked"], 1)

    def test_valid_multi_version_chain(self):
        """Two certifies with file change → two versions, chain should be valid."""
        findings_path = self._write_findings(
            [
                {"rule": "r1", "level": "pass", "message": "ok"},
            ]
        )
        certify(self.skill_dir, self.backend, findings_path=findings_path)
        self._write_file("run.sh", "#!/bin/bash\necho v2\n")
        certify(self.skill_dir, self.backend, findings_path=findings_path)
        result = audit(self.skill_dir, self.backend)
        self.assertTrue(result["valid"])
        self.assertEqual(result["versions_checked"], 2)

    def test_tampered_hash_detected(self):
        """Modifying a version manifest's content → audit detects hash mismatch."""
        findings_path = self._write_findings(
            [
                {"rule": "r1", "level": "pass", "message": "ok"},
            ]
        )
        certify(self.skill_dir, self.backend, findings_path=findings_path)
        # Tamper with the version file
        v_file = os.path.join(self.skill_dir, ".skill-meta", "versions", "v000001.json")
        with open(v_file, "r") as f:
            data = json.load(f)
        data["scanStatus"] = (
            "deny"  # tamper: was "pass", now "deny" — without re-hashing
        )
        with open(v_file, "w") as f:
            json.dump(data, f)
        result = audit(self.skill_dir, self.backend)
        self.assertFalse(result["valid"])
        error_msgs = [e["error"] for e in result["errors"]]
        self.assertTrue(any("manifestHash" in msg for msg in error_msgs))

    def test_broken_chain_detected(self):
        """Corrupting previousManifestSignature → audit detects chain break."""
        findings_path = self._write_findings(
            [
                {"rule": "r1", "level": "pass", "message": "ok"},
            ]
        )
        certify(self.skill_dir, self.backend, findings_path=findings_path)
        self._write_file("run.sh", "#!/bin/bash\necho v2\n")
        certify(self.skill_dir, self.backend, findings_path=findings_path)

        # Tamper with v000002's previousManifestSignature
        v2_file = os.path.join(
            self.skill_dir, ".skill-meta", "versions", "v000002.json"
        )
        with open(v2_file, "r") as f:
            data = json.load(f)
        data["previousManifestSignature"] = "BROKEN"
        # Re-hash and re-sign to avoid hash mismatch detection (test chain specifically)
        from agent_sec_cli.skill_ledger.models.manifest import SignedManifest

        m = SignedManifest.from_json(json.dumps(data))
        m.manifestHash = m.compute_manifest_hash()
        sig_val, fp = self.backend.sign(m.manifestHash.encode("utf-8"))
        data["manifestHash"] = m.manifestHash
        data["signature"]["value"] = sig_val
        data["signature"]["keyFingerprint"] = fp
        with open(v2_file, "w") as f:
            json.dump(data, f)

        result = audit(self.skill_dir, self.backend)
        self.assertFalse(result["valid"])
        error_msgs = [e["error"] for e in result["errors"]]
        self.assertTrue(any("chain broken" in msg for msg in error_msgs))

    def test_no_versions_returns_valid(self):
        """Empty .skill-meta (no versions) → audit succeeds with 0 checked."""
        os.makedirs(
            os.path.join(self.skill_dir, ".skill-meta", "versions"), exist_ok=True
        )
        result = audit(self.skill_dir, self.backend)
        self.assertTrue(result["valid"])
        self.assertEqual(result["versions_checked"], 0)


# ---------------------------------------------------------------------------
# File hash diff
# ---------------------------------------------------------------------------


class TestFileHashDiff(SkillDirTestCase):
    """Tests for file integrity diff — drives drifted/unchanged decisions."""

    def test_identical_hashes_match(self):
        hashes = compute_file_hashes(self.skill_dir)
        diff = diff_file_hashes(hashes, hashes)
        self.assertTrue(diff["match"])
        self.assertEqual(diff["added"], [])
        self.assertEqual(diff["removed"], [])
        self.assertEqual(diff["modified"], [])

    def test_added_file_detected(self):
        old = compute_file_hashes(self.skill_dir)
        self._write_file("new.py", "print('hi')\n")
        new = compute_file_hashes(self.skill_dir)
        diff = diff_file_hashes(old, new)
        self.assertFalse(diff["match"])
        self.assertIn("new.py", diff["added"])

    def test_removed_file_detected(self):
        old = compute_file_hashes(self.skill_dir)
        os.remove(os.path.join(self.skill_dir, "run.sh"))
        new = compute_file_hashes(self.skill_dir)
        diff = diff_file_hashes(old, new)
        self.assertFalse(diff["match"])
        self.assertIn("run.sh", diff["removed"])

    def test_modified_file_detected(self):
        old = compute_file_hashes(self.skill_dir)
        self._write_file("run.sh", "#!/bin/bash\necho CHANGED\n")
        new = compute_file_hashes(self.skill_dir)
        diff = diff_file_hashes(old, new)
        self.assertFalse(diff["match"])
        self.assertIn("run.sh", diff["modified"])

    def test_skill_meta_excluded(self):
        """The .skill-meta directory must be excluded from hashing."""
        os.makedirs(os.path.join(self.skill_dir, ".skill-meta"), exist_ok=True)
        with open(os.path.join(self.skill_dir, ".skill-meta", "latest.json"), "w") as f:
            f.write("{}")
        hashes = compute_file_hashes(self.skill_dir)
        self.assertNotIn(".skill-meta/latest.json", hashes)

    def test_git_dir_excluded(self):
        """The .git directory must be excluded from hashing."""
        os.makedirs(os.path.join(self.skill_dir, ".git"), exist_ok=True)
        with open(os.path.join(self.skill_dir, ".git", "config"), "w") as f:
            f.write("[core]")
        hashes = compute_file_hashes(self.skill_dir)
        self.assertNotIn(".git/config", hashes)


if __name__ == "__main__":
    unittest.main()
