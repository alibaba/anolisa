#!/usr/bin/env python3
"""Verify that qodercli reports the native tokenless plugin and resources."""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any


def _plugins(value: Any) -> list[dict[str, Any]]:
    if isinstance(value, list):
        return [item for item in value if isinstance(item, dict)]
    if isinstance(value, dict):
        for key in ("plugins", "installed", "items"):
            if isinstance(value.get(key), list):
                return [item for item in value[key] if isinstance(item, dict)]
        if "id" in value or "pluginId" in value:
            return [value]
        raise ValueError("expected a plugin array or a recognized plugin object")
    raise ValueError("expected a JSON object or array")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--presence-only",
        action="store_true",
        help="succeed when tokenless@local exists, regardless of health",
    )
    args = parser.parse_args()
    try:
        document = json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError) as exc:
        print(f"qodercli plugins list returned invalid JSON: {exc}", file=sys.stderr)
        return 1

    try:
        plugins = _plugins(document)
    except ValueError as exc:
        print(f"qodercli plugins list returned unsupported JSON: {exc}", file=sys.stderr)
        return 2

    plugin = next(
        (
            item
            for item in plugins
            if item.get("id") == "tokenless@local"
            or item.get("pluginId") == "tokenless@local"
        ),
        None,
    )
    if plugin is None:
        print("qodercli does not report tokenless@local", file=sys.stderr)
        return 1
    if args.presence_only:
        return 0
    if plugin.get("enabled") is not True:
        print("tokenless@local is registered but not enabled", file=sys.stderr)
        return 1

    resources = plugin.get("resources")
    if not isinstance(resources, dict):
        print("tokenless@local has no resource inventory", file=sys.stderr)
        return 1
    hooks = resources.get("hooks")
    commands = resources.get("commands")
    if not isinstance(hooks, list) or not isinstance(commands, list):
        print("tokenless@local has an invalid resource inventory", file=sys.stderr)
        return 1
    events = [
        hook.get("event")
        for hook in hooks
        if isinstance(hook, dict) and isinstance(hook.get("event"), str)
    ]
    command_names = [
        command.get("name")
        for command in commands
        if isinstance(command, dict) and isinstance(command.get("name"), str)
    ]
    pre_count = events.count("PreToolUse")
    post_count = events.count("PostToolUse")
    if pre_count != 2 or post_count != 1:
        print(
            f"tokenless hook inventory is incomplete: PreToolUse={pre_count}, "
            f"PostToolUse={post_count}",
            file=sys.stderr,
        )
        return 1
    if not any(
        name == "tokenless-stats" or name.endswith(":tokenless-stats")
        for name in command_names
    ):
        print("tokenless-stats command was not discovered", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
