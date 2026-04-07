"""Unit tests for security_events — module-level log_event() and get_writer()."""

import unittest
from unittest.mock import MagicMock, patch

from agent_sec_cli.security_events.schema import SecurityEvent


class TestGetWriter(unittest.TestCase):
    def test_singleton_returns_same_instance(self):
        import agent_sec_cli.security_events
        # Reset singleton
        agent_sec_cli.security_events._writer = None
        w1 = agent_sec_cli.security_events.get_writer()
        w2 = agent_sec_cli.security_events.get_writer()
        self.assertIs(w1, w2)
        # Cleanup
        agent_sec_cli.security_events._writer = None


class TestLogEvent(unittest.TestCase):
    @patch("agent_sec_cli.security_events.get_writer")
    def test_log_event_delegates_to_writer(self, mock_get_writer):
        mock_writer = MagicMock()
        mock_get_writer.return_value = mock_writer

        from agent_sec_cli.security_events import log_event

        evt = SecurityEvent(event_type="t", category="c", details={})
        log_event(evt)

        mock_writer.write.assert_called_once_with(evt)

    @patch("agent_sec_cli.security_events.get_writer")
    def test_log_event_swallows_exceptions(self, mock_get_writer):
        mock_writer = MagicMock()
        mock_writer.write.side_effect = RuntimeError("disk full")
        mock_get_writer.return_value = mock_writer

        from agent_sec_cli.security_events import log_event

        evt = SecurityEvent(event_type="t", category="c", details={})
        # Should not raise
        log_event(evt)


if __name__ == "__main__":
    unittest.main()
