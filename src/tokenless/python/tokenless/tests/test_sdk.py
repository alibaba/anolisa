"""Installed-wheel tests for the four Tokenless lifecycle SDK operations."""

from __future__ import annotations

import json
import re
import tempfile
import unittest
from dataclasses import fields
from pathlib import Path
from unittest.mock import patch

from anolisa_tokenless import (
    Attribution,
    BeforeModelCapabilities,
    BeforeModelRequest,
    ContentOrigin,
    OutputOptimization,
    PostToolCapabilities,
    PostToolRequest,
    PreToolAction,
    PreToolCapabilities,
    PreToolRequest,
    ResultKind,
    RetrieveRequest,
    TokenlessConfig,
    TokenlessError,
    TokenlessSdk,
    ToolResultStatus,
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
        request = BeforeModelRequest(
            tools=(tool, {"type": "web_search"}),
            visible_context="",
            retrieve_tool_name="tokenless_retrieve",
            capabilities=BeforeModelCapabilities(
                replace_tools=True,
                publish_retrieve_tool=True,
            ),
            attribution=self.attribution,
        )
        result = await sdk.before_model(request)

        self.assertEqual(tool["function"]["description"], description)
        self.assertEqual(result.tools[1], {"type": "web_search"})
        marker = re.search(
            r"<<tokenless:([0-9a-f]{24})>>",
            result.tools[0]["function"]["description"],
        )
        self.assertIsNotNone(marker)
        assert marker is not None
        self.assertIn(marker.group(1), result.visible_markers)
        self.assertIsNotNone(result.retrieve_tool)
        assert result.retrieve_tool is not None
        self.assertEqual(result.retrieve_tool.name, "tokenless_retrieve")

        recovered = await sdk.retrieve(
            RetrieveRequest(
                marker.group(1).upper(),
                result.visible_markers,
                self.attribution,
            )
        )
        self.assertEqual(recovered.payload, description)
        with self.assertRaisesRegex(TokenlessError, "not authorized"):
            await sdk.retrieve(
                RetrieveRequest(marker.group(1), frozenset(), self.attribution)
            )

    def test_retrieve_declaration_is_owned_by_core(self) -> None:
        sdk = self.sdk(rtk_enabled=False)
        declaration = sdk.retrieve_tool_declaration()
        self.assertEqual(declaration.name, "tokenless_retrieve")
        self.assertEqual(declaration.input_schema["required"], ["hash_or_marker"])
        self.assertEqual(
            declaration.as_function_tool()["function"]["parameters"],
            declaration.input_schema,
        )

    def test_config_contains_only_runtime_resources(self) -> None:
        with self.assertRaisesRegex(ValueError, "absolute path"):
            TokenlessConfig(data_dir="relative")
        self.assertEqual(
            {field.name for field in fields(TokenlessConfig)},
            {"data_dir", "retrieve_tool_name", "rtk_enabled"},
        )

    def test_packaged_rtk_requires_a_stable_filesystem_resource(self) -> None:
        with patch("anolisa_tokenless.sdk.files") as package_files:
            package_files.return_value.joinpath.return_value = object()
            with self.assertRaisesRegex(RuntimeError, "unpacked wheel"):
                self.sdk()

    async def test_pre_tool_uses_core_rewrite_and_preserves_input(self) -> None:
        sdk = self.sdk()
        original_arguments = {"command": "grep needle file.txt", "other": [1]}
        result = await sdk.pre_tool(
            PreToolRequest(
                tool_name="shell",
                arguments=original_arguments,
                command_field="command",
                capabilities=PreToolCapabilities(
                    replace_arguments=True,
                    block_and_suggest=False,
                ),
                attribution=Attribution("sdk-agent", "sdk-session", "call-7"),
            )
        )
        self.assertEqual(result.action, PreToolAction.REPLACE_ARGUMENTS)
        self.assertEqual(result.output_optimization, OutputOptimization.RTK)
        self.assertEqual(original_arguments["command"], "grep needle file.txt")
        self.assertIn(str(sdk._rtk_path), result.arguments["command"])
        self.assertIn("TOKENLESS_AGENT_ID=sdk-agent", result.arguments["command"])
        self.assertIn("TOKENLESS_SESSION_ID=sdk-session", result.arguments["command"])
        self.assertIn("TOKENLESS_TOOL_USE_ID=call-7", result.arguments["command"])
        self.assertIn(
            f"TOKENLESS_DATA_DIR={self.temporary_directory.name}",
            result.arguments["command"],
        )

    def test_post_tool_tool_kind_requires_call_identity_for_wire_strings(self) -> None:
        with self.assertRaisesRegex(ValueError, "tool_use_id"):
            PostToolRequest(
                result_kind="tool",  # type: ignore[arg-type]
                tool_name="api",
                content="result",
                status=ToolResultStatus.SUCCESS,
                content_origin=ContentOrigin.API_RESPONSE,
                output_optimization=OutputOptimization.NONE,
                capabilities=PostToolCapabilities(True, True, True),
                attribution=self.attribution,
            )

    async def test_post_tool_routes_rtk_error_and_retrieve_in_core(self) -> None:
        sdk = self.sdk(rtk_enabled=False)
        capabilities = PostToolCapabilities(
            replace_output=True,
            publish_retrieve_tool=True,
            replace_with_text=True,
        )
        attribution = Attribution("sdk-agent", "sdk-session", "call-8")

        optimized_content = json.dumps({"items": list(range(100))})
        optimized = await sdk.post_tool(
            PostToolRequest(
                result_kind=ResultKind.TOOL,
                tool_name="shell",
                content=optimized_content,
                status=ToolResultStatus.SUCCESS,
                content_origin=ContentOrigin.COMMAND_OUTPUT,
                output_optimization=OutputOptimization.RTK,
                capabilities=capabilities,
                attribution=attribution,
            )
        )
        self.assertEqual(optimized.output, optimized_content)
        self.assertEqual(optimized.applied_operations, ())

        error = await sdk.post_tool(
            PostToolRequest(
                result_kind=ResultKind.TOOL,
                tool_name="shell",
                content="/bin/sh: jq: command not found",
                status=ToolResultStatus.ERROR,
                content_origin=ContentOrigin.COMMAND_OUTPUT,
                output_optimization=OutputOptimization.NONE,
                capabilities=capabilities,
                attribution=attribution,
            )
        )
        self.assertIn("ENV_DEPENDENCY_MISSING", error.additional_context or "")

        retrieved = await sdk.post_tool(
            PostToolRequest(
                result_kind=ResultKind.RETRIEVE,
                tool_name="tokenless_retrieve",
                content="restored payload",
                status=ToolResultStatus.SUCCESS,
                content_origin=ContentOrigin.API_RESPONSE,
                output_optimization=OutputOptimization.NONE,
                capabilities=capabilities,
                attribution=self.attribution,
            )
        )
        self.assertEqual(retrieved.output, "restored payload")
        self.assertEqual(retrieved.applied_operations, ())

    async def test_post_tool_uses_core_json_pipeline(self) -> None:
        sdk = self.sdk(rtk_enabled=False)
        original = json.dumps(
            {"items": [{"name": "same", "value": index} for index in range(300)]}
        )
        result = await sdk.post_tool(
            PostToolRequest(
                result_kind=ResultKind.TOOL,
                tool_name="api",
                content=original,
                status=ToolResultStatus.SUCCESS,
                content_origin=ContentOrigin.API_RESPONSE,
                output_optimization=OutputOptimization.NONE,
                capabilities=PostToolCapabilities(
                    replace_output=True,
                    publish_retrieve_tool=True,
                    replace_with_text=True,
                ),
                attribution=Attribution("sdk-agent", "sdk-session", "call-9"),
            )
        )
        self.assertLess(len(result.output.encode()), len(original.encode()))
        self.assertTrue(result.applied_operations)

    def test_stats_client_is_lazy_and_uses_runtime_data_dir(self) -> None:
        sdk = self.sdk(rtk_enabled=False)
        self.assertIsNone(sdk._stats)

        stats = sdk.stats
        self.assertIs(stats, sdk.stats)
        self.assertEqual(stats.status.data_dir, sdk.runtime.data_dir)


if __name__ == "__main__":
    unittest.main()
