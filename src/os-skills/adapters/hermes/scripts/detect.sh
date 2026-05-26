#!/usr/bin/env bash
# detect.sh — Inspect os-skills Hermes integration. Read-only.
#
# Reports hermes CLI, Hermes home/skills directory, and per-skill presence.
# Exits 0 when ready, 1 when not installed but installable, and 2 when
# prerequisites are missing.
set -euo pipefail

COMPONENT="${ANOLISA_COMPONENT:-os-skills}"
AGENT="${ANOLISA_TARGET:-hermes}"
PROJECT_ROOT="${ANOLISA_PROJECT_ROOT:-}"
TARGET_DIR="${ANOLISA_TARGET_DIR:-}"
HERMES_HOME="${HERMES_HOME:-$HOME/.hermes}"
HERMES_BIN="${HERMES_BIN:-}"
HERMES_SKILLS_DIR="${HERMES_SKILLS_DIR:-${HERMES_HOME%/}/skills}"
export PATH="$HOME/.local/bin:${HERMES_HOME%/}/bin:/usr/local/bin:$PATH"

OS_SKILLS=(
    copaw-usage
    install-claude-code
    install-copaw
    install-hermes
    install-openclaw
    setup-mcp
    aliyun-ecs
    github
    kernel-dev
    sysom-agentsight
    sysom-diagnosis
    clawhub-skill-mng
    cosh-guide
    humanizer
    image-gen
    pdf-reader
    xlsx
    alinux-cve-query
    alinux-admin
    backup-restore
    regex-mastery
    shell-scripting
    storage-resize
    upgrade-alinux-kernel
)

line()  { printf '[%s] %s\n' "$COMPONENT" "$*"; }
field() { printf '[%s]   %-26s %s\n' "$COMPONENT" "$1" "$2"; }

PREREQ_MISSING=()
INSTALL_MISSING=()
note_prereq_missing() { PREREQ_MISSING+=("$1"); }
note_install_missing() { INSTALL_MISSING+=("$1"); }

if [ -z "$HERMES_BIN" ]; then
    HERMES_BIN="$(command -v hermes 2>/dev/null || true)"
fi

line "${AGENT} detect"
if [ -n "$HERMES_BIN" ] && [ -x "$HERMES_BIN" ]; then
    field "hermes CLI" "present (${HERMES_BIN})"
else
    field "hermes CLI" "missing"
    note_prereq_missing "hermes CLI"
fi

if [ -d "$HERMES_HOME" ]; then
    field "hermes home" "present (${HERMES_HOME})"
else
    field "hermes home" "not installed (${HERMES_HOME})"
    note_install_missing "hermes home"
fi

if [ -d "$HERMES_SKILLS_DIR" ]; then
    field "skills dir" "present (${HERMES_SKILLS_DIR})"
else
    field "skills dir" "not installed (${HERMES_SKILLS_DIR})"
    note_install_missing "skills dir"
fi

# Adapter source resources — informational only.
adapter_sources=()
[ -n "$TARGET_DIR" ]  && adapter_sources+=("$TARGET_DIR/share/anolisa/skills")
[ -n "$PROJECT_ROOT" ] && adapter_sources+=("$PROJECT_ROOT/src/os-skills")
adapter_sources+=(
    "$HOME/.copilot-shell/skills"
    "$HOME/.local/share/anolisa/skills"
    "/usr/share/anolisa/skills"
)
adapter_resource="-"
for cand in "${adapter_sources[@]}"; do
    [ -n "$cand" ] && [ -d "$cand" ] || continue
    if [ -f "$cand/install-hermes/SKILL.md" ]; then
        adapter_resource="$cand"
        break
    fi
    found="$(find "$cand" -path "*/install-hermes/SKILL.md" -type f -print -quit)"
    if [ -n "$found" ]; then
        adapter_resource="$cand"
        break
    fi
done
field "adapter resources" "$adapter_resource"
if [ "$adapter_resource" = "-" ]; then
    note_prereq_missing "adapter resources"
fi

present=0
missing_skills=()
for s in "${OS_SKILLS[@]}"; do
    if [ -f "${HERMES_SKILLS_DIR%/}/$s/SKILL.md" ]; then
        present=$((present + 1))
    else
        missing_skills+=("$s")
    fi
done
total=${#OS_SKILLS[@]}
field "skills installed" "${present}/${total}"
if [ ${#missing_skills[@]} -gt 0 ]; then
    line "missing skills: ${missing_skills[*]}"
    note_install_missing "skills"
fi

if [ ${#PREREQ_MISSING[@]} -gt 0 ]; then
    line "${AGENT}: missing prerequisites (${PREREQ_MISSING[*]})"
    exit 2
fi
if [ ${#INSTALL_MISSING[@]} -gt 0 ]; then
    line "${AGENT}: not installed (ready to install)"
    exit 1
fi
line "${AGENT}: ready"
exit 0
