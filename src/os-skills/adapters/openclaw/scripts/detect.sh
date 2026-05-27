#!/usr/bin/env bash
# detect.sh — Inspect os-skills OpenClaw integration. Read-only.
#
# Reports OpenClaw CLI, skills directory, and per-skill presence. Exits 0
# when every expected skill is installed under the OpenClaw skills dir, and
# non-zero otherwise (e.g. OpenClaw missing or skills not yet installed).
set -euo pipefail

COMPONENT="${ANOLISA_COMPONENT:-os-skills}"
AGENT="${ANOLISA_TARGET:-openclaw}"
ADAPTER_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"
PROJECT_ROOT="${ANOLISA_PROJECT_ROOT:-}"
TARGET_DIR="${ANOLISA_TARGET_DIR:-}"
OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$OPENCLAW_HOME}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR%/}"
OPENCLAW_HOME="${OPENCLAW_HOME%/}"
OPENCLAW_BIN="${OPENCLAW_BIN:-}"
OPENCLAW_SKILLS_DIR="${OPENCLAW_SKILLS_DIR:-${OPENCLAW_STATE_DIR%/}/skills}"
export PATH="$HOME/.local/bin:${OPENCLAW_STATE_DIR%/}/bin:/usr/local/bin:$PATH"

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

if [ -z "$OPENCLAW_BIN" ]; then
    OPENCLAW_BIN="$(command -v openclaw 2>/dev/null || true)"
fi

line "${AGENT} detect"
if [ -n "$OPENCLAW_BIN" ] && [ -x "$OPENCLAW_BIN" ]; then
    field "openclaw CLI" "present (${OPENCLAW_BIN})"
else
    field "openclaw CLI" "missing"
    note_prereq_missing "openclaw CLI"
fi

if [ -d "$OPENCLAW_STATE_DIR" ]; then
    field "openclaw home" "present (${OPENCLAW_STATE_DIR})"
else
    field "openclaw home" "not installed (${OPENCLAW_STATE_DIR})"
    note_install_missing "openclaw home"
fi

if [ -d "$OPENCLAW_SKILLS_DIR" ]; then
    field "skills dir" "present (${OPENCLAW_SKILLS_DIR})"
else
    field "skills dir" "not installed (${OPENCLAW_SKILLS_DIR})"
    note_install_missing "skills dir"
fi

# Adapter source resources (informational only — install path may differ when
# the component was installed from RPM rather than the source checkout).
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
    if [ -f "$cand/install-openclaw/SKILL.md" ]; then
        adapter_resource="$cand"
        break
    fi
    found="$(find "$cand" -path "*/install-openclaw/SKILL.md" -type f -print -quit)"
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
    if [ -f "${OPENCLAW_SKILLS_DIR%/}/$s/SKILL.md" ]; then
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
