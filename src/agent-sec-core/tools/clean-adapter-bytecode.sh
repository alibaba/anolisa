#!/usr/bin/env bash
# Remove CPython bytecode caches from agent-sec-core adapter resource roots.
#
# The security hooks launch with `python3 -B`, which stops the interpreter from
# *writing* new caches but does not stop it from *importing* a cache that is
# already on disk. A resource root still holding pre-`-B` bytecode would keep
# executing that bytecode, and would keep ANOLISA's adapter bundle digest away
# from the value recorded at enable time. This one-shot sweep is therefore part
# of the fix, not a cleanup nicety, and it must run before the bundle digest is
# recomputed — never from a status/report path, which stays read-only.
#
# Bounded by construction: within each root only directories named
# `__pycache__` are considered, and inside those only regular `*.pyc`/`*.pyo`
# files are deleted. A `__pycache__` still holding anything else (a foreign
# payload, a symlink pointing outside the bundle) is left in place and reported
# as a failure: that content is exactly what the bundle digest must keep
# flagging, so it must never be silently erased nor silently tolerated.
#
# Usage: clean-adapter-bytecode.sh <resource-root>...
#
# Exits 0 when every root is clean, 1 when any cache could not be fully
# removed, and 2 on usage errors — so an install/update transaction fails
# loudly instead of going on to record a healthy adapter over bytecode that
# nothing verified.

set -euo pipefail

PROGRAM_NAME="$(basename "$0")"

usage() {
    cat <<EOF
Usage: $PROGRAM_NAME <resource-root>...

Removes __pycache__ directories and the *.pyc/*.pyo files inside them from the
given agent-sec-core adapter resource roots. Roots that do not exist are
skipped: not every adapter is installed on every host.
EOF
}

note() {
    printf '%s: %s\n' "$PROGRAM_NAME" "$*"
}

warn() {
    printf '%s: %s\n' "$PROGRAM_NAME" "$*" >&2
}

# Sweep one resource root. Returns non-zero if any cache survived.
sweep_root() {
    local root="$1"
    local failures=0
    local cache_dir

    if [ -L "$root" ]; then
        warn "refusing to sweep '$root': resource root is a symbolic link"
        return 1
    fi
    if [ ! -d "$root" ]; then
        return 0
    fi

    # Enumerate into a file rather than `< <(find ...)`: a process
    # substitution's exit status is invisible to the loop and to `set -e`, so a
    # partial listing (EACCES on a subdirectory, EIO) would end the loop with
    # failures=0 and report success over caches that were never examined.
    # A command substitution would see the status but silently drops the NUL
    # separators `-print0` depends on, so a temp file is the only form that
    # keeps both properties.
    #
    # `-type d` matches the directory itself, never a symlink named
    # `__pycache__`, so a link cannot redirect the deletion out of the bundle.
    local listing="$WORK_DIR/listing"
    if ! find "$root" -type d -name __pycache__ -print0 >"$listing"; then
        warn "failed to enumerate '$root'; bytecode may remain"
        return 1
    fi

    while IFS= read -r -d '' cache_dir; do
        if ! find "$cache_dir" -maxdepth 1 -type f \
            \( -name '*.pyc' -o -name '*.pyo' \) -delete; then
            warn "failed to delete bytecode in '$cache_dir'"
            failures=$((failures + 1))
            continue
        fi
        # rmdir only succeeds on an empty directory, which is the check we
        # want: anything left behind was not interpreter-written bytecode.
        if ! rmdir "$cache_dir" 2>/dev/null; then
            warn "'$cache_dir' still holds non-bytecode content; left in place for bundle-digest review"
            failures=$((failures + 1))
            continue
        fi
        note "removed $cache_dir"
    done <"$listing"

    [ "$failures" -eq 0 ]
}

if [ "$#" -lt 1 ]; then
    usage >&2
    exit 2
fi

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

case "$1" in
    -h | --help)
        usage
        exit 0
        ;;
esac

status=0
for resource_root in "$@"; do
    if ! sweep_root "$resource_root"; then
        status=1
    fi
done
exit "$status"
