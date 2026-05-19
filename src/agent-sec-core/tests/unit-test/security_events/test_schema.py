"""Unit tests for security_events.schema — SecurityEvent dataclass."""

import json
import os
import unittest
import uuid
from datetime import datetime

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

    def test_result_default_succeeded(self):
        evt = SecurityEvent(event_type="t", category="c", details={})
        self.assertEqual(evt.result, "succeeded")

    def test_result_can_be_failed(self):
        evt = SecurityEvent(event_type="t", category="c", details={}, result="failed")
        self.assertEqual(evt.result, "failed")

    def test_trace_id_default_empty(self):
        evt = SecurityEvent(event_type="t", category="c", details={})
        self.assertEqual(evt.trace_id, "")

    def test_session_id_default_none(self):
        evt = SecurityEvent(event_type="t", category="c", details={})
        self.assertIsNone(evt.session_id)

    def test_agent_trace_fields_default_none(self):
        evt = SecurityEvent(event_type="t", category="c", details={})
        self.assertIsNone(evt.run_id)
        self.assertIsNone(evt.call_id)
        self.assertIsNone(evt.tool_call_id)


class TestSecurityEventToDict(unittest.TestCase):
    def test_to_dict_has_all_keys(self):
        evt = SecurityEvent(event_type="t", category="c", details={"a": 1})
        d = evt.to_dict()
        expected_keys = {
            "event_id",
            "event_type",
            "category",
            "result",
            "timestamp",
            "trace_id",
            "pid",
            "uid",
            "session_id",
            "run_id",
            "call_id",
            "tool_call_id",
            "details",
        }
        self.assertEqual(set(d.keys()), expected_keys)

    def test_to_dict_includes_top_level_tracing_fields(self):
        evt = SecurityEvent(
            event_type="code_scan",
            category="code_scan",
            details={},
            trace_id="trace-1",
            session_id="session-1",
            run_id="run-1",
            call_id="call-1",
            tool_call_id="tool-1",
        )

        payload = evt.to_dict()

        self.assertEqual(payload["trace_id"], "trace-1")
        self.assertEqual(payload["session_id"], "session-1")
        self.assertEqual(payload["run_id"], "run-1")
        self.assertEqual(payload["call_id"], "call-1")
        self.assertEqual(payload["tool_call_id"], "tool-1")

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
