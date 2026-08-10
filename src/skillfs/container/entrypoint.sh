#!/usr/bin/env bash
#
# SkillFS sidecar entrypoint.
#
# Runs the preflight checks, then `exec`s SkillFS so the FUSE process replaces
# this shell as PID 1. That is what makes the kubelet's SIGTERM land directly on
# SkillFS, which unmounts cleanly on its way out.
#
# There is deliberately no `sleep infinity`, no retry loop, and no trap that
# keeps the container alive after SkillFS exits: a dead mount must surface as a
# container restart, not as a healthy container serving nothing.
#
# Configuration (environment):
#   SKILLFS_SOURCE      absolute path to the writable skill source root
#   SKILLFS_MOUNTPOINT  absolute path where the FUSE view is exposed
#   SKILLFS_DISCOVER_ROOT  reader-visible skills root advertised by discover
#   SKILLFS_EXTRA_ARGS  extra `skillfs mount` arguments, whitespace separated
#   SKILLFS_SKIP_PREFLIGHT  set to 1 to skip preflight (debugging only)
#   RUST_LOG            SkillFS log filter, defaults to info
#
# Any argument passed to the container image is treated as a full command and
# executed instead of the mount, which keeps `kubectl debug`-style overrides
# possible without a second image.

set -uo pipefail

log() {
	printf '[skillfs-entrypoint] %s\n' "$*" >&2
}

# Escape hatch for debugging: `command: ["/usr/local/bin/skillfs-sidecar-entrypoint"]`
# plus `args: ["sleep", "3600"]` runs that command instead of the mount.
if (($# > 0)); then
	log "explicit command override, exec: $*"
	exec "$@"
fi

SKILLFS_BIN="${SKILLFS_BIN:-/usr/local/bin/skillfs}"
SOURCE_DIR="${SKILLFS_SOURCE:-}"
MOUNTPOINT="${SKILLFS_MOUNTPOINT:-}"
DISCOVER_ROOT="${SKILLFS_DISCOVER_ROOT:-}"
EXTRA_ARGS="${SKILLFS_EXTRA_ARGS:-}"
export RUST_LOG="${RUST_LOG:-info}"

if [[ ! -x "$SKILLFS_BIN" ]]; then
	log "FAIL: SkillFS binary '$SKILLFS_BIN' is missing or not executable"
	exit 127
fi

log "skillfs version: $("$SKILLFS_BIN" --version 2>&1 | head -1)"
log "uid=$(id -u) gid=$(id -g) source=${SOURCE_DIR:-<unset>} mountpoint=${MOUNTPOINT:-<unset>}"

if [[ "${SKILLFS_SKIP_PREFLIGHT:-0}" == "1" ]]; then
	log "WARNING: SKILLFS_SKIP_PREFLIGHT=1, skipping prerequisite validation"
else
	/usr/local/bin/skillfs-preflight
	preflight_status=$?
	if ((preflight_status != 0)); then
		log "FAIL: preflight exited $preflight_status; not starting the mount"
		exit "$preflight_status"
	fi
fi

# Word splitting on SKILLFS_EXTRA_ARGS is intentional: the value is an argument
# list, not a single argument.
declare -a extra=()
if [[ -n "$EXTRA_ARGS" ]]; then
	read -r -a extra <<<"$EXTRA_ARGS"
fi

# `--foreground` keeps SkillFS as the container's main process so kubelet owns
# restart; `--managed` and its detached supervisor must not be used here.
# `--allow-other` is what lets the Agent container's UID reach the propagated
# view. Both flags are fixed by the sidecar contract and are not configurable
# through SKILLFS_EXTRA_ARGS.
declare -a cmd=(
	"$SKILLFS_BIN" mount "$SOURCE_DIR" "$MOUNTPOINT"
	--foreground
	--allow-other
)
if [[ -n "$DISCOVER_ROOT" ]]; then
	cmd+=(--skill-discover-root "$DISCOVER_ROOT")
fi
if ((${#extra[@]} > 0)); then
	cmd+=("${extra[@]}")
fi

log "exec: ${cmd[*]}"
exec "${cmd[@]}"
