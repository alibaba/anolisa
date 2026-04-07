"""Unit tests for security_middleware.backends.asset_verify — AssetVerifyBackend.

verifier.py is mocked — we test the backend's orchestration and error handling.
"""

import unittest
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

from agent_sec_cli.security_middleware.backends.asset_verify import AssetVerifyBackend
from agent_sec_cli.security_middleware.context import RequestContext


def _make_mock_verifier(
    verify_skill_result=None,
    verify_skill_side_effect=None,
    verify_skills_dir_result=None,
):
    """Build a mock verifier module with configurable behaviour."""
    verifier = MagicMock()
    verifier.DEFAULT_TRUSTED_KEYS_DIR = "/mock/keys"
    verifier.DEFAULT_CONFIG = "/mock/config.conf"
    verifier.load_trusted_keys.return_value = ["key1"]

    if verify_skill_side_effect:
        verifier.verify_skill.side_effect = verify_skill_side_effect
    elif verify_skill_result is not None:
        verifier.verify_skill.return_value = verify_skill_result
    else:
        verifier.verify_skill.return_value = None  # success = no exception

    if verify_skills_dir_result is not None:
        verifier.verify_skills_dir.return_value = verify_skills_dir_result
    else:
        verifier.verify_skills_dir.return_value = {"passed": [], "failed": []}

    verifier.load_config.return_value = {"skills_dirs": ["/mock/skills"]}
    return verifier


class TestAssetVerifyBackend(unittest.TestCase):
    def setUp(self):
        self.backend = AssetVerifyBackend()
        self.ctx = RequestContext(action="verify")

    @patch.object(AssetVerifyBackend, "_import_verifier")
    def test_single_skill_pass(self, mock_import):
        mock_import.return_value = _make_mock_verifier()
        result = self.backend.execute(self.ctx, skill="/path/to/my-skill")

        self.assertTrue(result.success)
        self.assertEqual(result.data["passed"], 1)
        self.assertEqual(result.data["failed"], 0)
        self.assertIn("[OK]", result.stdout)

    @patch.object(AssetVerifyBackend, "_import_verifier")
    def test_single_skill_fail(self, mock_import):
        mock_import.return_value = _make_mock_verifier(
            verify_skill_side_effect=Exception("signature mismatch")
        )
        result = self.backend.execute(self.ctx, skill="/path/to/bad-skill")

        self.assertFalse(result.success)
        self.assertEqual(result.data["failed"], 1)
        self.assertIn("[ERROR]", result.stdout)

    @patch.object(AssetVerifyBackend, "_import_verifier")
    def test_full_scan(self, mock_import):
        verifier = _make_mock_verifier(
            verify_skills_dir_result={
                "passed": ["skill-a", "skill-b"],
                "failed": [{"name": "skill-c", "error": "bad sig"}],
            }
        )
        mock_import.return_value = verifier
        result = self.backend.execute(self.ctx)

        self.assertFalse(result.success)
        self.assertEqual(result.data["passed"], 2)
        self.assertEqual(result.data["failed"], 1)

    @patch.object(AssetVerifyBackend, "_import_verifier")
    def test_import_failure(self, mock_import):
        mock_import.side_effect = ImportError("no module")
        result = self.backend.execute(self.ctx)

        self.assertFalse(result.success)
        self.assertIn("Failed to import verifier", result.error)


if __name__ == "__main__":
    unittest.main()
