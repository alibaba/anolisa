"""Certify command — three-phase manifest creation and scan-result signing.

Implements ``agent-sec-cli skill-ledger certify`` with two input modes:

- **External findings mode** (``--findings``): read a findings file produced
  by an external scanner (e.g. skill-vetter via Agent).
- **Auto-invoke mode** (no ``--findings``): auto-invoke registered non-``skill``
  scanners from the registry.  In v1 no such scanners exist; framework only.

Three execution phases:

1. **Consistency** — ensure manifest exists and matches current files.
2. **Collect**    — obtain scan results (external file or auto-invoke).
3. **Update**     — normalise findings, merge scans[], aggregate, re-sign.
"""

import json
import logging
from pathlib import Path
from typing import Any

from agent_sec_cli.skill_ledger.core.file_hasher import (
    compute_file_hashes,
    diff_file_hashes,
)
from agent_sec_cli.skill_ledger.core.version_chain import (
    create_snapshot,
    get_previous_signature,
    load_latest_manifest,
    next_version_id,
    save_manifest,
)
from agent_sec_cli.skill_ledger.errors import FindingsFileError
from agent_sec_cli.skill_ledger.models.finding import NormalizedFinding
from agent_sec_cli.skill_ledger.models.manifest import (
    ManifestSignature,
    SignedManifest,
)
from agent_sec_cli.skill_ledger.models.scan import (
    ScanEntry,
    aggregate_scan_status,
)
from agent_sec_cli.skill_ledger.scanner.parsers import parse_findings
from agent_sec_cli.skill_ledger.scanner.registry import ScannerRegistry
from agent_sec_cli.skill_ledger.signing.base import SigningBackend
from agent_sec_cli.skill_ledger.utils import utc_now_iso

logger = logging.getLogger(__name__)


def _sign_manifest(manifest: SignedManifest, backend: SigningBackend) -> SignedManifest:
    """Compute manifestHash, sign it, and attach the signature to *manifest*."""
    manifest.manifestHash = manifest.compute_manifest_hash()
    sig_value, fingerprint = backend.sign(manifest.manifestHash.encode("utf-8"))
    manifest.signature = ManifestSignature(
        algorithm=backend.name,
        value=sig_value,
        keyFingerprint=fingerprint,
    )
    return manifest


def _load_findings(findings_path: str) -> list[dict[str, Any]]:
    """Load and validate the findings JSON file."""
    path = Path(findings_path)
    if not path.is_file():
        raise FindingsFileError(findings_path, "file does not exist")
    try:
        raw = path.read_text(encoding="utf-8")
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise FindingsFileError(findings_path, f"invalid JSON: {exc}") from exc

    # Accept both a bare list and {"findings": [...]}
    if isinstance(data, list):
        return data
    if isinstance(data, dict) and "findings" in data:
        findings = data["findings"]
        if isinstance(findings, list):
            return findings
    raise FindingsFileError(
        findings_path,
        "expected a JSON array or an object with a 'findings' key",
    )


def _determine_scan_status(findings: list[NormalizedFinding]) -> str:
    """Derive the scan status from a list of normalised findings.

    - Any finding with ``level == "deny"``     → ``"deny"``
    - Any finding with ``level == "warn"``     → ``"warn"``
    - Otherwise                                → ``"pass"``
    """
    has_deny = any(f.level == "deny" for f in findings)
    if has_deny:
        return "deny"
    has_warn = any(f.level == "warn" for f in findings)
    if has_warn:
        return "warn"
    return "pass"


def _build_scan_entry(
    normalized: list[NormalizedFinding],
    scanner: str,
    scanner_version: str | None,
) -> ScanEntry:
    """Construct a :class:`ScanEntry` from normalised findings."""
    return ScanEntry(
        scanner=scanner,
        version=scanner_version or "unknown",
        status=_determine_scan_status(normalized),
        findings=[f.to_findings_dict() for f in normalized],
        scannedAt=utc_now_iso(),
    )


def _resolve_parser_and_normalise(
    raw_findings: list[dict[str, Any]],
    scanner_name: str,
    registry: ScannerRegistry,
) -> list[NormalizedFinding]:
    """Look up the parser for *scanner_name* and normalise raw findings.

    Falls back to ``findings-array`` if the scanner is not registered
    (backward-compatible).
    """
    parser_info = registry.get_parser_for_scanner(scanner_name)
    if parser_info is None:
        logger.debug(
            "Scanner %r not in registry; falling back to findings-array parser",
            scanner_name,
        )
    return parse_findings(raw_findings, parser_info)


# ------------------------------------------------------------------
# Auto-invoke mode (framework — v1 has no invocable scanners)
# ------------------------------------------------------------------


