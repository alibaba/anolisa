"""Unit tests for security_events.config — log path selection."""

import unittest
from unittest.mock import patch

from agent_sec_cli.security_events.config import FALLBACK_LOG_PATH, PRIMARY_LOG_PATH, get_log_path


class TestGetLogPath(unittest.TestCase):
    @patch("agent_sec_cli.security_events.config.os.access", return_value=True)
    @patch("agent_sec_cli.security_events.config.os.path.isdir", return_value=True)
    @patch("agent_sec_cli.security_events.config.os.makedirs")
    def test_primary_path_when_writable(self, mock_makedirs, mock_isdir, mock_access):
        path = get_log_path()
        self.assertEqual(path, PRIMARY_LOG_PATH)

    @patch("agent_sec_cli.security_events.config.os.access", return_value=False)
    @patch("agent_sec_cli.security_events.config.os.path.isdir", return_value=True)
    @patch("agent_sec_cli.security_events.config.os.makedirs")
    def test_fallback_when_primary_not_writable(self, mock_makedirs, mock_isdir, mock_access):
        path = get_log_path()
        self.assertEqual(path, FALLBACK_LOG_PATH)

    @patch("agent_sec_cli.security_events.config.os.makedirs")
    def test_fallback_when_makedirs_fails(self, mock_makedirs):
        # First call (primary) raises, second call (fallback) succeeds
        mock_makedirs.side_effect = [OSError("permission denied"), None]
        path = get_log_path()
        self.assertEqual(path, FALLBACK_LOG_PATH)


if __name__ == "__main__":
    unittest.main()
