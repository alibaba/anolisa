"""Installed-wheel tests for the native Tokenless Python runtime."""

from __future__ import annotations

import asyncio
import json
import re
import sqlite3
import tempfile
import unittest
from concurrent.futures import ThreadPoolExecutor
from importlib.metadata import distribution
from pathlib import Path

from anolisa_tokenless import (
    RetrievalError,
    TokenlessConfig,
    TokenlessError,
    TokenlessRuntime,
    ToolResponseCompressor,
    __version__,
)


class TokenlessRuntimeTests(unittest.TestCase):
    """Exercise public Python API behavior against real SQLite state."""

    def setUp(self) -> None:
        self.temporary_directory = tempfile.TemporaryDirectory(
            prefix="tokenless-python-test-"
        )
        self.runtime = TokenlessRuntime(
            self.temporary_directory.name,
            stats_enabled=True,
        )

    def tearDown(self) -> None:
        del self.runtime
        self.temporary_directory.cleanup()

    @staticmethod
    def long_response(sentinel: str = "ORCHID-7291") -> str:
        return json.dumps(
            {
                "items": [f"{sentinel}-record-{index:04d}" for index in range(200)],
                "tail": f"RECOVERY_SENTINEL={sentinel}\n",
            },
            ensure_ascii=False,
        )

    def test_version_matches_component(self) -> None:
        self.assertRegex(__version__, r"^\d+\.\d+\.\d+$")

    def test_distribution_contains_license_and_documentation_link(self) -> None:
        package = distribution("anolisa-tokenless")
        self.assertEqual(package.metadata.get_all("License-File"), ["LICENSE"])

        license_paths = [
            path
            for path in package.files or ()
            if str(path).endswith(".dist-info/licenses/LICENSE")
        ]
        self.assertEqual(len(license_paths), 1)
        self.assertIn("Apache License", Path(license_paths[0].locate()).read_text())

        metadata = package.read_text("METADATA")
        self.assertIsNotNone(metadata)
        assert metadata is not None
        self.assertIn(
            "https://github.com/alibaba/anolisa/blob/main/src/tokenless/README.md",
            metadata,
        )
        self.assertNotIn("../../README.md", metadata)

    def test_compress_and_retrieve_byte_exact(self) -> None:
        payload = "RECOVERY_SENTINEL=ORCHID-7291\n" + ("世界" * 200)
        original = json.dumps({"tail": payload}, ensure_ascii=False)
        result = self.runtime.compress_response(
            original,
            truncate_strings_at=96,
            max_depth=8,
            agent_id="python-test",
            session_id="session-a",
            tool_use_id="tool-a",
        )
        self.assertTrue(result.applied)
        self.assertLess(len(result.output.encode()), len(original.encode()))
        marker = re.search(r"<<tokenless:([0-9a-f]{24})>>", result.output)
        self.assertIsNotNone(marker)
        assert marker is not None
        recovered = self.runtime.retrieve(marker.group(1).upper())
        self.assertEqual(recovered, payload)

    def test_framework_core_compresses_and_authorizes_visible_marker(self) -> None:
        payload = "RECOVERY_SENTINEL=FRAMEWORK\n" + ("世界" * 3_000)
        original = json.dumps({"payload": payload}, ensure_ascii=False)
        compressor = ToolResponseCompressor(
            TokenlessConfig(
                mode="aggressive",
                data_dir=Path(self.temporary_directory.name, "framework"),
                min_chars=0,
            ),
        )
        compressed = asyncio.run(
            compressor.compress_text(
                original,
                tool_name="api_call",
                agent_id="framework-test",
                session_id="session",
                tool_use_id="tool",
            ),
        )
        self.assertIsNotNone(compressed)
        assert compressed is not None
        marker = re.search(r"<<tokenless:([0-9a-f]{24})>>", compressed)
        self.assertIsNotNone(marker)
        assert marker is not None

        with self.assertRaisesRegex(RetrievalError, "not present"):
            asyncio.run(compressor.retrieve(marker.group(1), "no visible marker"))
        recovered = asyncio.run(
            compressor.retrieve(marker.group(1).upper(), compressed)
        )
        self.assertEqual(recovered, payload)

    def test_framework_core_treats_oversized_integer_as_text(self) -> None:
        compressor = ToolResponseCompressor(
            TokenlessConfig(
                data_dir=Path(self.temporary_directory.name, "oversized-integer"),
                min_chars=0,
            ),
        )
        compressed = asyncio.run(
            compressor.compress_text(
                "9" * 4_301,
                tool_name="api_call",
                agent_id="framework-test",
                session_id="session",
                tool_use_id="tool",
            ),
        )
        self.assertIsNone(compressed)

    def test_framework_core_treats_deep_json_as_text(self) -> None:
        compressor = ToolResponseCompressor(
            TokenlessConfig(
                data_dir=Path(self.temporary_directory.name, "deep-json"),
                min_chars=0,
            ),
        )
        compressed = asyncio.run(
            compressor.compress_text(
                "[" * 10_000 + "]" * 10_000,
                tool_name="api_call",
                agent_id="framework-test",
                session_id="session",
                tool_use_id="tool",
            ),
        )
        self.assertIsNone(compressed)

    def test_framework_config_enforces_common_policy(self) -> None:
        compressor = ToolResponseCompressor(TokenlessConfig(mode="balanced"))
        self.assertTrue(compressor.is_excluded("Read"))
        self.assertFalse(compressor.is_excluded("api_call"))
        self.assertEqual(compressor.thresholds_for("Bash"), (65_536, 128, 8))
        with self.assertRaisesRegex(ValueError, "absolute path"):
            TokenlessConfig(data_dir="relative")

    def test_invalid_json_raises_package_error(self) -> None:
        with self.assertRaisesRegex(TokenlessError, "JSON parse error"):
            self.runtime.compress_response("not-json")

    def test_missing_hash_raises_package_error(self) -> None:
        with self.assertRaisesRegex(TokenlessError, "no stashed payload"):
            self.runtime.retrieve("000000000000000000000000")

    def test_malformed_hash_raises_package_error(self) -> None:
        with self.assertRaisesRegex(TokenlessError, "invalid stash hash"):
            self.runtime.retrieve("not-a-hash")

    def test_stash_initialization_failure_is_reversible_fail_open(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="tokenless-python-broken-stash-"
        ) as directory:
            Path(directory, "stash.db").write_bytes(b"not a sqlite database")
            runtime = TokenlessRuntime(directory, stats_enabled=False)
            self.assertFalse(runtime.stash_available)
            self.assertIsNotNone(runtime.stash_error)

            original = self.long_response()
            result = runtime.compress_response(
                original,
                truncate_arrays_at=2,
                agent_id="python-test",
            )
            self.assertEqual(result.disposition, "reversibility-unavailable")
            self.assertEqual(result.output, original)

    def test_short_string_limit_is_reversible_fail_open(self) -> None:
        original = json.dumps("x" * 400)
        result = self.runtime.compress_response(original, truncate_strings_at=10)

        self.assertEqual(result.disposition, "reversibility-unavailable")
        self.assertEqual(result.output, original)
        self.assertEqual(result.stash_errors, 0)
        self.assertEqual(result.unrecoverable_truncations, 1)

    def test_string_stash_write_failure_is_reversible_fail_open(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="tokenless-python-string-failure-"
        ) as directory:
            runtime = TokenlessRuntime(directory, stats_enabled=False)
            with sqlite3.connect(Path(directory, "stash.db")) as connection:
                connection.execute("DROP TABLE stash")

            original = json.dumps("x" * 400)
            result = runtime.compress_response(original, truncate_strings_at=80)
            self.assertEqual(result.disposition, "reversibility-unavailable")
            self.assertEqual(result.output, original)
            self.assertEqual(result.stash_errors, 1)
            self.assertEqual(result.unrecoverable_truncations, 1)

    def test_depth_stash_write_failure_is_reversible_fail_open(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="tokenless-python-depth-failure-"
        ) as directory:
            runtime = TokenlessRuntime(directory, stats_enabled=False)
            with sqlite3.connect(Path(directory, "stash.db")) as connection:
                connection.execute("DROP TABLE stash")

            original = json.dumps({"nested": {"payload": "x" * 400}})
            result = runtime.compress_response(original, max_depth=0)
            self.assertEqual(result.disposition, "reversibility-unavailable")
            self.assertEqual(result.output, original)
            self.assertEqual(result.stash_errors, 1)
            self.assertEqual(result.unrecoverable_truncations, 1)

    def test_parallel_calls_do_not_cross_attribution_or_state(self) -> None:
        def compress(index: int) -> str:
            result = self.runtime.compress_response(
                self.long_response(f"SENTINEL-{index}"),
                truncate_arrays_at=2,
                agent_id="python-test",
                session_id=f"session-{index}",
                tool_use_id=f"tool-{index}",
            )
            match = re.search(r"<<tokenless:([0-9a-f]{24})>>", result.output)
            self.assertIsNotNone(match)
            assert match is not None
            return self.runtime.retrieve(match.group(1))

        with ThreadPoolExecutor(max_workers=8) as executor:
            recovered = list(executor.map(compress, range(16)))
        for index, payload in enumerate(recovered):
            self.assertIn(f"SENTINEL-{index}-record-0002", payload)
            self.assertIn(f"SENTINEL-{index}-record-0199", payload)

        with sqlite3.connect(f"{self.temporary_directory.name}/stats.db") as connection:
            rows = connection.execute(
                "SELECT session_id, tool_use_id FROM stats WHERE agent_id = 'python-test'"
            ).fetchall()
        self.assertEqual(len(rows), 16)
        self.assertEqual(
            set(rows),
            {(f"session-{index}", f"tool-{index}") for index in range(16)},
        )


if __name__ == "__main__":
    unittest.main()
