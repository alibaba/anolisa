import os
import stat
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from agent_sec_cli.skill_ledger.core.status import _keys_info
from agent_sec_cli.skill_ledger.signing.key_manager import (
    write_key_enc,
    write_key_pub,
)


class TestKeyFilePermissions(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()

    def tearDown(self):
        import shutil

        shutil.rmtree(self.tmpdir)

    def test_private_key_is_0600_and_public_key_is_0644(self):
        key_dir = Path(self.tmpdir) / "ledger-keys"
        with patch.dict(
            os.environ,
            {"AGENT_SEC_SKILL_LEDGER_KEY_DIR": str(key_dir)},
            clear=True,
        ):
            key_enc = write_key_enc(b"encrypted-private-key")
            key_pub = write_key_pub(b"public-key")

        self.assertEqual(stat.S_IMODE(key_enc.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(key_pub.stat().st_mode), 0o644)

    def test_status_does_not_read_private_key_contents(self):
        key_dir = Path(self.tmpdir) / "ledger-keys"
        with patch.dict(
            os.environ,
            {"AGENT_SEC_SKILL_LEDGER_KEY_DIR": str(key_dir)},
            clear=True,
        ):
            key_enc = write_key_enc(b"encrypted-private-key")
            write_key_pub(b"public-key")
            key_enc.chmod(0o000)

            info = _keys_info()

        self.assertTrue(info["initialized"])
        self.assertTrue(info["encrypted"])


if __name__ == "__main__":
    unittest.main()
