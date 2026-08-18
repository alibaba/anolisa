# anolisa-tokenless

Self-contained CPython SDK for schema compression, RTK command rewriting, response compression,
TOON encoding, and marker-scoped Stash retrieval.

The package is built from the ANOLISA monorepo and supports CPython 3.11 or later on the platform
targeted by its wheel. The pinned RTK executable is included in the wheel; no Tokenless binary is
required on `PATH`. See the
[Tokenless user manual](https://github.com/alibaba/anolisa/blob/main/src/tokenless/README.md#build-the-python-runtime)
for source-build prerequisites, instructions, and API boundaries.
