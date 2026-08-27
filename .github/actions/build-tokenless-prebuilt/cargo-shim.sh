#!/usr/bin/env bash
# Keep Maturin metadata on the host and route compilation through the release Cross profile.
set -euo pipefail

die() {
    printf 'ERROR: %s\n' "$*" >&2
    exit 1
}

HOST_CARGO="${TOKENLESS_HOST_CARGO:?TOKENLESS_HOST_CARGO is required}"
CROSS_PROFILE_SCRIPT="${TOKENLESS_CROSS_PROFILE_SCRIPT:?TOKENLESS_CROSS_PROFILE_SCRIPT is required}"
PROFILE="${TOKENLESS_CROSS_PROFILE:?TOKENLESS_CROSS_PROFILE is required}"
EXPECTED_TARGET="${TOKENLESS_RUST_TARGET:?TOKENLESS_RUST_TARGET is required}"

case "$PROFILE/$EXPECTED_TARGET" in
    gnu2.17-x86_64/x86_64-unknown-linux-gnu | \
    gnu2.17-aarch64/aarch64-unknown-linux-gnu | \
    darwin11-aarch64/aarch64-apple-darwin) ;;
    *) die "Cross profile $PROFILE does not match target $EXPECTED_TARGET" ;;
esac
[ -x "$HOST_CARGO" ] || die "host Cargo is not executable: $HOST_CARGO"
[ -x "$CROSS_PROFILE_SCRIPT" ] || \
    die "Cross profile script is not executable: $CROSS_PROFILE_SCRIPT"

case "${1:-}" in
    metadata | locate-project | --version | -V)
        exec "$HOST_CARGO" "$@"
        ;;
    build | rustc)
        command_name="$1"
        shift
        arguments=()
        while [ "$#" -gt 0 ]; do
            case "$1" in
                --target)
                    [ "$#" -ge 2 ] || die '--target requires a value'
                    [ "$2" = "$EXPECTED_TARGET" ] || \
                        die "Maturin requested target $2, expected $EXPECTED_TARGET"
                    shift 2
                    ;;
                --target=*)
                    requested_target="${1#--target=}"
                    [ "$requested_target" = "$EXPECTED_TARGET" ] || \
                        die "Maturin requested target $requested_target, expected $EXPECTED_TARGET"
                    shift
                    ;;
                *)
                    arguments+=("$1")
                    shift
                    ;;
            esac
        done
        exec "$CROSS_PROFILE_SCRIPT" "$PROFILE" "$command_name" "${arguments[@]}"
        ;;
    *)
        die "unsupported Maturin Cargo command: ${1:-<empty>}"
        ;;
esac
