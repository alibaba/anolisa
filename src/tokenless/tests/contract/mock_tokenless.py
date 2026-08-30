#!/usr/bin/env python3
"""Mock ``tokenless`` Protocol v2 transport for the hook contract suite.

The behavior is selected by ``TOKENLESS_MOCK_BEHAVIOR``:

  applied           return a deterministic transformed result
  no_savings        return the original result with a no-savings disposition
  passthrough       return the original result with a passthrough disposition
  error_disposition return a well-formed result the hook must not apply
  timeout           sleep past the hook timeout
  nonzero_exit      exit 1 without output
  malformed_stdout  print non-JSON output

Every invocation appends its argv to ``TOKENLESS_MOCK_LOG`` when set. The
mock also validates the v2 envelope and operation payload used by each hook.
"""

import json
import os
import sys
import time


def truncate_strings(value):
    if isinstance(value, str):
        return value[:20] if len(value) > 20 else value
    if isinstance(value, list):
        return [truncate_strings(item) for item in value]
    if isinstance(value, dict):
        return {key: truncate_strings(item) for key, item in value.items()}
    return value


def envelope(request: dict, result: dict) -> None:
    print(
        json.dumps(
            {
                "protocol_version": 2,
                "operation": request["operation"],
                "attribution": request["attribution"],
                "result": result,
            }
        )
    )


def respond_before_model(request: dict, behavior: str) -> int:
    input_data = request.get("input")
    if (
        not isinstance(input_data, dict)
        or not isinstance(input_data.get("tools"), list)
        or "visible_context" not in input_data
        or not isinstance(input_data.get("capabilities"), dict)
    ):
        return 3

    if behavior == "error_disposition":
        envelope(request, {})
        return 0
    tools = input_data["tools"]
    if behavior == "applied":
        tools = truncate_strings(tools)
    elif behavior not in {"no_savings", "passthrough"}:
        return 4
    envelope(
        request,
        {"tools": tools, "visible_markers": [], "retrieve_tool": None},
    )
    return 0


def respond_post_tool(request: dict, behavior: str) -> int:
    input_data = request.get("input")
    if (
        not isinstance(input_data, dict)
        or not isinstance(input_data.get("content"), str)
        or not isinstance(input_data.get("capabilities"), dict)
        or "content_origin" not in input_data
        or "output_optimization" not in input_data
    ):
        return 3

    content = input_data["content"]
    disposition = behavior
    output = content
    operations = []
    before_tokens = 100
    after_tokens = 100
    can_replace = input_data["capabilities"].get("replace_output") is True
    if behavior == "applied" and can_replace:
        output = json.dumps(
            truncate_strings(json.loads(content)),
            separators=(",", ":"),
            ensure_ascii=False,
        )
        disposition = "applied"
        operations = ["json_truncation"]
        after_tokens = 50
    elif behavior == "applied" or behavior == "passthrough":
        disposition = "passthrough"
    elif behavior == "no_savings":
        disposition = "no_savings"
    elif behavior == "error_disposition":
        disposition = "tool_error"
    else:
        return 4

    envelope(
        request,
        {
            "output": output,
            "disposition": disposition,
            "content_type": "json",
            "applied_operations": operations,
            "recoverability": "lossless",
            "before_tokens": before_tokens,
            "after_tokens": after_tokens,
            "stash_keys": [],
        },
    )
    return 0


def main() -> int:
    log_path = os.environ.get("TOKENLESS_MOCK_LOG")
    if log_path:
        with open(log_path, "a") as log:
            log.write(" ".join(sys.argv[1:]) + "\n")

    behavior = os.environ.get("TOKENLESS_MOCK_BEHAVIOR", "applied")
    if sys.argv[1:] != ["compress"]:
        return 2
    raw = sys.stdin.read()

    if behavior == "timeout":
        time.sleep(60)
        return 0
    if behavior == "nonzero_exit":
        return 1
    if behavior == "malformed_stdout":
        print("this is not a protocol response")
        return 0

    request = json.loads(raw)
    if (
        request.get("protocol_version") != 2
        or request.get("operation") not in {"before_model", "post_tool"}
        or not isinstance(request.get("attribution"), dict)
        or set(request) != {"protocol_version", "operation", "attribution", "input"}
    ):
        return 3
    if request["operation"] == "before_model":
        return respond_before_model(request, behavior)
    return respond_post_tool(request, behavior)


if __name__ == "__main__":
    sys.exit(main())
