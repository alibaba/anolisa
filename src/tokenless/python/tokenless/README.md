# anolisa-tokenless

Self-contained CPython SDK for schema compression, RTK command rewriting, response compression,
TOON encoding, and marker-scoped Stash retrieval.

The package is built from the ANOLISA monorepo and supports CPython 3.11 or later on the platform
targeted by its wheel. The pinned RTK executable is included in the wheel; no Tokenless binary is
required on `PATH`. See the
[Tokenless user manual](https://github.com/alibaba/anolisa/blob/main/src/tokenless/README.md#build-the-python-runtime)
for source-build prerequisites, instructions, and API boundaries.

The public `TokenlessStats` client provides typed, read-only status, summary, recent-record,
record-detail, structured-diff, and session-comparison queries over the Runtime's `stats.db`.
Token counts are estimates and only operations with positive savings are recorded. Record details
and detailed diffs can contain sensitive stored tool content. Read-only describes the API surface:
opening the client follows CLI initialization and may create or migrate `stats.db`, so the data
directory must be writable. `limit=None` for summary or comparison reads at most the newest 10,000
records. Session and tool-use diffs also read at most the newest 10,000 matching records;
comparisons should pass a dry-run session before an active Tokenless session.
