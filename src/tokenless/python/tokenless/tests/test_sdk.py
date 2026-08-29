"""Installed-wheel tests for the complete Tokenless lifecycle SDK."""

from __future__ import annotations

import json
import os
import re
import shlex
import tempfile
import unittest
from pathlib import Path
from unittest.mock import AsyncMock, patch

from anolisa_tokenless import (
    Attribution,
    ModelRequest,
    RetrievalError,
    RetrieveRequest,
    TokenlessConfig,
    TokenlessSdk,
    ToolCall,
    ToolResult,
    ToolStatus,
)


class TokenlessSdkTests(unittest.IsolatedAsyncioTestCase):
    """Exercise all four lifecycle boundaries against the native runtime."""

    def setUp(self) -> None:
        self.temporary_directory = tempfile.TemporaryDirectory(
            prefix="tokenless-sdk-test-"
        )
        self.attribution = Attribution("sdk-test", "session-a")

    def tearDown(self) -> None:
        self.temporary_directory.cleanup()

    def sdk(self, **overrides: object) -> TokenlessSdk:
        return TokenlessSdk(
            TokenlessConfig(
                data_dir=Path(self.temporary_directory.name),
                min_chars=0,
                **overrides,
            )
        )

    async def test_before_model_compresses_schema_and_scopes_retrieve(self) -> None:
        sdk = self.sdk(rtk_enabled=False)
        description = "SCHEMA_SENTINEL " + "details " * 500
        tool = {
            "type": "function",
            "function": {
                "name": "lookup",
                "description": description,
                "parameters": {"type": "object", "properties": {}},
            },
        }
        builtin = {"type": "web_search"}
        request = ModelRequest((tool, builtin), "", self.attribution)
        result = await sdk.before_model(request)

        self.assertEqual(tool["function"]["description"], description)
        self.assertIsNot(result.tools[0], tool)
        self.assertEqual(result.tools[1], builtin)
        marker = re.search(
            r"<<tokenless:([0-9a-f]{24})>>",
            result.tools[0]["function"]["description"],
        )
        self.assertIsNotNone(marker)
        assert marker is not None
        self.assertIn(marker.group(1), result.visible_markers)
        self.assertEqual(result.tools[-1]["function"]["name"], "tokenless_retrieve")

        recovered = await sdk.retrieve(
            RetrieveRequest(
                marker.group(1).upper(),
                result.visible_markers,
                self.attribution,
            )
        )
        self.assertEqual(recovered, description)
        with self.assertRaisesRegex(RetrievalError, "not visible"):
            await sdk.retrieve(
                RetrieveRequest(marker.group(1), frozenset(), self.attribution)
            )

    async def test_before_model_preserves_unknown_tools_and_hides_retrieve(
        self,
    ) -> None:
        sdk = self.sdk(rtk_enabled=False)
        request = ModelRequest(
            (
                {"type": "web_search"},
                sdk.retrieve_schema(),
            ),
            "",
            self.attribution,
            frozenset({"0123456789abcdef01234567"}),
        )
        result = await sdk.before_model(request)
        self.assertEqual(result.tools, ({"type": "web_search"},))
        self.assertEqual(result.visible_markers, frozenset())

    async def test_before_model_rejects_retrieve_name_collisions(self) -> None:
        sdk = self.sdk(rtk_enabled=False)
        conflicting = sdk.retrieve_schema()
        conflicting["function"]["description"] = "Application-owned tool"
        with self.assertRaisesRegex(ValueError, "reserved"):
            await sdk.before_model(ModelRequest((conflicting,), "", self.attribution))

        schema = sdk.retrieve_schema()
        with self.assertRaisesRegex(ValueError, "reserved"):
            await sdk.before_model(ModelRequest((schema, schema), "", self.attribution))

    def test_packaged_rtk_requires_a_stable_filesystem_resource(self) -> None:
        with patch("anolisa_tokenless.sdk.files") as package_files:
            package_files.return_value.joinpath.return_value = object()
            with self.assertRaisesRegex(RuntimeError, "unpacked wheel"):
                self.sdk()

    async def test_packaged_rtk_rewrites_a_copied_call_with_attribution(self) -> None:
        sdk = self.sdk()
        original_arguments = {"command": "grep needle file.txt", "other": [1]}
        call = ToolCall(
            "shell",
            original_arguments,
            Attribution("sdk-agent", "sdk-session", "call-7"),
            command_field="command",
        )
        result = await sdk.before_tool_call(call)
        self.assertTrue(result.rewritten)
        self.assertEqual(original_arguments["command"], "grep needle file.txt")
        self.assertIn(str(sdk._rtk_path), result.arguments["command"])
        self.assertIn("TOKENLESS_AGENT_ID=sdk-agent", result.arguments["command"])
        self.assertIn("TOKENLESS_SESSION_ID=sdk-session", result.arguments["command"])
        self.assertIn("TOKENLESS_TOOL_USE_ID=call-7", result.arguments["command"])

        composed = await sdk.before_tool_call(
            ToolCall(
                "shell",
                {"command": "git status\ncargo test"},
                Attribution("sdk-agent", "sdk-session", "call-composed"),
                command_field="command",
            )
        )
        self.assertTrue(composed.rewritten)
        self.assertIn("\ncargo test", composed.arguments["command"])

    async def test_rtk_rewrite_state_bypasses_result_optimization(self) -> None:
        sdk = self.sdk()
        rewritten = await sdk.before_tool_call(
            ToolCall(
                "shell",
                {"command": "grep needle file.txt"},
                Attribution("sdk-agent", "sdk-session", "call-rtk"),
                command_field="command",
            )
        )
        self.assertTrue(rewritten.rewritten)

        sdk._response.compress_text = AsyncMock(
            side_effect=AssertionError("RTK output reached response compression")
        )
        sdk._compress_toon = AsyncMock(
            side_effect=AssertionError("RTK output reached TOON compression")
        )
        result = ToolResult(
            rewritten,
            json.dumps({"items": list(range(100))}),
            ToolStatus.SUCCESS,
        )

        self.assertIs(await sdk.after_tool_call(result), result)
        sdk._response.compress_text.assert_not_awaited()
        sdk._compress_toon.assert_not_awaited()

    def test_rtk_anchoring_preserves_shell_separators_and_quotes(self) -> None:
        sdk = self.sdk()
        attribution = Attribution("sdk-agent", "sdk-session", "call-anchor")
        prefix = " ".join(
            ["env"]
            + [
                f"{name}={shlex.quote(value)}"
                for name, value in sdk._attribution_env(attribution).items()
            ]
            + [shlex.quote(os.fspath(sdk._rtk_path))]
        )
        source = (
            "rtk one\targ\n"
            "plain&&rtk two; rtk three|rtk four\n"
            "plain&&\N{NO-BREAK SPACE}rtk unicode-space\n"
            "printf '%s' '; rtk quoted' # && rtk comment"
        )
        self.assertEqual(
            sdk._anchor_rtk(source, attribution),
            f"{prefix} one\targ\n"
            f"plain&&{prefix} two; {prefix} three|{prefix} four\n"
            "plain&&\N{NO-BREAK SPACE}rtk unicode-space\n"
            "printf '%s' '; rtk quoted' # && rtk comment",
        )

    async def test_after_tool_status_policy(self) -> None:
        sdk = self.sdk(rtk_enabled=False)
        call = ToolCall(
            "api",
            {},
            Attribution("sdk-agent", "sdk-session", "call-8"),
        )
        error = await sdk.after_tool_call(
            ToolResult(call, "/bin/sh: jq: command not found", ToolStatus.ERROR)
        )
        self.assertIn("ENV_DEPENDENCY_MISSING", error.additional_context or "")
        self.assertIn("Skip retry", error.additional_context or "")
        interrupted = ToolResult(call, "partial", ToolStatus.INTERRUPTED)
        self.assertIs(await sdk.after_tool_call(interrupted), interrupted)

        rewritten = ToolResult(
            ToolCall(call.name, call.arguments, call.attribution, rewritten=True),
            json.dumps({"items": list(range(100))}),
            ToolStatus.SUCCESS,
        )
        self.assertIs(await sdk.after_tool_call(rewritten), rewritten)

        retrieved = ToolResult(
            ToolCall(
                sdk.config.retrieve_tool_name,
                {},
                Attribution("sdk-agent", "sdk-session", "call-retrieve"),
            ),
            json.dumps({"payload": list(range(100))}),
            ToolStatus.SUCCESS,
        )
        self.assertIs(await sdk.after_tool_call(retrieved), retrieved)

    async def test_success_uses_a_strictly_smaller_candidate(self) -> None:
        sdk = self.sdk(rtk_enabled=False, mode="aggressive")
        call = ToolCall(
            "api",
            {},
            Attribution("sdk-agent", "sdk-session", "call-9"),
        )
        original = json.dumps(
            {"items": [{"name": "same", "value": index} for index in range(300)]}
        )
        result = await sdk.after_tool_call(
            ToolResult(call, original, ToolStatus.SUCCESS)
        )
        self.assertTrue(result.transformed)
        self.assertLess(len(result.content.encode()), len(original.encode()))

    async def test_excluded_tool_bypasses_all_result_optimization(self) -> None:
        sdk = self.sdk(rtk_enabled=False, excluded_tools={"api"})
        sdk._compress_toon = AsyncMock(return_value="changed")
        result = ToolResult(
            ToolCall(
                "api",
                {},
                Attribution("sdk-agent", "sdk-session", "call-excluded"),
            ),
            json.dumps({"items": list(range(100))}),
            ToolStatus.SUCCESS,
        )

        self.assertIs(await sdk.after_tool_call(result), result)
        sdk._compress_toon.assert_not_awaited()

    def test_stats_client_is_lazy_and_uses_runtime_data_dir(self) -> None:
        sdk = self.sdk(rtk_enabled=False)
        self.assertIsNone(sdk._stats)

        stats = sdk.stats
        self.assertIs(stats, sdk.stats)
        self.assertEqual(stats.status.data_dir, sdk.runtime.data_dir)


if __name__ == "__main__":
    unittest.main()
