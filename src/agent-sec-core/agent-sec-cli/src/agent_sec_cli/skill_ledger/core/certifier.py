"""Scan and certify workflows for signed skill-ledger manifests.

``scan`` runs built-in scanners and records their results. ``certify`` imports
findings produced elsewhere, primarily by the Agent-driven skill-vetter flow.
Both paths share the same manifest update, aggregation, signing, and persistence
logic.
"""

import json
import logging
from pathlib import Path
from typing import Any

from agent_sec_cli.skill_ledger.config import remember_skill_dir
from agent_sec_cli.skill_ledger.core.file_hasher import (
    compute_file_hashes,
    diff_file_hashes,
)
from agent_sec_cli.skill_ledger.core.version_chain import (
    create_snapshot,
    get_previous_signature,
    list_version_ids,
    load_latest_manifest,
    next_version_id,
    save_manifest,
)
from agent_sec_cli.skill_ledger.errors import FindingsFileError, SignatureInvalidError
from agent_sec_cli.skill_ledger.models.finding import NormalizedFinding
from agent_sec_cli.skill_ledger.models.manifest import (
    ManifestSignature,
    SignedManifest,
)
from agent_sec_cli.skill_ledger.models.scan import (
    ScanEntry,
    aggregate_scan_status,
)
from agent_sec_cli.skill_ledger.scanner import skill_code_scanner
from agent_sec_cli.skill_ledger.scanner.builtins.dispatcher import (
    run_builtin_scanner,
)
from agent_sec_cli.skill_ledger.scanner.names import (
    DEFAULT_BUILTIN_SCANNERS,
    canonicalize_scanner_name,
)
from agent_sec_cli.skill_ledger.scanner.parsers import parse_findings
from agent_sec_cli.skill_ledger.scanner.registry import (
    ScannerInfo,
    ScannerRegistry,
)
from agent_sec_cli.skill_ledger.signing.base import SigningBackend
from agent_sec_cli.skill_ledger.utils import utc_now_iso, validate_skill_dir

logger = logging.getLogger(__name__)

_ManifestState = str  # missing | trusted | unsigned | drifted | tampered


def _remember_skill_dir_best_effort(skill_dir: str) -> None:
    """Append unknown skill dirs to managedSkillDirs without failing the command."""
    try:
        remember_skill_dir(Path(skill_dir))
    except Exception:
        logger.debug("auto-remember failed for %s, continuing", skill_dir, exc_info=True)


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
    """Derive the per-scanner status from normalised findings."""
    if any(f.level == "deny" for f in findings):
        return "deny"
    if any(f.level == "warn" for f in findings):
        return "warn"
    return "pass"


