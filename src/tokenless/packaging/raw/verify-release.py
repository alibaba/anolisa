#!/usr/bin/env python3
"""Validate Tokenless source metadata before assembling a raw package."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


WORKSPACE_VERSION = re.compile(
    r"(?ms)^\[workspace\.package\]\s*$.*?^version\s*=\s*\"([^\"]+)\""
)
COMPONENT = re.compile(r"(?ms)^\[component\]\s*$.*?(?=^\[|\Z)")
FIELD = r"(?m)^{}\s*=\s*\"([^\"]+)\""


def match_field(text: str, name: str, path: Path) -> str:
    """Read one string field from a scoped TOML table."""
    match = re.search(FIELD.format(re.escape(name)), text)
    if match is None:
        raise SystemExit(f"ERROR: {path} has no {name} field")
    return match.group(1)


def read_json_version(path: Path) -> str:
    """Read one generated adapter's top-level JSON version."""
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise SystemExit(f"ERROR: cannot read {path}: {error}") from error
    version = document.get("version") if isinstance(document, dict) else None
    if not isinstance(version, str) or not version:
        raise SystemExit(f"ERROR: {path} has no string version")
    return version


def read_hermes_version(path: Path) -> str:
    """Read the generated Hermes manifest's simple version field."""
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise SystemExit(f"ERROR: cannot read {path}: {error}") from error
    match = re.search(r'(?m)^version:\s*["\']?([^"\'\s]+)', text)
    if match is None:
        raise SystemExit(f"ERROR: {path} has no version")
    return match.group(1)


def verify_versions(root: Path, contract: Path) -> str:
    """Return the source version after checking packaged release metadata."""
    cargo_path = root / "Cargo.toml"
    try:
        cargo_text = cargo_path.read_text(encoding="utf-8")
        contract_text = contract.read_text(encoding="utf-8")
    except OSError as error:
        raise SystemExit(f"ERROR: cannot read release metadata: {error}") from error

    cargo_match = WORKSPACE_VERSION.search(cargo_text)
    if cargo_match is None:
        raise SystemExit(f"ERROR: {cargo_path} has no workspace package version")
    expected = cargo_match.group(1)

    component_match = COMPONENT.search(contract_text)
    if component_match is None:
        raise SystemExit(f"ERROR: {contract} has no [component] table")
    component_text = component_match.group(0)
    if match_field(component_text, "name", contract) != "tokenless":
        raise SystemExit(f"ERROR: {contract} is not a tokenless contract")
    contract_version = match_field(component_text, "version", contract)
    if contract_version != expected:
        raise SystemExit(
            f"ERROR: {contract} version {contract_version} does not match "
            f"Cargo.toml version {expected}"
        )

    adapters = root / "adapters" / "tokenless"
    json_manifests = (
        adapters / "manifest.json",
        adapters / "openclaw" / "package.json",
        adapters / "openclaw" / "openclaw.plugin.json",
        adapters / "dsh" / "package.json",
        adapters / "qoder" / ".qoder-plugin" / "plugin.json",
        adapters / "claude-code" / ".claude-plugin" / "plugin.json",
        adapters / "codex" / ".codex-plugin" / "plugin.json",
        adapters / "qwencode" / "qwen-extension.json",
        adapters / "qwenpaw" / "plugin.json",
    )
    versions = {str(path.relative_to(root)): read_json_version(path) for path in json_manifests}
    hermes = adapters / "hermes" / "plugin.yaml"
    versions[str(hermes.relative_to(root))] = read_hermes_version(hermes)
    drift = [f"{path}={version}" for path, version in versions.items() if version != expected]
    if drift:
        raise SystemExit(
            f"ERROR: generated adapter versions do not match {expected}: "
            + ", ".join(drift)
        )
    # The QwenPaw requirements file is stamped too, but as wheel URLs: a
    # stale or unstamped version there only fails inside `qwenpaw plugin install`.
    requirements = adapters / "qwenpaw" / "requirements.txt"
    text = requirements.read_text(encoding="utf-8")
    referenced = set(re.findall(r"/tokenless/v([^/]+)/", text)) | set(
        re.findall(r"anolisa_tokenless-([^-]+)-", text)
    )
    if "@VERSION@" in text or referenced != {expected}:
        raise SystemExit(
            f"ERROR: {requirements.relative_to(root)} does not reference "
            f"tokenless {expected}: {sorted(referenced) or 'no wheel URL'}"
        )
    return expected


def parse_args() -> argparse.Namespace:
    """Parse source and contract paths."""
    parser = argparse.ArgumentParser()
    parser.add_argument("source_root", type=Path)
    parser.add_argument("contract", type=Path)
    return parser.parse_args()


def main() -> int:
    """Print the verified release version for the packaging shell script."""
    args = parse_args()
    print(verify_versions(args.source_root.resolve(), args.contract.resolve()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
