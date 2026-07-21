#!/usr/bin/env python3
"""Fail when component metadata drifts from its authoritative version."""

import json
import re
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
VERSION_RE = re.compile(
    r'^\s*(?:"version"|version)\s*[:=]\s*"([^"]+)"\s*,?\s*$', re.MULTILINE
)
TOML_CONTRACTS = (
    ("src/agent-sec-core/openclaw-plugin/package.json", "src/agent-sec-core/adapters/component.toml"),
    ("src/agentsight/Cargo.toml", "src/agentsight/component.toml"),
    ("src/copilot-shell/package.json", "src/copilot-shell/component.toml"),
    ("src/cosh-ng/Cargo.toml", "src/cosh-ng/component.toml"),
    ("src/skillfs/Cargo.toml", "src/skillfs/component.toml"),
    ("src/ws-ckpt/src/Cargo.toml", "src/ws-ckpt/component.toml"),
)
VERSION_TEMPLATES = (
    ("src/agent-memory/Cargo.toml", "src/agent-memory/.anolisa/component.toml.in"),
    ("src/tokenless/Cargo.toml", "src/tokenless/.anolisa/component.toml.in"),
    ("src/tokenless/Cargo.toml", "src/tokenless/adapters/tokenless/manifest.json.in"),
    ("src/tokenless/Cargo.toml", "src/tokenless/adapters/tokenless/openclaw/package.json.in"),
    ("src/tokenless/Cargo.toml", "src/tokenless/adapters/tokenless/openclaw/openclaw.plugin.json.in"),
    ("src/tokenless/Cargo.toml", "src/tokenless/adapters/tokenless/hermes/plugin.yaml.in"),
    ("src/tokenless/Cargo.toml", "src/tokenless/adapters/tokenless/qoder/.qoder-plugin/plugin.json.in"),
    ("src/tokenless/Cargo.toml", "src/tokenless/adapters/tokenless/claude-code/.claude-plugin/plugin.json.in"),
    ("src/tokenless/Cargo.toml", "src/tokenless/adapters/tokenless/codex/.codex-plugin/plugin.json.in"),
    ("src/tokenless/Cargo.toml", "src/tokenless/adapters/tokenless/qwencode/qwen-extension.json.in"),
)
AGENT_MEMORY_JSON = (
    "src/agent-memory/adapters/agent-memory/manifest.json",
    "src/agent-memory/adapters/agent-memory/openclaw/package.json",
    "src/agent-memory/adapters/agent-memory/openclaw/openclaw.plugin.json",
    "src/agent-memory/config/mcp-server.json",
)
GENERATED_CONTRACTS = (
    "src/agent-memory/.anolisa/component.toml",
    "src/tokenless/.anolisa/component.toml",
)


def read_toml_version(path: str) -> str:
    match = VERSION_RE.search((ROOT / path).read_text())
    if not match:
        raise ValueError(f"no version field in {path}")
    return match.group(1)


def read_json_version(path: str) -> str:
    version = json.loads((ROOT / path).read_text()).get("version")
    if not isinstance(version, str):
        raise ValueError(f"no JSON version field in {path}")
    return version


def read_version(path: str) -> str:
    return read_json_version(path) if path.endswith(".json") else read_toml_version(path)


def check_equal(errors: list[str], source: str, target: str) -> None:
    expected = read_version(source)
    actual = read_version(target)
    if actual != expected:
        errors.append(f"{target}: expected {expected}, found {actual} (source: {source})")


def check_template(errors: list[str], source: str, template: str) -> None:
    expected = read_version(source)
    content = (ROOT / template).read_text()
    if content.count("@VERSION@") != 1:
        errors.append(f"{template}: expected exactly one @VERSION@ placeholder")
        return
    rendered = content.replace("@VERSION@", expected)
    match = VERSION_RE.search(rendered)
    if not match or match.group(1) != expected:
        errors.append(f"{template}: does not render component.version={expected}")


def check_agent_memory_lock(errors: list[str], expected: str) -> None:
    path = "src/agent-memory/adapters/agent-memory/openclaw/package-lock.json"
    lock = json.loads((ROOT / path).read_text())
    versions = (lock.get("version"), lock.get("packages", {}).get("", {}).get("version"))
    if versions != (expected, expected):
        errors.append(f"{path}: expected root and package versions {expected}, found {versions}")


def check_generated_contracts_untracked(errors: list[str]) -> None:
    for path in GENERATED_CONTRACTS:
        result = subprocess.run(
            ["git", "ls-files", "--error-unmatch", path],
            cwd=ROOT,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        if result.returncode == 0:
            errors.append(f"{path}: generated component contract must not be tracked")


def main() -> int:
    errors: list[str] = []
    try:
        for source, target in TOML_CONTRACTS:
            check_equal(errors, source, target)
        for source, template in VERSION_TEMPLATES:
            check_template(errors, source, template)

        agent_memory_version = read_toml_version("src/agent-memory/Cargo.toml")
        for path in AGENT_MEMORY_JSON:
            actual = read_json_version(path)
            if actual != agent_memory_version:
                errors.append(f"{path}: expected {agent_memory_version}, found {actual}")
        check_agent_memory_lock(errors, agent_memory_version)
        check_generated_contracts_untracked(errors)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        errors.append(str(error))

    if errors:
        print("Component version check failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print("Component version metadata is synchronized.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
