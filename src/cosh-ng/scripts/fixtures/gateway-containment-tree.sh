#!/usr/bin/env bash
# Process-tree fixture for the isolated Gateway systemd containment test.
set -euo pipefail

usage() {
    printf 'usage: %s {assert-clean|launch|leaf|spawn-grandchild|double-fork} ...\n' "$0" >&2
    exit 64
}

process_start_time() {
    local pid="$1" suffix

    [ -r "/proc/$pid/stat" ] || return 1
    IFS= read -r suffix < "/proc/$pid/stat"
    suffix="${suffix##*) }"
    # starttime is field 22, or field 20 after pid and comm are removed.
    set -- $suffix
    [ "$#" -ge 20 ] || return 1
    printf '%s\n' "${20}"
}

same_process_is_alive() {
    local pid="$1" expected="$2" actual

    actual="$(process_start_time "$pid" 2>/dev/null)" || return 1
    [ "$actual" = "$expected" ]
}

current_cgroup() {
    local hierarchy controllers path

    while IFS=: read -r hierarchy controllers path; do
        if [ "$hierarchy" = "0" ] && [ -z "$controllers" ]; then
            case "$path" in
                /*) printf '%s\n' "$path"; return 0 ;;
            esac
        fi
    done < "/proc/$BASHPID/cgroup"
    return 1
}

assert_clean() {
    local state="$1" generation next identity role pid start cgroup member

    generation=0
    if [ -f "$state/generation" ]; then
        IFS= read -r generation < "$state/generation"
    fi
    case "$generation" in
        '' | *[!0-9]*) exit 65 ;;
    esac
    next=$((generation + 1))

    if [ -d "$state/old" ]; then
        for identity in "$state"/old/*.identity; do
            [ -e "$identity" ] || continue
            IFS=: read -r role pid start < "$identity"
            if same_process_is_alive "$pid" "$start"; then
                printf 'stale %s process remains alive: pid=%s\n' "$role" "$pid" >&2
                exit 70
            fi
        done
    fi

    cgroup="$(current_cgroup)" || exit 71
    [ "$cgroup" != "/" ] || exit 71
    while IFS= read -r member; do
        [ -n "$member" ] || continue
        if [ "$member" != "$BASHPID" ]; then
            printf 'old cgroup is not empty before restart: pid=%s\n' "$member" >&2
            exit 72
        fi
    done < "/sys/fs/cgroup${cgroup}/cgroup.procs"

    : > "$state/preclean.$next"
}

leaf() {
    local pid_file="$1" signal_mode="$2"

    printf '%s\n' "$BASHPID" > "$pid_file"
    if [ "$signal_mode" = "ignore-term" ]; then
        trap '' TERM
    fi
    while :; do
        sleep 60 &
        wait "$!" || true
    done
}

spawn_grandchild() {
    local pid_file="$1"

    "$0" leaf "$pid_file" ignore-term &
    wait "$!"
}

double_fork() {
    local pid_file="$1"

    python3 - "$0" "$pid_file" <<'PY'
import os
import sys

fixture, pid_file = sys.argv[1:]
pid = os.fork()
if pid > 0:
    os.waitpid(pid, 0)
    raise SystemExit(0)
os.setsid()
pid = os.fork()
if pid > 0:
    os._exit(0)
os.execv(fixture, [fixture, "leaf", pid_file, "ignore-term"])
PY
}

wait_for_pid_file() {
    local path="$1" attempt

    for attempt in $(seq 1 100); do
        if [ -s "$path" ]; then
            return 0
        fi
        sleep 0.05
    done
    printf 'timed out waiting for pid file: %s\n' "$path" >&2
    return 1
}

record_identity() {
    local state="$1" role="$2" pid start

    IFS= read -r pid < "$state/pids/$role"
    start="$(process_start_time "$pid")"
    printf '%s:%s:%s\n' "$role" "$pid" "$start" > "$state/old/$role.identity"
}

try_escape() {
    local state="$1" token="$2" generation="$3" scope="$4" unit

    unit="cosh-gateway-containment-${token}-escape-${scope}-${generation}"
    if [ "$scope" = "user" ]; then
        if timeout 5s systemd-run --user --quiet --unit "$unit" /bin/sleep 300; then
            : > "$state/escape-succeeded.$scope.$generation"
            return 1
        fi
    elif timeout 5s systemd-run --system --quiet --unit "$unit" /bin/sleep 300; then
        : > "$state/escape-succeeded.$scope.$generation"
        return 1
    fi
}

launch() {
    local state="$1" token="$2" generation role

    mkdir -p "$state/pids" "$state/old"
    generation=0
    if [ -f "$state/generation" ]; then
        IFS= read -r generation < "$state/generation"
    fi
    case "$generation" in
        '' | *[!0-9]*) exit 65 ;;
    esac
    generation=$((generation + 1))
    [ -f "$state/preclean.$generation" ] || exit 73
    printf '%s\n' "$generation" > "$state/generation"
    rm -f -- "$state"/pids/*

    try_escape "$state" "$token" "$generation" user || exit 74
    try_escape "$state" "$token" "$generation" system || exit 75
    : > "$state/escape-denied.$generation"

    printf '%s\n' "$BASHPID" > "$state/pids/main"
    "$0" leaf "$state/pids/direct" normal &
    "$0" spawn-grandchild "$state/pids/grandchild" &
    "$0" double-fork "$state/pids/double-fork"
    setsid -f "$0" leaf "$state/pids/setsid" ignore-term

    for role in direct grandchild double-fork setsid; do
        wait_for_pid_file "$state/pids/$role"
    done
    if [ "$generation" -eq 1 ]; then
        record_identity "$state" main
        for role in direct grandchild double-fork setsid; do
            record_identity "$state" "$role"
        done
    fi
    : > "$state/ready.$generation"
    wait
}

case "${1:-}" in
    assert-clean)
        [ "$#" -eq 2 ] || usage
        assert_clean "$2"
        ;;
    launch)
        [ "$#" -eq 3 ] || usage
        launch "$2" "$3"
        ;;
    leaf)
        [ "$#" -eq 3 ] || usage
        leaf "$2" "$3"
        ;;
    spawn-grandchild)
        [ "$#" -eq 2 ] || usage
        spawn_grandchild "$2"
        ;;
    double-fork)
        [ "$#" -eq 2 ] || usage
        double_fork "$2"
        ;;
    *) usage ;;
esac
