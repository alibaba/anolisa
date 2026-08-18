#!/usr/bin/env python3
"""Tests for the Hermes adapter TOON minimum payload threshold.

TOON on small JSON saves only a handful of characters (~0.3% below
~500 chars) while the per-event encode cost stays the same, so the
Hermes pipeline must skip the TOON step for payloads under
``_MIN_TOON_CHARS`` (500) without invoking ``tokenless compress-toon``.
Response compression still runs for those payloads; larger payloads
keep flowing to TOON.
"""

import importlib.util
import json
import os
import sys
import unittest

_REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
_PLUGIN_SRC = os.path.join(
    _REPO_ROOT, "adapters", "tokenless", "hermes", "__init__.py"
)

_needs_py39 = sys.version_info < (3, 9)


def _load_plugin(path: str, name: str):
    """Load the Hermes plugin module under a unique name."""
    sys.modules.pop("hook_utils", None)
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    pre_path = sys.path[:]
    try:
        spec.loader.exec_module(module)
    finally:
        sys.path[:] = pre_path
    return module


def _json_payload(char_target: int) -> str:
    """A JSON object string of roughly ``char_target`` characters."""
    inner = "x" * max(char_target - 30, 1)
    return json.dumps({"stdout": inner, "exit_code": 0})


@unittest.skipIf(_needs_py39, "hermes plugin requires Python 3.9+")
class HermesToonMinPayloadThresholdTest(unittest.TestCase):
    """Spy on _encode_toon to prove it is gated by _MIN_TOON_CHARS."""

    @classmethod
    def setUpClass(cls):
        cls.plugin = _load_plugin(_PLUGIN_SRC, "hermes_plugin_toon_threshold")

    def setUp(self):
        self.toon_calls = []

        plugin = self.plugin
        # Make the pipeline believe tokenless is installed and the input
        # is a plain tool result (not a skill file).
        self._orig_have = plugin._have
        self._orig_is_skill = plugin._is_skill_file
        self._orig_compress = plugin._compress_response
        self._orig_encode = plugin._encode_toon
        plugin._have = lambda *a, **k: True
        plugin._is_skill_file = lambda result: False
        # Default: response compression gains nothing.
        plugin._compress_response = lambda *a, **k: None

        def _spy_encode(data, session_id="", tool_call_id=""):
            self.toon_calls.append(data)
            return "toon:" + data[: len(data) // 2], 50

        plugin._encode_toon = _spy_encode

    def tearDown(self):
        plugin = self.plugin
        plugin._have = self._orig_have
        plugin._is_skill_file = self._orig_is_skill
        plugin._compress_response = self._orig_compress
        plugin._encode_toon = self._orig_encode

    def test_small_payload_skips_toon_and_passes_through(self):
        """Under _MIN_TOON_CHARS with no compression savings: no TOON call."""
        result = self.plugin.on_transform_tool_result(
            tool_name="some_api_tool",
            result=_json_payload(300),
            session_id="s1",
            tool_call_id="t1",
        )
        self.assertEqual(self.toon_calls, [],
                         "_encode_toon must not run below the threshold")
        self.assertIsNone(result, "No savings → pass through unchanged")

    def test_small_compressed_result_skips_toon(self):
        """Compressed result under _MIN_TOON_CHARS keeps compressed form."""
        compressed = _json_payload(250)
        self.plugin._compress_response = lambda *a, **k: compressed
        result = self.plugin.on_transform_tool_result(
            tool_name="some_api_tool",
            result=_json_payload(400),
            session_id="s1",
            tool_call_id="t1",
        )
        self.assertEqual(self.toon_calls, [],
                         "_encode_toon must not run below the threshold")
        self.assertEqual(result, compressed,
                         "Payload keeps the response-compressed form")

    def test_large_payload_still_toon_encoded(self):
        """At/above _MIN_TOON_CHARS the TOON step still runs."""
        result = self.plugin.on_transform_tool_result(
            tool_name="some_api_tool",
            result=_json_payload(800),
            session_id="s1",
            tool_call_id="t1",
        )
        self.assertEqual(len(self.toon_calls), 1,
                         "_encode_toon must run for large payloads")
        self.assertTrue(str(result).startswith("toon:"),
                        f"Expected TOON output, got: {result!r}")


if __name__ == "__main__":
    unittest.main(verbosity=2)