def _build_scan_entry(
    normalized: list[NormalizedFinding],
    scanner: str,
    scanner_version: str | None,
) -> ScanEntry:
    """Construct a :class:`ScanEntry` from normalised findings."""
    return ScanEntry(
        scanner=canonicalize_scanner_name(scanner),
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
    """Look up the parser for *scanner_name* and normalise raw findings."""
    canonical_name = canonicalize_scanner_name(scanner_name)
    parser_info = registry.get_parser_for_scanner(canonical_name)
    if parser_info is None:
        logger.debug(
            "Scanner %r not in registry; falling back to findings-array parser",
            canonical_name,
        )
    return parse_findings(raw_findings, parser_info)


def _auto_invoke_scanners(
    skill_dir: str,
    registry: ScannerRegistry,
    scanner_names: list[str] | None = None,
) -> list[ScanEntry]:
    """Invoke registered non-``skill`` scanners and collect results."""
    invocable = registry.list_invocable_scanners(
        names=scanner_names or DEFAULT_BUILTIN_SCANNERS
    )

    if not invocable:
        logger.info("No auto-invocable scanners registered; skipping auto-invoke")
        return []

    entries: list[ScanEntry] = []
    for scanner_info in invocable:
        invoked = _invoke_scanner(skill_dir, scanner_info)
        if invoked is None:
            continue

        raw_findings, scanner_name, scanner_version = invoked
        normalized = _resolve_parser_and_normalise(
            raw_findings,
            scanner_name,
            registry,
        )
        entries.append(
            _build_scan_entry(
                normalized,
                scanner_name,
                scanner_version,
            )
        )

    return entries


def _invoke_scanner(
    skill_dir: str,
    scanner_info: ScannerInfo,
) -> tuple[list[dict[str, Any]], str, str | None] | None:
    """Dispatch a registered scanner and return findings, name, and version."""
    if _is_skill_code_scanner(scanner_info):
        return (
            skill_code_scanner.scan_skill_code(skill_dir),
            scanner_info.name,
            _scanner_version(scanner_info),
        )

    if scanner_info.type == "builtin":
        try:
            result = run_builtin_scanner(
                scanner_info.name,
                skill_dir,
                options=scanner_info.extra,
            )
        except ValueError:
            logger.warning(
                "Scanner %r (type=%r) auto-invoke not implemented; skipping",
                scanner_info.name,
                scanner_info.type,
            )
            return None
        return result.findings, result.scanner, result.version

    logger.warning(
        "Scanner %r (type=%r) auto-invoke not implemented; skipping",
        scanner_info.name,
        scanner_info.type,
    )
    return None


def _scanner_version(scanner_info: ScannerInfo) -> str | None:
    configured_version = scanner_info.extra.get("version")
    if configured_version is not None:
        return str(configured_version)
    if _is_skill_code_scanner(scanner_info):
        return skill_code_scanner.SCANNER_VERSION
    return None


def _is_skill_code_scanner(scanner_info: ScannerInfo) -> bool:
    return (
        scanner_info.type == "builtin"
        and scanner_info.name == skill_code_scanner.SCANNER_NAME
    )


def _safe_load_latest_manifest(skill_dir: str) -> tuple[SignedManifest | None, bool]:
    """Load latest.json, returning ``(None, True)`` when it is corrupted."""
    try:
        return load_latest_manifest(skill_dir), False
    except (json.JSONDecodeError, ValueError):
        return None, True


def _classify_manifest(
    manifest: SignedManifest | None,
    current_hashes: dict[str, str],
    backend: SigningBackend,
    *,
    corrupted: bool = False,
) -> _ManifestState:
    """Classify the existing manifest before a write-oriented operation."""
    if corrupted:
        return "tampered"
    if manifest is None:
        return "missing"
    if not diff_file_hashes(manifest.fileHashes, current_hashes)["match"]:
        return "drifted"
    expected_hash = manifest.compute_manifest_hash()
    if manifest.manifestHash != expected_hash:
        return "tampered"
    if manifest.signature is None:
        return "unsigned"
    try:
        backend.verify(
            manifest.manifestHash.encode("utf-8"),
            manifest.signature.value,
            manifest.signature.keyFingerprint,
        )
    except SignatureInvalidError:
        return "tampered"
    return "trusted"


def _previous_version_id(skill_dir: str, manifest: SignedManifest | None) -> str | None:
    """Return the best available previous version id for a new manifest."""
    if manifest is not None:
        return manifest.versionId
    existing = list_version_ids(skill_dir)
    return existing[-1] if existing else None


def _previous_signature(skill_dir: str, manifest: SignedManifest | None) -> str | None:
    """Return the best available previous signature for a new manifest."""
    if manifest is not None and manifest.signature is not None:
        return manifest.signature.value
    return get_previous_signature(skill_dir)


def _new_manifest(
    skill_dir: str,
    current_hashes: dict[str, str],
    previous_manifest: SignedManifest | None,
) -> SignedManifest:
    """Create a new unsigned manifest object for the current skill contents."""
    skill_name = Path(skill_dir).name
    return SignedManifest(
        versionId=next_version_id(skill_dir),
        previousVersionId=_previous_version_id(skill_dir, previous_manifest),
        skillName=skill_name,
        fileHashes=current_hashes,
        scanStatus="none",
        previousManifestSignature=_previous_signature(skill_dir, previous_manifest),
    )


def _prepare_manifest_for_update(
    skill_dir: str,
    current_hashes: dict[str, str],
    backend: SigningBackend,
) -> tuple[SignedManifest, _ManifestState, bool]:
    """Return a manifest ready to receive scan entries.

    Missing, drifted, or tampered manifests create a new version. Unsigned
    baselines are reused and signed in-place.
    """
    loaded, corrupted = _safe_load_latest_manifest(skill_dir)
    state = _classify_manifest(loaded, current_hashes, backend, corrupted=corrupted)
    if state in {"missing", "drifted", "tampered"}:
        manifest = _new_manifest(skill_dir, current_hashes, loaded)
        return manifest, state, True
    if loaded is None:
        # Defensive fallback; state should be "missing" above.
        manifest = _new_manifest(skill_dir, current_hashes, None)
        return manifest, "missing", True
    return loaded, state, False


def _canonical_scan_name_set(scans: list[ScanEntry]) -> set[str]:
    return {canonicalize_scanner_name(scan.scanner) for scan in scans}


def _merge_scan_entries(
    manifest: SignedManifest,
    scan_entries: list[ScanEntry],
) -> None:
    """Replace existing scanner entries with incoming entries and canonical names."""
    incoming = {canonicalize_scanner_name(entry.scanner) for entry in scan_entries}
    merged: list[ScanEntry] = []
    seen: set[str] = set()

    for existing in manifest.scans:
        canonical = canonicalize_scanner_name(existing.scanner)
        if canonical in incoming or canonical in seen:
            continue
        existing.scanner = canonical
        merged.append(existing)
        seen.add(canonical)

    for entry in scan_entries:
        entry.scanner = canonicalize_scanner_name(entry.scanner)
        if entry.scanner in seen:
            continue
        merged.append(entry)
        seen.add(entry.scanner)

    manifest.scans = merged
    manifest.scanStatus = aggregate_scan_status(manifest.scans)


def _persist_manifest_update(
    skill_dir: str,
    manifest: SignedManifest,
    scan_entries: list[ScanEntry],
    backend: SigningBackend,
    *,
    new_version_created: bool = False,
) -> None:
    """Merge scan entries, sign the manifest, and save latest/version JSON."""
    if new_version_created:
        create_snapshot(skill_dir, manifest.versionId)
    _merge_scan_entries(manifest, scan_entries)
    manifest.updatedAt = utc_now_iso()
    _sign_manifest(manifest, backend)
    save_manifest(skill_dir, manifest, write_version=True)


def _result_payload(
    manifest: SignedManifest,
    *,
    skill_dir: str,
    new_version_created: bool,
    scanners_run: list[str],
    skipped_scanners: list[str] | None = None,
    status: str = "scanned",
    extra: dict[str, Any] | None = None,
) -> dict[str, Any]:
    data: dict[str, Any] = {
        "status": status,
        "versionId": manifest.versionId,
        "scanStatus": manifest.scanStatus,
        "newVersion": new_version_created,
        "skillName": Path(skill_dir).name,
        "createdAt": manifest.createdAt,
        "updatedAt": manifest.updatedAt,
        "fileCount": len(manifest.fileHashes),
        "manifestHash": manifest.manifestHash,
        "scannersRun": scanners_run,
    }
    if skipped_scanners is not None:
        data["skippedScanners"] = skipped_scanners
    if extra:
        data.update(extra)
    return data


def scan_skill(
    skill_dir: str,
    backend: SigningBackend,
    scanner_names: list[str] | None = None,
    *,
    force: bool = False,
) -> dict[str, Any]:
    """Run built-in scanners as needed and record signed scan results."""
    validate_skill_dir(skill_dir)
    _remember_skill_dir_best_effort(skill_dir)

    current_hashes = compute_file_hashes(skill_dir)
    registry = ScannerRegistry.from_config()
    requested = [
        canonicalize_scanner_name(name)
        for name in (scanner_names or DEFAULT_BUILTIN_SCANNERS)
    ]

    manifest, state, new_version_created = _prepare_manifest_for_update(
        skill_dir, current_hashes, backend
    )

    if force or state in {"missing", "unsigned", "drifted", "tampered"}:
        scanners_to_run = requested
    else:
        existing = _canonical_scan_name_set(manifest.scans)
        scanners_to_run = [name for name in requested if name not in existing]

    if not scanners_to_run:
        return _result_payload(
            manifest,
            skill_dir=skill_dir,
            new_version_created=False,
            scanners_run=[],
            skipped_scanners=requested,
            status="noop",
        )

    scan_entries = _auto_invoke_scanners(skill_dir, registry, scanners_to_run)
    if not scan_entries:
        return _result_payload(
            manifest,
            skill_dir=skill_dir,
            new_version_created=False,
            scanners_run=[],
            skipped_scanners=scanners_to_run,
            status="noop",
        )

    _persist_manifest_update(
        skill_dir,
        manifest,
        scan_entries,
        backend,
        new_version_created=new_version_created,
    )
    return _result_payload(
        manifest,
        skill_dir=skill_dir,
        new_version_created=new_version_created,
        scanners_run=[entry.scanner for entry in scan_entries],
        skipped_scanners=[name for name in requested if name not in scanners_to_run],
    )


def scan_batch(
    skill_dirs: list[Path],
    backend: SigningBackend,
    scanner_names: list[str] | None = None,
    *,
    force: bool = False,
) -> list[dict[str, Any]]:
    """Run ``scan`` over multiple skill directories."""
    results: list[dict[str, Any]] = []
    for skill_dir in skill_dirs:
        try:
            results.append(
                scan_skill(
                    str(skill_dir),
                    backend,
                    scanner_names=scanner_names,
                    force=force,
                )
            )
        except Exception as exc:
            results.append(
                {
                    "skillName": skill_dir.name,
                    "status": "error",
                    "error": str(exc),
                }
            )
    return results


def certify(
    skill_dir: str,
    backend: SigningBackend,
    findings_path: str | None = None,
    scanner: str = "skill-vetter",
    scanner_version: str | None = None,
    *,
    delete_findings: bool = False,
) -> dict[str, Any]:
    """Import external scanner findings and record them in a signed manifest."""
    if findings_path is None:
        raise FindingsFileError(
            "<missing>",
            "--findings is required for certify; use 'skill-ledger scan' for built-in scanners",
        )

    validate_skill_dir(skill_dir)
    _remember_skill_dir_best_effort(skill_dir)

    current_hashes = compute_file_hashes(skill_dir)
    registry = ScannerRegistry.from_config()
    manifest, _state, new_version_created = _prepare_manifest_for_update(
        skill_dir, current_hashes, backend
    )

    raw_findings = _load_findings(findings_path)
    normalized = _resolve_parser_and_normalise(raw_findings, scanner, registry)
    scan_entry = _build_scan_entry(normalized, scanner, scanner_version)

    _persist_manifest_update(
        skill_dir,
        manifest,
        [scan_entry],
        backend,
        new_version_created=new_version_created,
    )

    delete_result: dict[str, Any] = {}
    if delete_findings:
        try:
            Path(findings_path).unlink()
            delete_result["findingsDeleted"] = True
        except OSError as exc:
            delete_result["findingsDeleted"] = False
            delete_result["findingsDeleteError"] = str(exc)

    return _result_payload(
        manifest,
        skill_dir=skill_dir,
        new_version_created=new_version_created,
        scanners_run=[scan_entry.scanner],
        extra=delete_result,
    )


def certify_batch(
    skill_dirs: list[Path],
    backend: SigningBackend,
    findings_path: str | None = None,
    scanner: str = "skill-vetter",
    scanner_version: str | None = None,
) -> list[dict[str, Any]]:
    """Compatibility helper for callers that still import certify_batch."""
    results: list[dict[str, Any]] = []
    for skill_dir in skill_dirs:
        try:
            results.append(
                certify(
                    str(skill_dir),
                    backend,
                    findings_path=findings_path,
                    scanner=scanner,
                    scanner_version=scanner_version,
                )
            )
        except Exception as exc:
            results.append(
                {
                    "skillName": skill_dir.name,
                    "status": "error",
                    "error": str(exc),
                }
            )
    return results
