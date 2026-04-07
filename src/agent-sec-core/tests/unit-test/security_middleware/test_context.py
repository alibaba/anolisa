"""Unit tests for security_middleware.context — RequestContext dataclass."""

import unittest
import uuid
from datetime import datetime

from agent_sec_cli.security_middleware.context import RequestContext


class TestRequestContext(unittest.TestCase):
    def test_auto_trace_id_is_valid_uuid(self):
        ctx = RequestContext(action="test")
        # Should not raise
        uuid.UUID(ctx.trace_id)

    def test_auto_timestamp_is_parseable(self):
        ctx = RequestContext(action="test")
        # ISO-8601 must be parseable
        dt = datetime.fromisoformat(ctx.timestamp)
        self.assertIsInstance(dt, datetime)

    def test_explicit_trace_id_preserved(self):
        ctx = RequestContext(action="test", trace_id="my-trace")
        self.assertEqual(ctx.trace_id, "my-trace")

    def test_explicit_timestamp_preserved(self):
        ctx = RequestContext(action="test", timestamp="2025-01-01T00:00:00+00:00")
        self.assertEqual(ctx.timestamp, "2025-01-01T00:00:00+00:00")

    def test_caller_defaults_to_empty(self):
        ctx = RequestContext(action="test")
        self.assertEqual(ctx.caller, "")

    def test_session_id_defaults_to_none(self):
        ctx = RequestContext(action="test")
        self.assertIsNone(ctx.session_id)

    def test_two_contexts_get_different_trace_ids(self):
        ctx1 = RequestContext(action="a")
        ctx2 = RequestContext(action="b")
        self.assertNotEqual(ctx1.trace_id, ctx2.trace_id)


if __name__ == "__main__":
    unittest.main()
