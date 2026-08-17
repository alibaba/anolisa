#!/usr/bin/env bash
# Exercise the RPM spec scriptlets without building the RPM: the %post
# /etc/shells registration through the real RPM Lua interpreter and the
# %preun erase guard through fixture-backed bash runs.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SPEC="$ROOT/cosh-ng.spec.in"
TMP="$(mktemp -d /tmp/cosh-ng-rpm-scriptlet-test.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

# --- structural anchors: the lifecycle sections must stay in the spec ---
grep -q '^%preun$' "$SPEC"
grep -q '^%define cosh_replacement_ready ' "$SPEC"
grep -q '^Requires(preun): ' "$SPEC"
grep -Fxq \
    'Requires(preun):  (%{_bindir}/systemctl if systemd) /usr/bin/getent /usr/bin/awk' \
    "$SPEC"
grep -Fq "systemctl stop 'cosh-gateway@*.service'" "$SPEC"
grep -Fxq '%systemd_preun cosh-gateway@.service' "$SPEC"
grep -q '^%post -p <lua>$' "$SPEC"
# the extraction below slices on section boundaries, so the sections must
# keep their order: %preun, then %post, then %postun
awk '
    /^%preun$/ { a = NR }
    /^%post -p <lua>$/ { b = NR }
    /^%postun/ { c = NR }
    END { exit !(a && b && c && a < b && b < c) }
' "$SPEC"

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
    # the shared predicate must survive macro expansion with a queryformat
    # that emits a real newline (%%{NAME} folds to %{NAME}, \\n folds to \n)
    predicate_line="$(sed -n 's/^%define cosh_replacement_ready //p' "$SPEC")"
    expanded="$(rpm --define "cosh_replacement_ready $predicate_line" \
        --eval '%{cosh_replacement_ready}')"
    case "$expanded" in
        *"--qf '%{NAME}\n' -f"*) : ;;
        *)
            echo "ERROR: cosh_replacement_ready expanded unexpectedly: $expanded" >&2
            exit 1
            ;;
    esac

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

# --- %preun erase guard matrix through fixture-backed bash runs ---
STUB="$TMP/stub-bin"
install -d -m 0755 "$STUB"
GUARD_COSH="$STUB/cosh"
SYSTEMD_RUNTIME="$TMP/run/systemd/system"
SYSTEMCTL_LOG="$TMP/systemctl.log"
install -d -m 0755 "$SYSTEMD_RUNTIME"

PREDICATE="$(sed -n 's/^%define cosh_replacement_ready //p' "$SPEC")"
PREUN_RAW="$(awk '/^%preun$/{f=1;next} /^%post/{f=0} f' "$SPEC" |
    sed '/^%systemd_preun cosh-gateway@\.service$/d')"
# Bash 5.2 enables patsub_replacement by default, which would expand every
# unquoted '&' in the substituted predicate to the matched pattern text;
# keep the replacement strings verbatim.
shopt -u patsub_replacement 2>/dev/null || :
PREUN="${PREUN_RAW//'%{cosh_replacement_ready}'/$PREDICATE}"
PREUN="${PREUN//'%{_bindir}'/$STUB}"
PREUN="${PREUN//\/run\/systemd\/system/$SYSTEMD_RUNTIME}"
PREUN="${PREUN//%%/%}"
case "$PREUN" in
    *'%{cosh_replacement_ready}'*)
        echo "ERROR: %preun still references the unexpanded predicate macro" >&2
        exit 1
        ;;
esac
expected_predicate="${PREDICATE//'%{_bindir}'/$STUB}"
expected_predicate="${expected_predicate//%%/%}"
case "$PREUN" in
    *"$expected_predicate"*) : ;;
    *)
        echo "ERROR: predicate was not spliced verbatim into %preun" >&2
        exit 1
        ;;
esac

write_stub() {
    printf '%s\n' "#!/usr/bin/env bash" "$2" > "$STUB/$1"
    chmod 0755 "$STUB/$1"
}

run_preun() {
    local action="$1"
    PATH="$STUB:/usr/bin:/bin" bash -c "$PREUN" cosh-preun "$action"
}

expect_preun() {
    local name="$1"
    local action="$2"
    local expected_status="$3"
    local status=0

    run_preun "$action" >"$TMP/preun.out" 2>"$TMP/preun.err" || status=$?
    if [ "$status" -ne "$expected_status" ]; then
        echo "ERROR: %preun case '$name' exited $status, expected $expected_status:" >&2
        cat "$TMP/preun.err" >&2
        exit 1
    fi
}

write_stub getent "printf '%s\n' 'coshuser:x:1000:1000::/home/coshuser:$GUARD_COSH'"
write_stub rpm "printf '%s\n' cosh-ng"
write_stub cosh ":"
write_stub systemctl "printf '%s\n' \"\$*\" >> '$SYSTEMCTL_LOG'"

: > "$SYSTEMCTL_LOG"
expect_preun "erase with cosh login-shell user" 0 1
grep -Fq coshuser "$TMP/preun.err"
grep -Fq "$GUARD_COSH" "$TMP/preun.err"
test ! -s "$SYSTEMCTL_LOG"

expect_preun "upgrade never blocks" 1 0
test ! -s "$SYSTEMCTL_LOG"

write_stub getent "printf '%s\n' 'root:x:0:0:root:/root:/usr/bin/bash'"
expect_preun "erase without cosh users" 0 0
grep -Fxq 'stop cosh-gateway@*.service' "$SYSTEMCTL_LOG"

write_stub systemctl "printf '%s\n' \"\$*\" >> '$SYSTEMCTL_LOG'; exit 4"
expect_preun "failed Gateway instance stop" 0 1
grep -Fq 'failed to stop running Gateway instances' "$TMP/preun.err"

rmdir "$SYSTEMD_RUNTIME"
expect_preun "erase without a systemd manager" 0 0
install -d -m 0755 "$SYSTEMD_RUNTIME"
write_stub systemctl "printf '%s\n' \"\$*\" >> '$SYSTEMCTL_LOG'"

write_stub getent "exit 2"
expect_preun "failed passwd enumeration" 0 1
expect_preun "upgrade with broken enumeration" 1 0

write_stub getent "exit 0"
expect_preun "empty passwd enumeration" 0 1

write_stub getent "printf '%s\n' 'root:x:0:0:root:/root:/usr/bin/bash'"
write_stub awk "exit 3"
expect_preun "failed passwd filter" 0 1
rm -f "$STUB/awk"

write_stub getent "printf '%s\n' 'coshuser:x:1000:1000::/home/coshuser:$GUARD_COSH'"
write_stub rpm "exit 1"
expect_preun "failed replacement lookup" 0 1

write_stub rpm "printf '%s\n' cosh-ng unexpected-shell"
expect_preun "unexpected replacement owner" 0 1

write_stub rpm "printf '%s\n' cosh-ng copilot-shell"
chmod 0644 "$GUARD_COSH"
expect_preun "non-executable replacement" 0 1

chmod 0755 "$GUARD_COSH"
expect_preun "atomic provider swap" 0 0

rm -f "$GUARD_COSH"
expect_preun "upgrade without launcher" 1 0

echo "cosh-ng rpm scriptlet tests passed"
