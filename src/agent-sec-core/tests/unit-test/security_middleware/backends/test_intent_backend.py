"""Unit tests for security_middleware.backends.intent — IntentBackend (stub)."""

import unittest

from agent_sec_cli.security_middleware.backends.intent import IntentBackend
from agent_sec_cli.security_middleware.context import RequestContext


class TestIntentBackend(unittest.TestCase):
    def test_always_fails(self):
        backend = IntentBackend()
        ctx = RequestContext(action="intent")
        result = backend.execute(ctx)

        self.assertFalse(result.success)
        self.assertIn("not yet implemented", result.error.lower())


if __name__ == "__main__":
    unittest.main()
