#!/usr/bin/env python3
"""Integration tests for skill verifier"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

# Add asset-verify to path (updated for new location: tests/integration-test/asset-verify -> agent-sec-cli/asset-verify)
sys.path.insert(
    0, os.path.join(os.path.dirname(__file__), "../../../agent-sec-cli/asset-verify")
)

from errors import (
    ErrConfigMissing,
    ErrHashMismatch,
    ErrManifestMissing,
    ErrNoTrustedKeys,
    ErrSigMissing,
)
from verifier import (
    compute_file_hash,
    load_config,
    load_trusted_keys,
    verify_manifest_hashes,
    verify_skill,
    verify_skills_dir,
)


class TestComputeFileHash(unittest.TestCase):
    def test_hash_computation(self):
        with tempfile.NamedTemporaryFile(mode="w", delete=False) as f:
            f.write("test content")
            f.flush()
            path = f.name

        try:
            h = compute_file_hash(path)
            self.assertEqual(len(h), 64)  # SHA256 hex length
            # Known hash for "test content"
            self.assertEqual(
                h, "6ae8a75555209fd6c44157c0aed8016e763ff435a19cf186f76863140143ff72"
            )
        finally:
            os.unlink(path)


class TestVerifyManifestHashes(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        # Create test file
        self.test_file = os.path.join(self.tmpdir, "main.py")
        with open(self.test_file, "w") as f:
            f.write("print('hello')")
        self.file_hash = compute_file_hash(self.test_file)

    def tearDown(self):
        shutil.rmtree(self.tmpdir)

    def test_valid_hashes(self):
        manifest = {"files": [{"path": "main.py", "hash": self.file_hash}]}
        # Should not raise
        verify_manifest_hashes(self.tmpdir, manifest, "test_skill")

    def test_hash_mismatch(self):
        manifest = {"files": [{"path": "main.py", "hash": "wrong_hash"}]}
        with self.assertRaises(ErrHashMismatch):
            verify_manifest_hashes(self.tmpdir, manifest, "test_skill")

    def test_missing_file(self):
        manifest = {"files": [{"path": "nonexistent.py", "hash": "somehash"}]}
        with self.assertRaises(ErrHashMismatch) as ctx:
            verify_manifest_hashes(self.tmpdir, manifest, "test_skill")
        self.assertIn("FILE_MISSING", str(ctx.exception))


class TestVerifySkill(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.skill_dir = os.path.join(self.tmpdir, "test_skill")
        os.makedirs(self.skill_dir)

        # Create test file
        test_file = os.path.join(self.skill_dir, "main.py")
        with open(test_file, "w") as f:
            f.write("print('hello')")

        self.file_hash = compute_file_hash(test_file)

    def tearDown(self):
        shutil.rmtree(self.tmpdir)

    def test_missing_manifest(self):
        # Create sig inside .skill-meta but no manifest
        meta_dir = os.path.join(self.skill_dir, ".skill-meta")
        os.makedirs(meta_dir, exist_ok=True)
        sig_path = os.path.join(meta_dir, ".skill.sig")
        with open(sig_path, "w") as f:
            f.write("fake sig")

        with self.assertRaises(ErrManifestMissing):
            verify_skill(self.skill_dir, [])

    def test_missing_sig(self):
        # Create manifest inside .skill-meta but no sig
        manifest = {
            "version": "0.1",
            "skill_name": "test_skill",
            "algorithm": "SHA256",
            "files": [{"path": "main.py", "hash": self.file_hash}],
        }
        meta_dir = os.path.join(self.skill_dir, ".skill-meta")
        os.makedirs(meta_dir, exist_ok=True)
        manifest_path = os.path.join(meta_dir, "Manifest.json")
        with open(manifest_path, "w") as f:
            json.dump(manifest, f)

        with self.assertRaises(ErrSigMissing):
            verify_skill(self.skill_dir, [])


class TestVerifySkillsDir(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()

    def tearDown(self):
        shutil.rmtree(self.tmpdir)

    def test_nonexistent_dir(self):
        results = verify_skills_dir("/nonexistent/path", [])
        self.assertEqual(results["passed"], [])
        self.assertEqual(results["failed"], [])

    def test_empty_dir(self):
        empty_dir = os.path.join(self.tmpdir, "empty_skills")
        os.makedirs(empty_dir)
        results = verify_skills_dir(empty_dir, [])
        self.assertEqual(results["passed"], [])
        self.assertEqual(results["failed"], [])


class TestLoadConfig(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()

    def tearDown(self):
        shutil.rmtree(self.tmpdir)

    def test_missing_config(self):
        from pathlib import Path

        with self.assertRaises(ErrConfigMissing):
            load_config(Path("/nonexistent/config.conf"))

    def test_single_skills_dir(self):
        from pathlib import Path

        config_path = os.path.join(self.tmpdir, "config.conf")
        with open(config_path, "w") as f:
            f.write("skills_dir = /opt/skills\n")

        config = load_config(Path(config_path))
        self.assertEqual(config["skills_dirs"], ["/opt/skills"])

    def test_list_skills_dir(self):
        from pathlib import Path

        config_path = os.path.join(self.tmpdir, "config.conf")
        with open(config_path, "w") as f:
            f.write("skills_dir = [\n")
            f.write("    /opt/skills1\n")
            f.write("    /opt/skills2\n")
            f.write("]\n")

        config = load_config(Path(config_path))
        self.assertEqual(config["skills_dirs"], ["/opt/skills1", "/opt/skills2"])


class TestLoadTrustedKeys(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()

    def tearDown(self):
        shutil.rmtree(self.tmpdir)

    def test_nonexistent_dir(self):
        from pathlib import Path

        with self.assertRaises(ErrNoTrustedKeys):
            load_trusted_keys(Path("/nonexistent/keys"))

    def test_empty_keys_dir(self):
        from pathlib import Path

        with self.assertRaises(ErrNoTrustedKeys):
            load_trusted_keys(Path(self.tmpdir))


class TestIntegrationWithGPG(unittest.TestCase):
    """Integration test using system gpg"""

    @classmethod
    def setUpClass(cls):
        cls.gpg_available = shutil.which("gpg") is not None
        if not cls.gpg_available:
            return

        cls.tmpdir = tempfile.mkdtemp()
        cls.keys_dir = os.path.join(cls.tmpdir, "keys")
        cls.skills_dir = os.path.join(cls.tmpdir, "skills")
        cls.skill_dir = os.path.join(cls.skills_dir, "test_skill")
        os.makedirs(cls.keys_dir)
        os.makedirs(cls.skill_dir)

        # Generate test key pair
        cls.gpg_home = os.path.join(cls.tmpdir, "gnupg")
        os.makedirs(cls.gpg_home, mode=0o700)

        key_params = """