def _auto_invoke_scanners(
    skill_dir: str,
    registry: ScannerRegistry,
    scanner_names: list[str] | None = None,
) -> list[ScanEntry]:
    """Invoke registered non-``skill`` scanners and collect results.

    In v1 only ``skill-vetter`` (type ``"skill"``) is registered, so this
    function returns an empty list.  The framework is ready for future
    ``builtin``/``cli``/``api`` scanner adapters.
    """
    invocable = registry.list_invocable_scanners(names=scanner_names)

    if not invocable:
        logger.info("No auto-invocable scanners registered; skipping auto-invoke")
        return []

    # Future: iterate invocable scanners, call adapter, parse, build ScanEntry
    entries: list[ScanEntry] = []
    for scanner_info in invocable:
        logger.warning(
            "Scanner %r (type=%r) auto-invoke not yet implemented; skipping",
            scanner_info.name,
            scanner_info.type,
        )
        # TODO: dispatch by scanner_info.type:
        #   "builtin" → call Python function
        #   "cli"     → subprocess.run(...)
        #   "api"     → HTTP POST
    return entries


# ------------------------------------------------------------------
# Main certify workflow
# ------------------------------------------------------------------


def certify(
    skill_dir: str,
    backend: SigningBackend,
    findings_path: str | None = None,
    scanner: str = "skill-vetter",
    scanner_version: str | None = None,
    scanner_names: list[str] | None = None,
) -> dict[str, Any]:
    """Execute the full certify workflow for a single skill directory.

    Two input modes:

    - *findings_path* provided → **external findings mode**: read the file,
      normalise via parser, build a single ScanEntry.
    - *findings_path* is ``None`` → **auto-invoke mode**: invoke all
      registered non-``skill`` scanners and collect results.

    Returns a JSON-serialisable result dict.
    """
    skill_name = Path(skill_dir).name
    current_hashes = compute_file_hashes(skill_dir)
    registry = ScannerRegistry.from_config()

    # ── Phase 1: Ensure manifest consistency ──
    manifest = load_latest_manifest(skill_dir)
    new_version_created = False

    if (
        manifest is None
        or not diff_file_hashes(manifest.fileHashes, current_hashes)["match"]
    ):
        vid = next_version_id(skill_dir)
        prev_sig = get_previous_signature(skill_dir)
        prev_vid = manifest.versionId if manifest is not None else None

        manifest = SignedManifest(
            versionId=vid,
            previousVersionId=prev_vid,
            skillName=skill_name,
            fileHashes=current_hashes,
            scanStatus="none",
            previousManifestSignature=prev_sig,
        )
        new_version_created = True
        create_snapshot(skill_dir, vid)

    # ── Phase 2: Collect scan results ──
    scan_entries: list[ScanEntry] = []

    if findings_path is not None:
        # External findings mode
        raw_findings = _load_findings(findings_path)
        normalized = _resolve_parser_and_normalise(raw_findings, scanner, registry)
        scan_entries.append(_build_scan_entry(normalized, scanner, scanner_version))
    else:
        # Auto-invoke mode
        scan_entries = _auto_invoke_scanners(skill_dir, registry, scanner_names)

    # ── Phase 3: Update manifest and sign ──
    if scan_entries:
        for entry in scan_entries:
            # Merge: replace existing entry for same scanner, or append
            manifest.scans = [s for s in manifest.scans if s.scanner != entry.scanner]
            manifest.scans.append(entry)

        manifest.scanStatus = aggregate_scan_status(manifest.scans)

    # Short-circuit: nothing changed — avoid re-signing and overwriting
    # Otherwise re-sign and persist (manifestHash recomputed each time)
    if scan_entries or new_version_created:
        _sign_manifest(manifest, backend)
        save_manifest(skill_dir, manifest, write_version=True)

    return {
        "versionId": manifest.versionId,
        "scanStatus": manifest.scanStatus,
        "newVersion": new_version_created,
        "skillName": skill_name,
    }


def certify_batch(
    skill_dirs: list[Path],
    backend: SigningBackend,
    findings_path: str | None = None,
    scanner: str = "skill-vetter",
    scanner_version: str | None = None,
    scanner_names: list[str] | None = None,
) -> list[dict[str, Any]]:
    """Certify multiple skill directories (``--all`` mode).

    .. note::

       *findings_path* applies only to the **first** skill directory when
       set, because findings are scanner results for a specific skill.
       For the remaining directories auto-invoke mode is used.

    Returns a list of per-skill result dicts.
    """
    if findings_path and len(skill_dirs) > 1:
        logger.warning(
            "--findings applies to the first skill only; "
            "remaining %d skills will use auto-invoke mode",
            len(skill_dirs) - 1,
        )

    results: list[dict[str, Any]] = []
    for idx, skill_dir in enumerate(skill_dirs):
        # Only apply external findings to the first skill
        effective_findings = findings_path if idx == 0 else None
        try:
            result = certify(
                str(skill_dir),
                backend,
                findings_path=effective_findings,
                scanner=scanner,
                scanner_version=scanner_version,
                scanner_names=scanner_names,
            )
            results.append(result)
        except Exception as exc:
            results.append(
                {
                    "skillName": skill_dir.name,
                    "error": str(exc),
                }
            )
    return results
