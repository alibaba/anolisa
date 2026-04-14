"""Thread-safe, rotation-aware JSONL writer for security events."""

from __future__ import annotations

import fcntl
import json
import os
import shutil
import sys
import threading
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional, TextIO

from agent_sec_cli.security_events.config import get_log_path
from agent_sec_cli.security_events.schema import SecurityEvent

# Default maximum log file size before rotation (100 MB)
DEFAULT_MAX_BYTES = 100 * 1024 * 1024
# Default number of rotated files to keep
DEFAULT_BACKUP_COUNT = 10


class SecurityEventWriter:
    """Append ``SecurityEvent`` records to a JSONL file.

    * **Thread-safe** — every ``write()`` is guarded by a ``threading.Lock``.
    * **Rotation-safe** — before each write the current inode is compared to
      the one recorded at open time; a mismatch (or missing file) triggers a
      transparent reopen.
    * **Auto-rotation** — automatically rotates the log file when it exceeds
      ``max_bytes`` (default: 100 MB), keeping up to ``backup_count`` backup
      files (default: 10).
    * **Fire-and-forget** — all internal errors are swallowed so that logging
      never disrupts the caller.
    """

    def __init__(
        self,
        path: str | Path | None = None,
        max_bytes: int = DEFAULT_MAX_BYTES,
        backup_count: int = DEFAULT_BACKUP_COUNT,
    ) -> None:
        self._path: Path = Path(path) if path else Path(get_log_path())
        self._max_bytes = max_bytes
        self._backup_count = backup_count
        self._lock = threading.Lock()
        self._fd: Optional[TextIO] = None
        self._inode: Optional[int] = None
        self._open()

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _open(self) -> None:
        """Open (or reopen) the log file and record its inode."""
        try:
            self._fd = self._path.open("a", encoding="utf-8")
            self._inode = os.fstat(self._fd.fileno()).st_ino
        except OSError as exc:
            print(
                f"[security_events] failed to open {self._path}: {exc}",
                file=sys.stderr,
            )
            self._fd = None
            self._inode = None

    def _close(self) -> None:
        """Close the current file descriptor if open."""
        if self._fd is not None:
            try:
                self._fd.close()
            except OSError:
                pass
            self._fd = None
            self._inode = None

    def _ensure_file(self) -> None:
        """Reopen the log file when inode changed or file was deleted."""
        try:
            st = self._path.stat()
            if st.st_ino != self._inode:
                self._close()
                self._open()
        except FileNotFoundError:
            self._close()
            self._open()
        except OSError as exc:
            print(
                f"[security_events] stat failed for {self._path}: {exc}",
                file=sys.stderr,
            )
            self._close()
            self._open()

    def _needs_rotation(self, additional_bytes: int = 0) -> bool:
        """Check if the current log file would exceed the size limit after adding additional_bytes."""
        try:
            st = self._path.stat()
            return st.st_size + additional_bytes >= self._max_bytes
        except OSError:
            return False

    def _rotate(self) -> None:
        """Rotate the log file by renaming it with a timestamp suffix.

        Rotation scheme:
            security-events.jsonl                       -> current (will be rotated)
            security-events.jsonl.20260408-143022.123   -> rotated at 2026-04-08 14:30:22.123
            security-events.jsonl.20260408-120515.456   -> rotated at 2026-04-08 12:05:15.456

        After rotation, old backups exceeding ``backup_count`` are cleaned up.
        """
        # Generate timestamp-based backup filename with millisecond precision
        timestamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S.%f")[:-3]  # Truncate to milliseconds
        backup_path = self._path.parent / f"{self._path.name}.{timestamp}"

        # Rotate current file to timestamp-named backup
        try:
            shutil.move(self._path, backup_path)
        except OSError as exc:
            print(
                f"[security_events] rotation failed: {exc}",
                file=sys.stderr,
            )
            return

        # Clean up old backups exceeding backup_count
        self._cleanup_old_backups()

        # Close the old file descriptor and open a new one
        self._close()
        self._open()

    def _rotate_under_flock(self) -> None:
        """Acquire a cross-process advisory lock, re-check size, then rotate.

        Normal writes (O_APPEND, < 4 KB) are already atomic across processes.
        Only the rotation path needs cross-process synchronization because
        ``_needs_rotation()`` + ``shutil.move()`` is a TOCTOU race when
        multiple processes detect the size threshold simultaneously.

        This method uses ``fcntl.flock(LOCK_EX | LOCK_NB)``:
        - The **winner** re-checks size under the lock and rotates.
        - **Losers** get ``BlockingIOError``, skip rotation, and reopen
          the (now-new) log file.
        """
        lock_path = self._path.parent / (self._path.name + ".lock")
        lock_fd = None
        try:
            lock_fd = lock_path.open("w")
            fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            # Another process is rotating — just reopen and continue
            self._close()
            self._open()
            return
        except OSError:
            # Lock file creation failed — fall through without rotation
            return

        try:
            # Re-check under lock — the winner may have already rotated
            if self._needs_rotation(0):
                self._rotate()
            else:
                # Already rotated by another process — just reopen
                self._close()
                self._open()
        finally:
            if lock_fd is not None:
                try:
                    fcntl.flock(lock_fd, fcntl.LOCK_UN)
                    lock_fd.close()
                except OSError:
                    pass

    def _cleanup_old_backups(self) -> None:
        """Remove oldest backup files if count exceeds backup_count.

        Backups are identified by the timestamp suffix pattern and sorted
        by modification time to determine which are oldest.
        """
        try:
            # Find all backup files matching the pattern
            dir_path = self._path.parent
            base_name = self._path.name

            backup_files = []
            for entry in dir_path.iterdir():
                # Match pattern: {base_name}.{timestamp}
                if entry.name.startswith(f"{base_name}.") and not entry.name.endswith((".tmp", ".lock")):
                    if entry.is_file():
                        # Use mtime to sort (more reliable than parsing timestamp)
                        mtime = entry.stat().st_mtime
                        backup_files.append((entry, mtime))

            # Sort by modification time (oldest first)
            backup_files.sort(key=lambda x: x[1])

            # Remove oldest files if we exceed backup_count
            while len(backup_files) > self._backup_count:
                oldest_path, _ = backup_files.pop(0)
                try:
                    oldest_path.unlink()
                except OSError:
                    pass
        except OSError as exc:
            print(
                f"[security_events] cleanup failed: {exc}",
                file=sys.stderr,
            )

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def write(self, event: SecurityEvent) -> None:
        """Serialize *event* and append it as a single JSONL line.

        This method is safe to call from any thread and will never raise.
        """
        with self._lock:
            try:
                self._ensure_file()
                if self._fd is None:
                    return

                # Calculate the size of the line we're about to write
                line = json.dumps(event.to_dict(), ensure_ascii=False) + "\n"
                line_bytes = len(line.encode('utf-8'))

                # Check if rotation is needed before writing (accounting for the new line)
                if self._needs_rotation(line_bytes):
                    self._rotate_under_flock()
                    if self._fd is None:
                        return

                self._fd.write(line)
                self._fd.flush()
            except Exception as exc:  # noqa: BLE001
                print(
                    f"[security_events] write error: {exc}",
                    file=sys.stderr,
                )
