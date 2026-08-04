#!/usr/bin/env python3
"""Validate version-bearing metadata used by component-owned raw packaging."""

import argparse
import json
import re
import tomllib
from pathlib import Path

SEMVER_PATTERN = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)

REQUIRED_RUNTIME_DEPENDENCIES = {
    "bubblewrap",
    "gnupg",
    "jq",
    "nodejs",
    "python3",
    "systemd",
}

PYTHON_RUNTIME_FIELDS = {
    "kind": "language-runtime",
    "version": ">=3.11,<3.12",
    "probe": "python3 --version",
    "source": "system",
}

RPM_RESOURCE_ROOTS = {
    "openclaw": "/opt/agent-sec/openclaw-plugin/",
    "hermes": "/opt/agent-sec/hermes-plugin/src/",
    "codex": "/opt/agent-sec/codex-plugin/hooks-plugin/",
    "qoder": "/opt/agent-sec/qoder-plugin/",
    "qwencode": "/opt/agent-sec/qwen-code-extension/",
    "cosh": "{datadir}/extensions/agent-sec-core/",
}


def read_toml(path: Path) -> dict[str, object]:
    """Read a TOML document from ``path``."""
    with path.open("rb") as stream:
        return tomllib.load(stream)


def read_json_version(path: Path) -> str:
    """Read the top-level version string from a JSON manifest."""
    document = json.loads(path.read_text(encoding="utf-8"))
    version = document.get("version")
    if not isinstance(version, str) or not version:
        raise SystemExit(f"ERROR: {path} has no string version")
    return version


def read_hermes_version(path: Path) -> str:
    """Read the simple top-level version field from the Hermes manifest."""
    match = re.search(
        r"(?m)^version:\s*[\"']?([^\"'\s]+)", path.read_text(encoding="utf-8")
    )
    if match is None:
        raise SystemExit(f"ERROR: {path} has no version")
    return match.group(1)


def verify_contract_metadata(contract: dict[str, object], contract_path: Path) -> None:
    """Verify packaging-owned dependencies and RPM adapter resource roots."""
    component = contract.get("component")
    if not isinstance(component, dict):
        raise SystemExit(f"ERROR: {contract_path} has no [component] table")

    dependencies = component.get("dependencies")
    if not isinstance(dependencies, list):
        raise SystemExit(f"ERROR: {contract_path} has no component dependencies")
    dependencies_by_name = {
        dependency.get("name"): dependency
        for dependency in dependencies
        if isinstance(dependency, dict) and isinstance(dependency.get("name"), str)
    }
    missing_dependencies = sorted(
        REQUIRED_RUNTIME_DEPENDENCIES - dependencies_by_name.keys()
    )
    if missing_dependencies:
        raise SystemExit(
            f"ERROR: {contract_path} is missing runtime dependencies: "
            + ", ".join(missing_dependencies)
        )

    python_runtime = dependencies_by_name["python3"]
    for field, expected in PYTHON_RUNTIME_FIELDS.items():
        actual = python_runtime.get(field)
        if actual != expected:
            raise SystemExit(
                f"ERROR: {contract_path} python3 dependency {field} is "
                f"{actual!r}, expected {expected!r}"
            )

    adapters = contract.get("adapters")
    if not isinstance(adapters, list):
        raise SystemExit(f"ERROR: {contract_path} has no adapters")
    adapters_by_framework = {
        adapter.get("framework"): adapter
        for adapter in adapters
        if isinstance(adapter, dict) and isinstance(adapter.get("framework"), str)
    }
    for framework, expected_root in RPM_RESOURCE_ROOTS.items():
        adapter = adapters_by_framework.get(framework)
        if not isinstance(adapter, dict):
            raise SystemExit(
                f"ERROR: {contract_path} has no {framework!r} adapter declaration"
            )
        backends = adapter.get("backends")
        rpm = backends.get("rpm") if isinstance(backends, dict) else None
        actual_root = rpm.get("resource_root") if isinstance(rpm, dict) else None
        if actual_root != expected_root:
            raise SystemExit(
                f"ERROR: {contract_path} {framework!r} RPM resource root is "
                f"{actual_root!r}, expected {expected_root!r}"
            )


def verify_versions(source_root: Path, contract_path: Path) -> str:
    """Verify the raw contract identity and all packaged component versions."""
    contract = read_toml(contract_path)
    component = contract.get("component")
    if not isinstance(component, dict):
        raise SystemExit(f"ERROR: {contract_path} has no [component] table")
    if component.get("name") != "sec-core":
        raise SystemExit(
            f"ERROR: {contract_path} component name is {component.get('name')!r}, "
            "expected 'sec-core'"
        )
    expected = component.get("version")
    if not isinstance(expected, str) or SEMVER_PATTERN.fullmatch(expected) is None:
        raise SystemExit(
            f"ERROR: {contract_path} component version is not valid SemVer: {expected!r}"
        )
    verify_contract_metadata(contract, contract_path)

    project = read_toml(source_root / "agent-sec-cli" / "pyproject.toml").get("project")
    if not isinstance(project, dict) or not isinstance(project.get("version"), str):
        raise SystemExit("ERROR: agent-sec-cli/pyproject.toml has no project version")

    versions = {
        "agent-sec-cli/pyproject.toml": project["version"],
        "openclaw-plugin/openclaw.plugin.json": read_json_version(
            source_root / "openclaw-plugin" / "openclaw.plugin.json"
        ),
        "openclaw-plugin/package.json": read_json_version(
            source_root / "openclaw-plugin" / "package.json"
        ),
        "hermes-plugin/src/plugin.yaml": read_hermes_version(
            source_root / "hermes-plugin" / "src" / "plugin.yaml"
        ),
        "codex-plugin/hooks-plugin/.codex-plugin/plugin.json": read_json_version(
            source_root
            / "codex-plugin"
            / "hooks-plugin"
            / ".codex-plugin"
            / "plugin.json"
        ),
        "qoder-plugin/.qoder-plugin/plugin.json": read_json_version(
            source_root / "qoder-plugin" / ".qoder-plugin" / "plugin.json"
        ),
        "qwen-code-extension/qwen-extension.json": read_json_version(
            source_root / "qwen-code-extension" / "qwen-extension.json"
        ),
        "cosh-extension/cosh-extension.json": read_json_version(
            source_root / "cosh-extension" / "cosh-extension.json"
        ),
    }
    mismatches = [
        f"{path}={version}" for path, version in versions.items() if version != expected
    ]
    if mismatches:
        raise SystemExit(
            f"ERROR: agent-sec-core release metadata does not match {expected}: "
            + ", ".join(mismatches)
        )
    return expected


def parse_args() -> argparse.Namespace:
    """Parse command-line arguments."""
    parser = argparse.ArgumentParser()
    parser.add_argument("source_root", type=Path)
    parser.add_argument("contract", type=Path)
    return parser.parse_args()


def main() -> int:
    """Validate release metadata and print the canonical version."""
    args = parse_args()
    print(verify_versions(args.source_root.resolve(), args.contract.resolve()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
