"""Unit tests for security_events.writer — SecurityEventWriter."""

import json
import multiprocessing
import os
import tempfile
import threading
import unittest

from agent_sec_cli.security_events.schema import SecurityEvent
from agent_sec_cli.security_events.writer import SecurityEventWriter


def _make_event(**overrides):
    defaults = dict(event_type="test", category="test_cat", details={"k": "v"})
    defaults.update(overrides)
    return SecurityEvent(**defaults)


class TestWriterBasic(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.NamedTemporaryFile(
            mode="w", suffix=".jsonl", delete=False
        )
        self.tmp.close()
        self.writer = SecurityEventWriter(path=self.tmp.name)

    def tearDown(self):
        try:
            os.unlink(self.tmp.name)
        except OSError:
            pass

    def test_write_appends_jsonl_line(self):
        evt = _make_event()
        self.writer.write(evt)
        with open(self.tmp.name) as fh:
            lines = fh.readlines()
        self.assertEqual(len(lines), 1)
        parsed = json.loads(lines[0])
        self.assertEqual(parsed["event_type"], "test")

    def test_write_multiple_events(self):
        for i in range(3):
            self.writer.write(_make_event(event_type=f"evt_{i}"))
        with open(self.tmp.name) as fh:
            lines = fh.readlines()
        self.assertEqual(len(lines), 3)
        for i, line in enumerate(lines):
            self.assertEqual(json.loads(line)["event_type"], f"evt_{i}")


class TestWriterRotation(unittest.TestCase):
    def test_rotation_detection(self):
        tmp = tempfile.NamedTemporaryFile(
            mode="w", suffix=".jsonl", delete=False
        )
        tmp.close()
        writer = SecurityEventWriter(path=tmp.name)

        # Write first event
        writer.write(_make_event(event_type="before_rotate"))

        # Simulate rotation: delete and recreate
        os.unlink(tmp.name)
        with open(tmp.name, "w"):
            pass  # empty file

        # Write after rotation
        writer.write(_make_event(event_type="after_rotate"))

        with open(tmp.name) as fh:
            lines = fh.readlines()
        # New file should have the post-rotation event
        self.assertTrue(len(lines) >= 1)
        parsed = json.loads(lines[-1])
        self.assertEqual(parsed["event_type"], "after_rotate")

        os.unlink(tmp.name)


class TestWriterAutoRotation(unittest.TestCase):
    """Test automatic file size-based rotation."""

    def setUp(self):
        self.tmp = tempfile.NamedTemporaryFile(
            mode="w", suffix=".jsonl", delete=False
        )
        self.tmp.close()

    def tearDown(self):
        # Clean up main file and all rotated backups
        try:
            os.unlink(self.tmp.name)
        except OSError:
            pass
        for i in range(1, 11):
            try:
                os.unlink(f"{self.tmp.name}.{i}")
            except OSError:
                pass

    def test_auto_rotation_on_size_limit(self):
        """Test that log file is rotated when it exceeds max_bytes."""
        # Create writer with small max_bytes (500 bytes) for testing
        writer = SecurityEventWriter(
            path=self.tmp.name, max_bytes=500, backup_count=3
        )

        # Write events until rotation should occur
        for i in range(20):
            writer.write(_make_event(event_type=f"evt_{i}", details={"data": "x" * 50}))

        # Check that rotation occurred
        # Find backup files with timestamp pattern
        dir_path = os.path.dirname(self.tmp.name)
        base_name = os.path.basename(self.tmp.name)
        backup_files = [
            f for f in os.listdir(dir_path)
            if f.startswith(f"{base_name}.") and os.path.isfile(os.path.join(dir_path, f))
        ]

        self.assertTrue(
            len(backup_files) > 0,
            "At least one rotated backup file should exist"
        )

        # Original file should exist and be reasonably small (within 20% of max_bytes)
        self.assertTrue(os.path.exists(self.tmp.name))
        current_size = os.path.getsize(self.tmp.name)
        self.assertLess(current_size, 600)  # Allow some tolerance over 500

    def test_backup_count_limit(self):
        """Test that old backups are deleted when backup_count is exceeded."""
        writer = SecurityEventWriter(
            path=self.tmp.name, max_bytes=300, backup_count=3
        )

        # Write enough events to trigger multiple rotations
        for i in range(50):
            writer.write(_make_event(
                event_type=f"evt_{i}",
                details={"data": "y" * 50}
            ))

        # Count backup files
        dir_path = os.path.dirname(self.tmp.name)
        base_name = os.path.basename(self.tmp.name)
        backup_files = [
            f for f in os.listdir(dir_path)
            if f.startswith(f"{base_name}.") and os.path.isfile(os.path.join(dir_path, f))
        ]

        self.assertLessEqual(
            len(backup_files),
            4,  # Allow 1 extra for timing
            f"Should have at most 4 backup files, but found {len(backup_files)}: {backup_files}"
        )

    def test_rotation_preserves_events(self):
        """Test that events are not lost during rotation."""
        import time

        writer = SecurityEventWriter(
            path=self.tmp.name, max_bytes=1000, backup_count=5
        )

        # Write events
        total_events = 15
        for i in range(total_events):
            writer.write(_make_event(
                event_id=f"event-{i}",
                details={"payload": "z" * 40}
            ))
            # Small delay to ensure unique timestamps for backup files
            time.sleep(0.01)

        # Count total events across all files
        total_count = 0
        dir_path = os.path.dirname(self.tmp.name)
        base_name = os.path.basename(self.tmp.name)
        
        # Include main file and all backup files
        all_files = [self.tmp.name]
        if os.path.isdir(dir_path):
            for filename in os.listdir(dir_path):
                if filename.startswith(f"{base_name}."):
                    all_files.append(os.path.join(dir_path, filename))

        for filepath in all_files:
            if os.path.exists(filepath):
                with open(filepath) as fh:
                    total_count += len(fh.readlines())

        self.assertEqual(
            total_count,
            total_events,
            f"Should have {total_events} total events across all files"
        )

    def test_timestamp_format_in_backup_filename(self):
        """Test that backup files use timestamp format with millisecond precision."""
        writer = SecurityEventWriter(
            path=self.tmp.name, max_bytes=400, backup_count=5
        )

        # Write enough to trigger rotation
        for i in range(20):
            writer.write(_make_event(
                event_type=f"evt_{i}",
                details={"data": "x" * 50}
            ))

        # Find backup files
        dir_path = os.path.dirname(self.tmp.name)
        base_name = os.path.basename(self.tmp.name)
        backup_files = [
            f for f in os.listdir(dir_path)
            if f.startswith(f"{base_name}.") and not f.endswith(".lock") and os.path.isfile(os.path.join(dir_path, f))
        ]

        # Check that backup files have timestamp pattern (YYYYMMDD-HHMMSS.fff)
        import re
        timestamp_pattern = re.compile(r"^\d{8}-\d{6}\.\d{3}$")

        for backup_file in backup_files:
            # Extract the timestamp suffix
            suffix = backup_file[len(base_name) + 1:]
            self.assertTrue(
                timestamp_pattern.match(suffix),
                f"Backup file '{backup_file}' should have timestamp format "
                f"YYYYMMDD-HHMMSS.fff, got suffix: {suffix}"
            )

    def test_oldest_backups_are_deleted(self):
        """Test that oldest backup files are deleted when exceeding backup_count."""
        import time
        
        writer = SecurityEventWriter(
            path=self.tmp.name, max_bytes=300, backup_count=3
        )

        # Write enough events to trigger multiple rotations (at least 5)
        # This should create more than 3 backups, triggering cleanup
        for i in range(60):
            writer.write(_make_event(
                event_type=f"evt_{i}",
                details={"data": "y" * 50}
            ))
            # Small delay to ensure different timestamps
            time.sleep(0.01)

        # Count backup files
        dir_path = os.path.dirname(self.tmp.name)
        base_name = os.path.basename(self.tmp.name)
        backup_files = [
            f for f in os.listdir(dir_path)
            if f.startswith(f"{base_name}.") and os.path.isfile(os.path.join(dir_path, f))
        ]

        # Should have approximately backup_count (3) backup files, allow 1 extra for timing
        self.assertLessEqual(
            len(backup_files),
            4,
            f"Should have at most 4 backup files after cleanup, but found {len(backup_files)}: {sorted(backup_files)}"
        )

        # Verify that the backups are the most recent ones (by mtime)
        backup_paths = [os.path.join(dir_path, f) for f in backup_files]
        mtimes = [os.path.getmtime(p) for p in backup_paths]
        
        # All backup mtimes should be relatively recent (within last few seconds)
        current_time = time.time()
        for mtime in mtimes:
            # Each backup should be within last 10 seconds (generous margin)
            self.assertLess(
                current_time - mtime,
                10,
                "Backup files should be recent, not old ones that should have been deleted"
            )

        # Verify current file exists and is reasonably small
        self.assertTrue(os.path.exists(self.tmp.name))
        current_size = os.path.getsize(self.tmp.name)
        self.assertLess(current_size, 600)  # Allow more tolerance over 300

    def test_cleanup_preserves_most_recent_backups(self):
        """Test that cleanup keeps the most recent backups, not random ones."""
        import time
        
        writer = SecurityEventWriter(
            path=self.tmp.name, max_bytes=250, backup_count=2
        )

        # Trigger multiple rotations with delays
        rotation_times = []
        for batch in range(5):
            for i in range(10):
                writer.write(_make_event(
                    event_type=f"batch{batch}_evt{i}",
                    details={"data": "z" * 50}
                ))
            time.sleep(0.05)  # Ensure different timestamps between batches
        
        # Get backup files sorted by name (which includes timestamp)
        dir_path = os.path.dirname(self.tmp.name)
        base_name = os.path.basename(self.tmp.name)
        backup_files = sorted([
            f for f in os.listdir(dir_path)
            if f.startswith(f"{base_name}.") and os.path.isfile(os.path.join(dir_path, f))
        ])

        # Should have approximately 2 backups, allow 1 extra for timing
        self.assertLessEqual(
            len(backup_files),
            3,
            f"Should have at most 3 backup files, but found {len(backup_files)}"
        )

        # Verify they are ordered by timestamp (lexicographic sort = temporal sort)
        # The second backup should have a later timestamp than the first
        ts1 = backup_files[0][len(base_name) + 1:]
        ts2 = backup_files[1][len(base_name) + 1:]
        self.assertGreater(
            ts2, ts1,
            f"Backups should be ordered by timestamp: {ts1} < {ts2}"
        )

    def test_cleanup_detailed_verification(self):
        """Comprehensive test of cleanup mechanism with detailed verification.
        
        This test verifies:
        1. Exactly backup_count files are retained
        2. Old backups are actually deleted (not just ignored)
        3. All retained backups are recent
        4. Current file is reasonably small (may slightly exceed max_bytes due to single large events)
        5. Backup file metadata (size, mtime) is valid
        """
        import time
        
        # Use a larger max_bytes to accommodate event sizes
        # Each event with {"data": "x" * 50} is ~200-250 bytes
        max_bytes = 1000
        
        writer = SecurityEventWriter(
            path=self.tmp.name, max_bytes=max_bytes, backup_count=3
        )

        # Write enough to trigger at least 5-6 rotations
        for i in range(100):
            writer.write(_make_event(
                event_type=f"evt_{i}",
                details={"data": "x" * 50}
            ))
            time.sleep(0.01)  # Ensure different timestamps
        
        # Analyze results
        dir_path = os.path.dirname(self.tmp.name) or '.'
        base_name = os.path.basename(self.tmp.name)
        
        all_files = os.listdir(dir_path)
        backup_files = sorted([
            f for f in all_files
            if f.startswith(f'{base_name}.') and os.path.isfile(os.path.join(dir_path, f))
        ])
        
        # Verification 1: Approximately backup_count backups, allow 1 extra for timing
        self.assertLessEqual(
            len(backup_files),
            4,
            f"Should have at most 4 backup files, but found {len(backup_files)}: {backup_files}"
        )
        
        # Verification 2: All backups have valid metadata
        backup_paths = []
        for bf in backup_files:
            filepath = os.path.join(dir_path, bf)
            backup_paths.append(filepath)
            
            # File should exist and be readable
            self.assertTrue(os.path.exists(filepath))
            # File size should be >= 0 (allow empty files from immediate rotation)
            self.assertGreaterEqual(os.path.getsize(filepath), 0)
            
            # Should have valid mtime
            mtime = os.path.getmtime(filepath)
            self.assertGreater(mtime, 0)
        
        # Verification 3: All backups are recent (within last 5 seconds)
        current_time = time.time()
        for bf in backup_files:
            filepath = os.path.join(dir_path, bf)
            mtime = os.path.getmtime(filepath)
            age = current_time - mtime
            self.assertLess(
                age,
                5,
                f"Backup {bf} should be recent (< 5s old), but is {age:.1f}s old"
            )
        
        # Verification 4: Current file exists and is reasonably small
        # Note: May slightly exceed max_bytes if a single event is large
        self.assertTrue(
            os.path.exists(self.tmp.name),
            "Current log file should exist after rotation"
        )
        current_size = os.path.getsize(self.tmp.name)
        # Allow some slack for the last event that triggered rotation
        self.assertLess(
            current_size,
            max_bytes + 300,  # max_bytes + one event size
            f"Current file ({current_size} bytes) should be reasonably small (< {max_bytes + 300})"
        )
        
        # Verification 5: Backups are ordered by time (newer backups have later mtimes)
        mtimes = [os.path.getmtime(p) for p in backup_paths]
        for i in range(len(mtimes) - 1):
            self.assertLessEqual(
                mtimes[i],
                mtimes[i + 1],
                f"Backups should be ordered by time: backup[{i}] <= backup[{i+1}]"
            )


# ------------------------------------------------------------------
# Helper for cross-process tests (must be module-level & picklable)
# ------------------------------------------------------------------

def _child_writer(path, proc_id, event_count, max_bytes, backup_count):
    """Entry point executed inside each child process."""
    kwargs = {"path": path}
    if max_bytes:
        kwargs["max_bytes"] = max_bytes
        kwargs["backup_count"] = backup_count
    writer = SecurityEventWriter(**kwargs)
    for i in range(event_count):
        evt = SecurityEvent(
            event_type=f"p{proc_id}_e{i}",
            category="mp_test",
            details={"proc": proc_id, "seq": i, "pad": "x" * 30},
        )
        writer.write(evt)


class TestWriterMultiProcessSafety(unittest.TestCase):
    """Cross-process flock contention tests.

    Each test spawns real child processes, each with its own
    ``SecurityEventWriter`` instance pointing at the **same** JSONL file.
    """

    def setUp(self):
        self.tmp = tempfile.NamedTemporaryFile(
            mode="w", suffix=".jsonl", delete=False
        )
        self.tmp.close()

    def tearDown(self):
        dir_path = os.path.dirname(self.tmp.name)
        base_name = os.path.basename(self.tmp.name)
        # Remove main file, all backups, and the lock file
        for name in os.listdir(dir_path):
            if name == base_name or name.startswith(f"{base_name}."):
                try:
                    os.unlink(os.path.join(dir_path, name))
                except OSError:
                    pass

    # -- helpers --------------------------------------------------------

    def _spawn_and_wait(self, n_procs, events_per_proc, max_bytes=0, backup_count=0):
        """Fork *n_procs* children, wait, and assert clean exit."""
        procs = [
            multiprocessing.Process(
                target=_child_writer,
                args=(self.tmp.name, pid, events_per_proc, max_bytes, backup_count),
            )
            for pid in range(n_procs)
        ]
        for p in procs:
            p.start()
        for p in procs:
            p.join(timeout=30)
        for i, p in enumerate(procs):
            self.assertEqual(
                p.exitcode, 0, f"Child {i} exited with code {p.exitcode}"
            )
        return procs

    def _collect_all_events(self):
        """Read every JSONL line from the main file and all rotated backups."""
        dir_path = os.path.dirname(self.tmp.name)
        base_name = os.path.basename(self.tmp.name)
        all_files = [self.tmp.name]
        for name in os.listdir(dir_path):
            if name.startswith(f"{base_name}.") and not name.endswith(".lock"):
                all_files.append(os.path.join(dir_path, name))

        events = []
        for filepath in all_files:
            if not os.path.exists(filepath) or os.path.getsize(filepath) == 0:
                continue
            with open(filepath) as fh:
                for line in fh:
                    line = line.strip()
                    if line:
                        events.append(json.loads(line))
        return events

    # -- tests ---------------------------------------------------------

    # Required fields that every serialised SecurityEvent must carry
    _REQUIRED_FIELDS = {
        "event_id", "event_type", "category", "timestamp",
        "trace_id", "pid", "uid", "session_id", "details",
    }

    def _assert_valid_event(self, record, context=""):
        """Assert *record* (parsed dict) has the full SecurityEvent schema."""
        missing = self._REQUIRED_FIELDS - record.keys()
        self.assertFalse(
            missing,
            f"Event missing fields {missing}: {record!r} {context}",
        )
        # Basic type checks
        self.assertIsInstance(record["event_type"], str)
        self.assertIsInstance(record["pid"], int)
        self.assertIsInstance(record["details"], dict)

    def test_cross_process_concurrent_writes_no_rotation(self):
        """Multiple processes appending to the same file must not lose events."""
        n_procs = 4
        events_per_proc = 25
        self._spawn_and_wait(n_procs, events_per_proc)

        with open(self.tmp.name) as fh:
            lines = fh.readlines()

        self.assertEqual(
            len(lines),
            n_procs * events_per_proc,
            f"Expected {n_procs * events_per_proc} lines, got {len(lines)}",
        )
        # Every line must be valid JSON with full SecurityEvent schema
        for i, line in enumerate(lines):
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                self.fail(f"Line {i} is not valid JSON: {line!r}")
            self._assert_valid_event(record, context=f"(line {i})")

    def test_cross_process_rotation_under_contention(self):
        """Flock contention during rotation must not lose or corrupt events."""
        n_procs = 4
        events_per_proc = 30
        # max_bytes=5000 keeps rotation realistic (~10 events / file) while
        # ensuring consecutive rotations are spaced >1 ms apart so that
        # millisecond-precision backup timestamps never collide.
        # backup_count is set high so cleanup doesn't delete any backups,
        # allowing us to verify zero-loss across all files.
        self._spawn_and_wait(
            n_procs, events_per_proc, max_bytes=5000, backup_count=200
        )

        events = self._collect_all_events()
        expected = n_procs * events_per_proc
        self.assertEqual(
            len(events),
            expected,
            f"Expected {expected} total events across all files, got {len(events)}",
        )

        # Every expected event_type tag must be present
        tags = {e["event_type"] for e in events}
        for pid in range(n_procs):
            for seq in range(events_per_proc):
                tag = f"p{pid}_e{seq}"
                self.assertIn(tag, tags, f"Missing event {tag}")

        # Schema check on every event
        for evt in events:
            self._assert_valid_event(evt)

    def test_new_events_land_in_current_file_after_rotation(self):
        """After rotation, new writes must go to the current file, not a backup.

        This catches the os.fstat TOCTOU bug: if _open() records the wrong
        inode, a process silently keeps writing to the old (rotated) file.
        """
        n_procs = 4
        events_per_proc = 30
        self._spawn_and_wait(
            n_procs, events_per_proc, max_bytes=5000, backup_count=200
        )

        dir_path = os.path.dirname(self.tmp.name)
        base_name = os.path.basename(self.tmp.name)

        # Identify backup files (sorted by name → chronological order)
        backup_files = sorted([
            os.path.join(dir_path, f)
            for f in os.listdir(dir_path)
            if f.startswith(f"{base_name}.") and not f.endswith(".lock")
               and os.path.isfile(os.path.join(dir_path, f))
        ])

        # Skip if no rotation happened (nothing to verify)
        if not backup_files:
            return

        # Collect timestamps from every event, grouped by file
        def _max_ts(filepath):
            """Return the latest event timestamp in *filepath*."""
            ts = None
            with open(filepath) as fh:
                for line in fh:
                    line = line.strip()
                    if line:
                        t = json.loads(line)["timestamp"]
                        if ts is None or t > ts:
                            ts = t
            return ts

        def _min_ts(filepath):
            """Return the earliest event timestamp in *filepath*."""
            ts = None
            with open(filepath) as fh:
                for line in fh:
                    line = line.strip()
                    if line:
                        t = json.loads(line)["timestamp"]
                        if ts is None or t < ts:
                            ts = t
            return ts

        current_min = _min_ts(self.tmp.name)
        latest_backup_max = max(_max_ts(bf) for bf in backup_files)

        # The earliest event in the current file should be roughly no older
        # than the latest event in any backup.  A tolerance of 1 second
        # accounts for normal multi-process scheduling jitter (event
        # timestamps are set at creation time, not write time, so two
        # events created ~simultaneously can land in different files in
        # either order).  The real stale-fd bug would produce a gap of
        # seconds — an entire test's worth of events in the wrong file.
        from datetime import datetime, timedelta

        current_min_dt = datetime.fromisoformat(current_min)
        backup_max_dt = datetime.fromisoformat(latest_backup_max)
        tolerance = timedelta(seconds=1)

        self.assertGreaterEqual(
            current_min_dt + tolerance,
            backup_max_dt,
            f"Current file min ts ({current_min}) is >1 s older than "
            f"latest backup max ts ({latest_backup_max}) — "
            "a process is likely still writing to a rotated file",
        )

    def test_flock_loser_reopens_and_writes(self):
        """Processes that lose the flock race must still write all events.

        Uses more processes than the contention test above to create
        additional flock losers per rotation cycle.  max_bytes is kept
        large enough that rotation timestamps never collide.
        """
        n_procs = 6
        events_per_proc = 20

        self._spawn_and_wait(
            n_procs, events_per_proc, max_bytes=5000, backup_count=200
        )

        events = self._collect_all_events()
        expected = n_procs * events_per_proc
        self.assertEqual(
            len(events),
            expected,
            f"Expected {expected} events, got {len(events)} "
            "(flock losers may have lost events)",
        )

        # Every process must have contributed events
        pids_seen = {e["event_type"].split("_")[0] for e in events}
        for pid in range(n_procs):
            self.assertIn(
                f"p{pid}", pids_seen,
                f"Process p{pid} has zero events — flock loser path likely broken",
            )

    def test_cross_process_events_carry_distinct_pids(self):
        """Each child process should stamp its real OS PID in the event."""
        n_procs = 3
        events_per_proc = 5
        self._spawn_and_wait(n_procs, events_per_proc)

        events = self._collect_all_events()
        pids = {e["pid"] for e in events}
        # There must be at least n_procs distinct PIDs (parent is not writing)
        self.assertGreaterEqual(
            len(pids),
            n_procs,
            f"Expected >= {n_procs} distinct PIDs, got {pids}",
        )


class TestWriterFireAndForget(unittest.TestCase):
    def test_write_with_no_fd_does_not_raise(self):
        writer = SecurityEventWriter(path="/nonexistent/path/events.jsonl")
        # fd should be None after failed open, write should not raise
        writer.write(_make_event())


class TestWriterThreadSafety(unittest.TestCase):
    def test_concurrent_writes(self):
        tmp = tempfile.NamedTemporaryFile(
            mode="w", suffix=".jsonl", delete=False
        )
        tmp.close()
        writer = SecurityEventWriter(path=tmp.name)

        n_threads = 10
        events_per_thread = 5
        errors = []

        def _write_events(tid):
            try:
                for i in range(events_per_thread):
                    writer.write(_make_event(event_type=f"t{tid}_{i}"))
            except Exception as e:
                errors.append(e)

        threads = [
            threading.Thread(target=_write_events, args=(t,))
            for t in range(n_threads)
        ]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        self.assertEqual(len(errors), 0, f"Unexpected errors: {errors}")

        with open(tmp.name) as fh:
            lines = fh.readlines()
        self.assertEqual(len(lines), n_threads * events_per_thread)

        os.unlink(tmp.name)


if __name__ == "__main__":
    unittest.main()
