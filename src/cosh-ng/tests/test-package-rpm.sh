#!/usr/bin/env bash
# Exercise the RPM spec %post /etc/shells registration through the real
# RPM Lua interpreter without building the RPM.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SPEC="$ROOT/cosh-ng.spec.in"
TMP="$(mktemp -d /tmp/cosh-ng-rpm-scriptlet-test.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

# --- structural anchors: the lifecycle sections must stay in the spec ---
grep -q '^%post -p <lua>$' "$SPEC"

# --- %post registration matrix through the real RPM Lua interpreter ---
SHELLS="$TMP/shells"
COSH="$TMP/cosh"

post_script() {
    awk '/^%post -p <lua>$/{f=1;next} /^%/{f=0} f' "$SPEC" |
        sed -e "s|/etc/shells|$SHELLS|g" -e "s|%{_bindir}/cosh|$COSH|g"
}

POST_SCRIPT="$(post_script)"
# the substitutions must have taken effect: a silent sed no-op would make
# the scriptlet below run against the real /etc/shells of this machine
case "$POST_SCRIPT" in
    *"$SHELLS"*) : ;;
    *)
        echo "ERROR: shells path substitution missed the %post scriptlet" >&2
        exit 1
        ;;
esac
case "$POST_SCRIPT" in
    *"$COSH"*) : ;;
    *)
        echo "ERROR: registration path substitution missed the %post scriptlet" >&2
        exit 1
        ;;
esac
case "$POST_SCRIPT" in
    *'/etc/shells'* | *'%{_bindir}/cosh'*)
        echo "ERROR: %post scriptlet still references packaged paths" >&2
        exit 1
        ;;
esac

run_post() {
    rpm --eval "%{lua:$POST_SCRIPT}" >/dev/null
}

expect_shells() {
    local name="$1"
    printf '%s' "$2" > "$TMP/expected"
    if ! cmp -s "$TMP/expected" "$SHELLS"; then
        echo "ERROR: %post case '$name' produced unexpected bytes:" >&2
        od -c "$SHELLS" >&2
        exit 1
    fi
}

run_post_case() {
    local name="$1"
    local initial="$2"
    local expected="$3"

    if [ "$initial" = "<missing>" ]; then
        rm -f "$SHELLS"
    else
        printf '%s' "$initial" > "$SHELLS"
    fi
    run_post
    expect_shells "$name (install)" "$expected"
    run_post
    expect_shells "$name (reinstall)" "$expected"
}

if command -v rpm >/dev/null 2>&1 && rpm --eval '%{lua:print("ok")}' >/dev/null 2>&1; then
    run_post_case "missing file" "<missing>" "$COSH"$'\n'
    run_post_case "empty file" "" "$COSH"$'\n'
    run_post_case "missing trailing newline" \
        $'/bin/sh\n/bin/bash' \
        $'/bin/sh\n/bin/bash\n'"$COSH"$'\n'
    run_post_case "existing trailing newline" \
        $'/usr/bin/bash\n' \
        $'/usr/bin/bash\n'"$COSH"$'\n'
    run_post_case "existing exact registration" \
        $'/usr/bin/bash\n'"$COSH"$'\n/usr/bin/zsh\n' \
        $'/usr/bin/bash\n'"$COSH"$'\n/usr/bin/zsh\n'
    run_post_case "duplicate registrations preserved" \
        $'/usr/bin/bash\n'"$COSH"$'\n'"$COSH"$'\n' \
        $'/usr/bin/bash\n'"$COSH"$'\n'"$COSH"$'\n'
    run_post_case "substring is not a registration" \
        $'/usr/bin/bash\n'"$COSH"$'-backup\n' \
        $'/usr/bin/bash\n'"$COSH"$'-backup\n'"$COSH"$'\n'

    # registration stays fail-open when the shells file cannot be opened
    rm -f "$SHELLS"
    rpm --eval "%{lua:io.open = function() return nil, 'Read-only file system' end
$(post_script)}" >/dev/null
    if [ -e "$SHELLS" ]; then
        echo "ERROR: fail-open %post unexpectedly touched the shells file" >&2
        exit 1
    fi
else
    echo "SKIP: rpm lua interpreter unavailable; %post matrix not exercised" >&2
fi

echo "cosh-ng rpm scriptlet tests passed"
