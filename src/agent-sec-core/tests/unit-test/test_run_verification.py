"""Unit tests for run_verification() output structure.

run_verification is the contract boundary between asset_verify and the
security middleware backend.  These tests mock verify_skill / load_trusted_keys
so we can validate the returned dict shape for every code path without
needing real GPG keys or signed skills.
"""

import unittest
from unittest.mock import patch

from agent_sec_cli.asset_verify.errors import ErrManifestMissing, ErrSigInvalid
from agent_sec_cli.asset_verify.verifier import run_verification

_MOD = "agent_sec_cli.asset_verify.verifier"


@patch(f"{_MOD}.load_trusted_keys", return_value=["fake-key"])
class TestRunVerificationSingleSkill(unittest.TestCase):
    """Single-skill path: run_verification(skill=<path>)."""

    @patch(f"{_MOD}.verify_skill", return_value=(True, "my-skill"))
    def test_success_returns_passed_list(self, _mock_vs, _mock_keys):
        result = run_verification(skill="/opt/skills/my-skill")

        self.assertIsInstance(result["passed"], list)
        self.assertIsInstance(result["failed"], list)
        self.assertEqual(result["passed"], ["my-skill"])
        self.assertEqual(result["failed"], [])

    @patch(f"{_MOD}.verify_skill", side_effect=ErrSigInvalid("bad-skill", "bad sig"))
    def test_failure_returns_failed_list(self, _mock_vs, _mock_keys):
        result = run_verification(skill="/opt/skills/bad-skill")

        self.assertIsInstance(result["passed"], list)
        self.assertIsInstance(result["failed"], list)
        self.assertEqual(result["passed"], [])
        self.assertEqual(len(result["failed"]), 1)
        self.assertEqual(result["failed"][0]["name"], "bad-skill")
        self.assertIn("bad sig", result["failed"][0]["error"])

    @patch(f"{_MOD}.verify_skill", side_effect=ErrManifestMissing("no-manifest"))
    def test_missing_manifest_returns_failed_list(self, _mock_vs, _mock_keys):
        result = run_verification(skill="/opt/skills/no-manifest")

        self.assertEqual(result["passed"], [])
        self.assertEqual(len(result["failed"]), 1)
        self.assertIn("name", result["failed"][0])
        self.assertIn("error", result["failed"][0])


@patch(f"{_MOD}.load_trusted_keys", return_value=["fake-key"])
@patch(f"{_MOD}.load_config", return_value={"skills_dirs": ["/opt/skills"]})
class TestRunVerificationBatch(unittest.TestCase):
    """Batch path: run_verification(skill=None)."""

    @patch(
        f"{_MOD}.verify_skills_dir",
        return_value={"passed": ["a", "b"], "failed": [{"name": "c", "error": "err"}]},
    )
    def test_batch_aggregates_results(self, _mock_vsd, _mock_cfg, _mock_keys):
        result = run_verification(skill=None)

        self.assertIsInstance(result["passed"], list)
        self.assertIsInstance(result["failed"], list)
        self.assertEqual(result["passed"], ["a", "b"])
        self.assertEqual(len(result["failed"]), 1)
        self.assertEqual(result["failed"][0]["name"], "c")


if __name__ == "__main__":
    unittest.main()
