#!/usr/bin/env bash
# ANOLISA `post_install` hook for the raw backend.
#
# Runs after a raw install or update has placed sec-core's files, and before
# `adapter status` / `AdapterClaim::bundle_match` re-derive an adapter's bundle
# digest. Its only job is the one-shot bytecode sweep: the hooks now launch
# with `python3 -B`, so no new caches appear, but `-B` does not stop CPython
# from importing bytecode already on disk. A host upgraded from a pre-`-B`
# release still carries those caches, and `remove_owned_files` leaves them
# alone because the install record never tracked them.
#
# Declared `strict = true` in the component contract: if the sweep fails, the
# install/update fails rather than recording a component whose adapters would
# then report drift for reasons nobody swept.
#
# Takes no arguments — ANOLISA invokes lifecycle hooks bare — so the datadir is
# derived from this script's own installed location
# (`<datadir>/hooks/sec-core/post-install.sh`).

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATADIR="$(cd "$HERE/../.." && pwd)"
SWEEPER="$HERE/clean-adapter-bytecode.sh"

if [ ! -x "$SWEEPER" ]; then
    printf 'post-install: missing bytecode sweeper at %s\n' "$SWEEPER" >&2
    exit 1
fi

# Exactly the raw adapter resource roots, matching the `dest` values in
# `[[adapters]]`. Roots that are absent are skipped by the sweeper: not every
# adapter is delivered on every host.
#
# Nothing outside these roots is touched. `{datadir}/skills` in particular is
# shared with other components (os-skills, ws-ckpt, …): sweeping it would
# delete bytecode sec-core does not own, and foreign non-bytecode content in
# one of its `__pycache__` directories would fail this strict hook and roll a
# sec-core update back for an unrelated component's files. Only the resource
# roots feed an `AdapterClaim` bundle digest, so only they matter for #2252.
exec "$SWEEPER" \
    "$DATADIR/adapters/sec-core/openclaw" \
    "$DATADIR/adapters/sec-core/hermes" \
    "$DATADIR/adapters/sec-core/codex" \
    "$DATADIR/adapters/sec-core/qoder" \
    "$DATADIR/adapters/sec-core/qwencode" \
    "$DATADIR/extensions/sec-core"
