"""Custom exception hierarchy for skill-ledger."""


class SkillLedgerError(Exception):
    """Base exception for all skill-ledger errors."""

    pass


# ---------------------------------------------------------------------------
# Key management
# ---------------------------------------------------------------------------


class KeyNotFoundError(SkillLedgerError):
    """Signing key files do not exist (run ``init-keys`` first)."""

    def __init__(self, path: str) -> None:
        super().__init__(
            f"Signing key not found: {path}. Run 'agent-sec-cli skill-ledger init-keys' first."
        )
        self.path = path


class KeyAlreadyExistsError(SkillLedgerError):
    """Signing key already exists and ``--force`` was not supplied."""

    def __init__(self, path: str) -> None:
        super().__init__(f"Key already exists: {path}. Use --force to overwrite.")
        self.path = path


class PassphraseError(SkillLedgerError):
    """Passphrase is incorrect or could not be obtained."""

    pass


# ---------------------------------------------------------------------------
# Manifest / signature
# ---------------------------------------------------------------------------


class ManifestNotFoundError(SkillLedgerError):
    """``latest.json`` does not exist in ``.skill-meta/``."""

    def __init__(self, skill_dir: str) -> None:
        super().__init__(f"No manifest found in {skill_dir}/.skill-meta/latest.json")
        self.skill_dir = skill_dir


class ManifestCorruptedError(SkillLedgerError):
    """``latest.json`` exists but cannot be parsed or has invalid structure."""

    def __init__(self, skill_dir: str, reason: str) -> None:
        super().__init__(f"Corrupted manifest in {skill_dir}: {reason}")
        self.skill_dir = skill_dir
        self.reason = reason


class SignatureInvalidError(SkillLedgerError):
    """Digital signature verification failed (possible tampering)."""

    def __init__(self, reason: str) -> None:
        super().__init__(f"Signature verification failed: {reason}")
        self.reason = reason


class ManifestHashMismatchError(SkillLedgerError):
    """Recomputed manifestHash does not match the stored value."""

    def __init__(self) -> None:
        super().__init__("manifestHash mismatch — manifest content has been modified")


# ---------------------------------------------------------------------------
# Version chain
# ---------------------------------------------------------------------------


class ChainBrokenError(SkillLedgerError):
    """``previousManifestSignature`` does not match the prior version's signature."""

    def __init__(self, version_id: str, reason: str) -> None:
        super().__init__(f"Version chain broken at {version_id}: {reason}")
        self.version_id = version_id
        self.reason = reason


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------


class ConfigError(SkillLedgerError):
    """Configuration file is missing or invalid."""

    def __init__(self, reason: str) -> None:
        super().__init__(f"Configuration error: {reason}")
        self.reason = reason


# ---------------------------------------------------------------------------
# Findings
# ---------------------------------------------------------------------------


class FindingsFileError(SkillLedgerError):
    """Findings JSON file is missing or invalid."""

    def __init__(self, path: str, reason: str) -> None:
        super().__init__(f"Invalid findings file {path}: {reason}")
        self.path = path
        self.reason = reason