Key-Type: RSA
Key-Length: 2048
Name-Real: Test Key
Name-Email: test@test.com
Expire-Date: 0
%no-protection
%commit
"""
        subprocess.run(
            ["gpg", "--homedir", cls.gpg_home, "--batch", "--gen-key"],
            input=key_params.encode(),
            capture_output=True,
        )

        # Export public key
        pub_key_path = os.path.join(cls.keys_dir, "test.asc")
        with open(pub_key_path, "w") as f:
            subprocess.run(
                [
                    "gpg",
                    "--homedir",
                    cls.gpg_home,
                    "--armor",
                    "--export",
                    "test@test.com",
                ],
                stdout=f,
            )

        # Create test skill files
        main_py = os.path.join(cls.skill_dir, "main.py")
        with open(main_py, "w") as f:
            f.write("print('hello')")

        # Create .skill-meta directory and manifest
        from verifier import compute_file_hash

        cls.meta_dir = os.path.join(cls.skill_dir, ".skill-meta")
        os.makedirs(cls.meta_dir)

        manifest = {
            "version": "0.1",
            "skill_name": "test_skill",
            "algorithm": "SHA256",
            "files": [{"path": "main.py", "hash": compute_file_hash(main_py)}],
        }
        cls.manifest_path = os.path.join(cls.meta_dir, "Manifest.json")
        with open(cls.manifest_path, "w") as f:
            json.dump(manifest, f)

        # Sign manifest
        cls.sig_path = os.path.join(cls.meta_dir, ".skill.sig")
        subprocess.run(
            [
                "gpg",
                "--homedir",
                cls.gpg_home,
                "--armor",
                "--detach-sign",
                "--output",
                cls.sig_path,
                cls.manifest_path,
            ],
            capture_output=True,
        )

    @classmethod
    def tearDownClass(cls):
        if hasattr(cls, "tmpdir") and os.path.exists(cls.tmpdir):
            shutil.rmtree(cls.tmpdir)

    def test_full_verification(self):
        if not self.gpg_available:
            self.skipTest("gpg not available")

        from pathlib import Path

        keys = load_trusted_keys(Path(self.keys_dir))
        self.assertTrue(len(keys) > 0)

        success, name = verify_skill(self.skill_dir, keys)
        self.assertTrue(success)
        self.assertEqual(name, "test_skill")

    def test_batch_verification(self):
        if not self.gpg_available:
            self.skipTest("gpg not available")

        from pathlib import Path

        keys = load_trusted_keys(Path(self.keys_dir))

        results = verify_skills_dir(self.skills_dir, keys)
        self.assertEqual(results["passed"], ["test_skill"])
        self.assertEqual(results["failed"], [])


if __name__ == "__main__":
    unittest.main()
