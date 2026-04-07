"""Unit tests for security_events.schema — SecurityEvent dataclass."""

import json
import os
import unittest
import uuid
from datetime import datetime, timezone

from agent_sec_cli.security_events.schema import SecurityEvent


class TestSecurityEventRequiredFields(unittest.TestCase):
    def test_required_fields_set(self):
        evt = SecurityEvent(
            event_type="test_event",
            category="test_cat",
            details={"key": "value"},
        )
        self.assertEqual(evt.event_type, "test_event")
        self.assertEqual(evt.category, "test_cat")
        self.assertEqual(evt.details, {"key": "value"})


class TestSecurityEventAutoFill(unittest.TestCase):
    def test_event_id_is_uuid(self):
        evt = SecurityEvent(event_type="t", category="c", details={})
        # Should not raise
        uuid.UUID(evt.event_id)

    def test_timestamp_is_iso8601(self):
        evt = SecurityEvent(event_type="t", category="c", details={})
        ts = datetime.fromisoformat(evt.timestamp)
        self.assertIsNotNone(ts.tzinfo)

    def test_pid_matches_current(self):
        evt = SecurityEvent(event_type="t", category="c", details={})
        self.assertEqual(evt.pid, os.getpid())

    def test_uid_matches_current(self):
        evt = SecurityEvent(event_type="t", category="c", details={})
        self.assertEqual(evt.uid, os.getuid())

    def test_trace_id_default_empty(self):
        evt = SecurityEvent(event_type="t", category="c", details={})
        self.assertEqual(evt.trace_id, "")

    def test_session_id_default_none(self):
        evt = SecurityEvent(event_type="t", category="c", details={})
        self.assertIsNone(evt.session_id)


class TestSecurityEventToDict(unittest.TestCase):
    def test_to_dict_has_all_keys(self):
        evt = SecurityEvent(event_type="t", category="c", details={"a": 1})
        d = evt.to_dict()
        expected_keys = {
            "event_id", "event_type", "category", "timestamp",
            "trace_id", "pid", "uid", "session_id", "details",
        }
        self.assertEqual(set(d.keys()), expected_keys)

    def test_to_dict_roundtrip_json(self):
        evt = SecurityEvent(
            event_type="sandbox_block",
            category="sandbox",
            details={"command": "rm -rf /", "reason": "dangerous"},
        )
        s = json.dumps(evt.to_dict())
        parsed = json.loads(s)
        self.assertEqual(parsed["event_type"], "sandbox_block")
        self.assertEqual(parsed["details"]["command"], "rm -rf /")


if __name__ == "__main__":
    unittest.main()
