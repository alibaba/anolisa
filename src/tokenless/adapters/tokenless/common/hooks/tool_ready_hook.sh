#!/usr/bin/env bash
# tokenless-hook-version: 9
# Token-Less Tool Ready environment pre-check.
#
# Hook event: PreToolUse (matcher: "" — matches all tools)
# Requires: jq
#
# Design: fail-open. If jq is missing or any phase fails, exit 0 silently.
#
# Four-Phase Flow:
#   Phase 1 — LOOKUP:   Find tool in config dictionary. Not found → skip.
#   Phase 2 — CHECK:    Scan system readiness. All ready → continue silently.
#   Phase 3 — FIX:      Auto-install missing deps. Success → continue silently.
#   Phase 4 — FEEDBACK: Fix failed. Inject additionalContext → "Skip retry".

set -euo pipefail

VERBOSE="${TOKENLESS_VERBOSE:-}"
log_v() { [ -n "$VERBOSE" ] && echo "[tokenless:ready] $1" >&2 || true; }

# --- Dependency check (fail-open) ---
if ! command -v jq &>/dev/null; then log_v "jq not found, skipping"; exit 0; fi

# --- File trust validation ---
# User-writable paths must be owned by current user and not world-writable.
is_trusted_file() {
  local f="$1"
  [ -f "$f" ] || return 1
  # System paths are always trusted
  case "$f" in /usr/share/*|/usr/libexec/*|/usr/lib/anolisa/*|/usr/local/share/*) return 0 ;; esac
  # Resolve symlink target before owner/perm checks
  local check_path="$f"
  if [ -L "$f" ]; then
    local target
    target=$(readlink -f "$f" 2>/dev/null || realpath "$f" 2>/dev/null || echo "")
    # System targets are always trusted
    case "$target" in /usr/share/*|/usr/libexec/*|/usr/lib/anolisa/*|/usr/local/share/*) return 0 ;; esac
    [ -z "$target" ] && return 1
    check_path="$target"
  fi
  local file_owner
  file_owner=$(stat -c '%u' "$check_path" 2>/dev/null || stat -f '%u' "$check_path" 2>/dev/null || echo "-1")
  if [ "$file_owner" != "$(id -u)" ] && [ "$file_owner" != "0" ]; then
    log_v "BLOCKED: $f owned by uid $file_owner (expected $(id -u) or 0)"
    return 1
  fi
  local file_perms
  file_perms=$(stat -c '%a' "$check_path" 2>/dev/null || stat -f '%Lp' "$check_path" 2>/dev/null || echo "777")
  local other_perms="${file_perms: -1}"
  if [ "$other_perms" -ge 6 ] 2>/dev/null; then
    log_v "BLOCKED: $f is world-writable (perms=$file_perms)"
    return 1
  fi
  return 0
}

# --- Resolve paths ---
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

SPEC_FILE=""
for candidate in \
    "${TOKENLESS_TOOL_READY_SPEC:-}" \
    "${ANOLISA_ADAPTER_DIR:+$ANOLISA_ADAPTER_DIR/common/tool-ready-spec.json}" \
    "$HOME/.local/share/anolisa/adapters/tokenless/common/tool-ready-spec.json" \
    "/usr/share/anolisa/adapters/tokenless/common/tool-ready-spec.json" \
    "$HOME/.tokenless/tool-ready-spec.json" \
    "${SCRIPT_DIR}/../tool-ready-spec.json"; do
    if [ -n "$candidate" ] && is_trusted_file "$candidate"; then
        SPEC_FILE="$candidate"
        break
    fi
done

FIX_SCRIPT=""
for candidate in \
    "${TOKENLESS_ENV_FIX_SCRIPT:-}" \
    "${ANOLISA_ADAPTER_DIR:+$ANOLISA_ADAPTER_DIR/common/tokenless-env-fix.sh}" \
    "$HOME/.local/share/anolisa/adapters/tokenless/common/tokenless-env-fix.sh" \
    "/usr/share/anolisa/adapters/tokenless/common/tokenless-env-fix.sh" \
    "$HOME/.tokenless/tokenless-env-fix.sh" \
    "${SCRIPT_DIR}/../tokenless-env-fix.sh"; do
    if [ -n "$candidate" ] && [ -x "$candidate" ] && is_trusted_file "$candidate"; then
        FIX_SCRIPT="$candidate"
        break
    fi
done

# --- Read input (fail-open) ---
INPUT=$(cat || { exit 0; })

# ============================================================================
# Phase 1: LOOKUP — Find tool in config dictionary
# ============================================================================

TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null || echo '')
log_v "Phase 1 LOOKUP: tool_name=$TOOL_NAME"
if [ -z "$TOOL_NAME" ]; then exit 0; fi

if [ ! -f "$SPEC_FILE" ]; then log_v "spec file not found, skipping"; exit 0; fi

# Resolve: aliases reverse lookup → exact key → case-insensitive fallback
# Each spec entry has an "aliases" array listing tool names from all agent
# frameworks (cosh, openclaw, hermes). We reverse-lookup from the input
# tool_name to find the matching spec key.
SPEC_KEY=$(jq -r --arg name "$TOOL_NAME" '
  to_entries[] | select(.key != "_meta") |
  .key as $spec_key |
  (.value.aliases // [])[] |
  select(. == $name) |
  $spec_key
' "$SPEC_FILE" 2>/dev/null | head -1)

# Fallback: exact spec key match
if [ -z "$SPEC_KEY" ]; then
  SPEC_KEY=$(jq -r --arg name "$TOOL_NAME" '
    to_entries[] | select(.key != "_meta") |
    select(.key == $name) | .key
  ' "$SPEC_FILE" 2>/dev/null | head -1)
fi

# Fallback: case-insensitive spec key match
if [ -z "$SPEC_KEY" ]; then
  SPEC_KEY=$(jq -r --arg name "$TOOL_NAME" '
    to_entries[] | select(.key != "_meta") |
    select(.key | ascii_downcase == ($name | ascii_downcase)) | .key
  ' "$SPEC_FILE" 2>/dev/null | head -1)
fi

if [ -z "$SPEC_KEY" ]; then
    log_v "Phase 1: $TOOL_NAME not in spec dict → skip"
    exit 0
fi
log_v "Phase 1: $TOOL_NAME → $SPEC_KEY found in spec dict"

# ============================================================================
# Phase 2: CHECK — Scan system readiness
# ============================================================================

# --- Normalize deps to object format ---
# Supports both string ("jq") and object ({binary:"jq",...}) formats.
# String defaults: manager="auto" (fix script auto-detects yum/dnf/apt/apk).
# Handles version constraints: "rtk>=0.35" → {binary:"rtk", version:">=0.35", ...}

normalize_deps() {
  local array="$1"
  echo "$array" | jq -c '[.[] | if type == "string" then
    (if (test(">=") or test("[^<]<[^=]") or test("=")) then
      {binary: (capture("^(?<b>[^>=<]+)") | .b), version: (match("[>=<]+[0-9.]+").string), package: (capture("^(?<b>[^>=<]+)") | .b), manager: "auto"}
    else
      {binary: ., package: ., manager: "auto"}
    end)
  else . end]' 2>/dev/null || echo '[]'
}

REQUIRED=$(normalize_deps "$(jq -c --arg key "$SPEC_KEY" '.[$key].required // []' "$SPEC_FILE")")
RECOMMENDED=$(normalize_deps "$(jq -c --arg key "$SPEC_KEY" '.[$key].recommended // []' "$SPEC_FILE")")
PERMISSIONS=$(jq -r --arg key "$SPEC_KEY" '.[$key].permissions[] // empty' "$SPEC_FILE" 2>/dev/null || echo '')

# --- Version comparison helper ---
# Handles prefixed versions (v22.1.0) and build suffixes (1.2.3-rc1)
version_ge() {
  local installed="$1" required="$2"
  # Strip common prefixes (v, V)
  installed="${installed#v}"; installed="${installed#V}"
  required="${required#v}"; required="${required#V}"
  local i_major i_minor i_patch r_major r_minor r_patch
  IFS='.' read -r i_major i_minor i_patch <<< "$installed"
  # Strip build suffixes per segment
  i_major="${i_major%%-*}"; i_minor="${i_minor%%-*}"; i_patch="${i_patch%%-*}"
  IFS='.' read -r r_major r_minor r_patch <<< "$required"
  r_major="${r_major%%-*}"; r_minor="${r_minor%%-*}"; r_patch="${r_patch%%-*}"
  # Extract only digits
  i_major=$(echo "${i_major:-0}" | grep -oE '[0-9]+' | head -1 || echo 0)
  i_minor=$(echo "${i_minor:-0}" | grep -oE '[0-9]+' | head -1 || echo 0)
  i_patch=$(echo "${i_patch:-0}" | grep -oE '[0-9]+' | head -1 || echo 0)
  r_major=$(echo "${r_major:-0}" | grep -oE '[0-9]+' | head -1 || echo 0)
  r_minor=$(echo "${r_minor:-0}" | grep -oE '[0-9]+' | head -1 || echo 0)
  r_patch=$(echo "${r_patch:-0}" | grep -oE '[0-9]+' | head -1 || echo 0)
  [ "$i_major" -gt "$r_major" ] && return 0
  [ "$i_major" -lt "$r_major" ] && return 1
  [ "$i_minor" -gt "$r_minor" ] && return 0
  [ "$i_minor" -lt "$r_minor" ] && return 1
  [ "$i_patch" -gt "$r_patch" ] && return 0
  [ "$i_patch" -lt "$r_patch" ] && return 1
  return 0
}

# --- Resolve binary path ---
# Tries command -v first, then known install paths.
resolve_binary() {
  local name="$1"
  local found
  found=$(command -v "$name" 2>/dev/null || true)
  if [ -n "$found" ]; then echo "$found"; return 0; fi
  for candidate in "$HOME/.local/bin/$name" "$HOME/.local/lib/anolisa/tokenless/$name"; do
    if [ -x "$candidate" ]; then echo "$candidate"; return 0; fi
  done
  return 1
}
# Output: "available", "missing", "version_low:<installed>:<required>"
check_dep() {
  local dep_json="$1"
  local binary version
  binary=$(echo "$dep_json" | jq -r '.binary')
  version=$(echo "$dep_json" | jq -r '.version // empty')

  local resolved
  if ! resolved=$(resolve_binary "$binary"); then
    echo "missing"
    return
  fi
  binary="$resolved"

  if [ -z "$version" ]; then
    echo "available"
    return
  fi

  local constraint_ver installed_version
  constraint_ver=$(echo "$version" | sed 's/[>=<]//g')
  installed_version=$("$binary" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || echo "0.0.0")
  [ -z "$installed_version" ] && installed_version="0.0.0"

  if version_ge "$installed_version" "$constraint_ver"; then
    echo "available"
  else
    echo "version_low:${installed_version}:${constraint_ver}"
  fi
}

# --- Check permissions ---
check_permissions() {
  local perm_missing=""
  for perm in $PERMISSIONS; do
    case "$perm" in
      file_read)   [ ! -r / ] && perm_missing="${perm_missing} file_read" ;;
      file_write)  touch "${TMPDIR:-/tmp}/.tokenless-ready-test" 2>/dev/null; rc=$?; rm -f "${TMPDIR:-/tmp}/.tokenless-ready-test" 2>/dev/null; [ $rc -ne 0 ] && perm_missing="${perm_missing} file_write" ;;
      exec_shell)  ! command -v bash &>/dev/null && perm_missing="${perm_missing} exec_shell" ;;
      docker_socket) [ ! -S /var/run/docker.sock ] && [ ! -S /run/docker.sock ] && perm_missing="${perm_missing} docker_socket" ;;
    esac
  done
  echo "$perm_missing"
}

# --- Scan all deps ---
MISSING_DEP_JSONS="[]"
HAS_REQUIRED_MISSING=false
HAS_VERSION_LOW=false
PERM_MISSING=$(check_permissions)

# Check required deps
req_count=$(echo "$REQUIRED" | jq 'length')
for i in $(seq 0 $((req_count - 1))); do
  dep_json=$(echo "$REQUIRED" | jq -c ".[$i]")
  status=$(check_dep "$dep_json")
  case "$status" in
    missing)
      HAS_REQUIRED_MISSING=true
      MISSING_DEP_JSONS=$(echo "$MISSING_DEP_JSONS" | jq -c ". + [$dep_json]")
      ;;
    version_low:*)
      HAS_VERSION_LOW=true
      ;;
  esac
done

# Check recommended deps
rec_count=$(echo "$RECOMMENDED" | jq 'length')
missing_count_rec=0
RECOMMENDED_MISSING_LIST=""
for i in $(seq 0 $((rec_count - 1))); do
  dep_json=$(echo "$RECOMMENDED" | jq -c ".[$i]")
  status=$(check_dep "$dep_json")
  case "$status" in
    missing)
      MISSING_DEP_JSONS=$(echo "$MISSING_DEP_JSONS" | jq -c ". + [$dep_json]")
      binary=$(echo "$dep_json" | jq -r '.binary')
      RECOMMENDED_MISSING_LIST="${RECOMMENDED_MISSING_LIST} ${binary}"
      missing_count_rec=$((missing_count_rec + 1))
      ;;
  esac
done

# --- Determine readiness ---
IS_READY=true
IS_PARTIAL=false
$HAS_REQUIRED_MISSING && IS_READY=false
$HAS_VERSION_LOW && IS_READY=false
[ -n "$PERM_MISSING" ] && IS_READY=false

# Recommended deps missing → PARTIAL (not blocking, but trigger auto-fix)
if $IS_READY && [ "$missing_count_rec" -gt 0 ]; then
    IS_PARTIAL=true
fi

if $IS_READY && ! $IS_PARTIAL; then
    log_v "Phase 2 CHECK: $TOOL_NAME → READY, silent pass"
    exit 0
fi
if $IS_PARTIAL; then
    log_v "Phase 2 CHECK: $TOOL_NAME → PARTIAL (recommended missing: ${RECOMMENDED_MISSING_LIST})"
else
    log_v "Phase 2 CHECK: $TOOL_NAME → NOT_READY (missing=$HAS_REQUIRED_MISSING version_low=$HAS_VERSION_LOW perm=$PERM_MISSING)"
fi

# ============================================================================
# Phase 3: FIX — Auto-install missing dependencies
# ============================================================================

missing_count=$(echo "$MISSING_DEP_JSONS" | jq 'length' 2>/dev/null || echo 0)

log_v "Phase 3 FIX: $missing_count missing deps, fix_script=$FIX_SCRIPT"

if [ "$missing_count" -gt 0 ] && [ -n "$FIX_SCRIPT" ] && [ -x "$FIX_SCRIPT" ]; then
    echo "$MISSING_DEP_JSONS" | bash "$FIX_SCRIPT" fix-all 2>/dev/null || true
    hash -r 2>/dev/null || true

    # Re-scan to check if fix succeeded
    STILL_MISSING=""
    HAS_REQUIRED_MISSING=false
    for i in $(seq 0 $((missing_count - 1))); do
        binary=$(echo "$MISSING_DEP_JSONS" | jq -r ".[$i].binary")
        if ! resolve_binary "$binary" >/dev/null 2>&1; then
            STILL_MISSING="${STILL_MISSING} ${binary}"
            HAS_REQUIRED_MISSING=true
        fi
    done

    # Re-check version_low entries after fix
    HAS_VERSION_LOW=false
    for i in $(seq 0 $((req_count - 1))); do
        dep_json=$(echo "$REQUIRED" | jq -c ".[$i]")
        status=$(check_dep "$dep_json")
        case "$status" in
            version_low:*)
                HAS_VERSION_LOW=true
                ;;
        esac
    done

    # Re-scan recommended deps after fix
    RECOMMENDED_MISSING_LIST=""
    missing_count_rec=0
    for i in $(seq 0 $((rec_count - 1))); do
        dep_json=$(echo "$RECOMMENDED" | jq -c ".[$i]")
        status=$(check_dep "$dep_json")
        case "$status" in
            missing)
                binary=$(echo "$dep_json" | jq -r '.binary')
                RECOMMENDED_MISSING_LIST="${RECOMMENDED_MISSING_LIST} ${binary}"
                missing_count_rec=$((missing_count_rec + 1))
                ;;
        esac
    done

    if [ -z "$STILL_MISSING" ] && ! $HAS_VERSION_LOW && [ -z "$PERM_MISSING" ]; then
        exit 0
    fi

    # After fix, re-check readiness
    # If only recommended still missing but required OK → PARTIAL, don't block
    if ! $HAS_REQUIRED_MISSING && ! $HAS_VERSION_LOW && [ -z "$PERM_MISSING" ]; then
        log_v "Phase 3 FIX: recommended deps partially installed, remaining: ${RECOMMENDED_MISSING_LIST}"
        DIAG_MSG="[tokenless:ready] ${TOOL_NAME}: PARTIAL — recommended deps not installed:${RECOMMENDED_MISSING_LIST}. Core tool is functional."
        jq -n --arg context "$DIAG_MSG" --arg msg "$DIAG_MSG" '{
          "systemMessage": $msg,
          "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "additionalContext": $context
          }
        }' || exit 0
        exit 0
    fi
fi

# ============================================================================
# Phase 4: FEEDBACK — Tool not available, inform the Agent
# ============================================================================

# PARTIAL (no fix script available): inform Agent but don't block
if $IS_PARTIAL && ! $HAS_REQUIRED_MISSING && ! $HAS_VERSION_LOW && [ -z "$PERM_MISSING" ]; then
    DIAG_MSG="[tokenless:ready] ${TOOL_NAME}: PARTIAL — recommended deps missing:${RECOMMENDED_MISSING_LIST}. Core tool is functional, extended deps may be unavailable."
    log_v "Phase 4 FEEDBACK: $TOOL_NAME → PARTIAL → injecting additionalContext (non-blocking)"
    jq -n --arg context "$DIAG_MSG" --arg msg "$DIAG_MSG" '{
      "systemMessage": $msg,
      "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "additionalContext": $context
      }
    }' || exit 0
    exit 0
fi

# NOT_READY: required deps or permissions missing → block with "Skip retry"
MISSING_LIST=""
for i in $(seq 0 $((missing_count - 1))); do
    binary=$(echo "$MISSING_DEP_JSONS" | jq -r ".[$i].binary")
    MISSING_LIST="${MISSING_LIST} ${binary}"
done

DIAG_PARTS=""
[ -n "$MISSING_LIST" ]  && DIAG_PARTS="${DIAG_PARTS} missing:${MISSING_LIST};"
$HAS_VERSION_LOW       && DIAG_PARTS="${DIAG_PARTS} version too low;"
[ -n "$PERM_MISSING" ] && DIAG_PARTS="${DIAG_PARTS} permission missing:${PERM_MISSING};"

DIAG_MSG="[tokenless:ready] ${TOOL_NAME}: NOT_READY (${DIAG_PARTS}) Skip retry."

log_v "Phase 4 FEEDBACK: $TOOL_NAME → NOT_READY → blocking with decision:block"

jq -n --arg context "$DIAG_MSG" --arg reason "$DIAG_MSG" '{
  "decision": "block",
  "reason": $reason,
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "additionalContext": $context
  }
}' || exit 0