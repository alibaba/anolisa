#!/usr/bin/env bash
# Build the SkillFS workspace.
#
# Usage:
#   scripts/build.sh             # debug build  -> target/debug/skillfs
#   scripts/build.sh --release   # release build -> target/release/skillfs

set -euo pipefail

PROFILE="debug"
CARGO_FLAGS=""
for arg in "$@"; do
	case "$arg" in
		--release)
			PROFILE="release"
			CARGO_FLAGS="--release"
			;;
		-h|--help)
			sed -n '2,6p' "$0"
			exit 0
			;;
		*)
			echo "Unknown argument: $arg" >&2
			exit 1
			;;
	esac
done

cd "$(dirname "$0")/.."

echo "=== Building SkillFS ($PROFILE) ==="
cargo build --workspace $CARGO_FLAGS

echo
echo "Build complete. Binary at: target/$PROFILE/skillfs"
