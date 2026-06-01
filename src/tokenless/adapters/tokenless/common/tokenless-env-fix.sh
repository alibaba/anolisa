#!/usr/bin/env bash
# tokenless-env-fix — Config-driven environment auto-fix for Tool Ready feature.
# Reads dependency specs from JSON (tool-ready-spec.json or stdin) and installs
# missing packages via the declared package manager (rpm/apt/pip/uv/npm/npx/cargo/symlink/dir/path).
#
# Usage:
#   tokenless-env-fix.sh fix '<json_dep_spec>'           # Fix single dep (JSON object)
#   tokenless-env-fix.sh fix-all '<json_array>'           # Fix multiple deps (JSON array)
#   tokenless-env-fix.sh fix-simple <binary> [manager]    # Fix by name (defaults to detected manager)
#   tokenless-env-fix.sh check                            # List all auto-fixable deps from spec
#
# Fix results are logged to ~/.tokenless/env-fix.log
# Duplicate fixes within 24h are skipped.

set -euo pipefail

SUDO_PREFIX=""
[ "$(id -u)" -ne 0 ] && SUDO_PREFIX="sudo"

FIX_LOG_DIR="${HOME}/.tokenless"
FIX_LOG="${FIX_LOG_DIR}/env-fix.log"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SPEC_FILE="${SCRIPT_DIR}/tool-ready-spec.json"

# Detect system package manager by underlying mechanism (rpm/dpkg/apk),
# then pick the best frontend within that family.
# Priority: rpm-based (Alinux) > dpkg-based > apk-based.
PACKAGE_MANAGER="rpm"
if command -v rpm &>/dev/null; then
  # rpm-based system (Alinux): prefer dnf (modern), then yum (legacy)
  if command -v dnf &>/dev/null; then
    PACKAGE_MANAGER="dnf"
  else
    PACKAGE_MANAGER="yum"
  fi
elif command -v dpkg &>/dev/null; then
  PACKAGE_MANAGER="apt"
elif command -v apk &>/dev/null; then
  PACKAGE_MANAGER="apk"
fi

# Guard to avoid repeated apt-get update across multiple install_via_system calls
_APT_UPDATED=false

# --- Logging helpers ---

log_fix() {
  local dep="$1" status="$2" detail="$3"
  local timestamp
  timestamp=$(date +%Y-%m-%dT%H:%M:%S)
  mkdir -p "$FIX_LOG_DIR"
  echo "${timestamp} fix=${dep} status=${status} detail=${detail}" >> "$FIX_LOG"
}

was_recently_fixed() {
  local dep="$1"
  if [ ! -f "$FIX_LOG" ]; then return 1; fi
  local cutoff
  cutoff=$(date -d '24 hours ago' +%Y-%m-%dT%H:%M:%S 2>/dev/null || date -v-24H +%Y-%m-%dT%H:%M:%S 2>/dev/null || echo "")
  if [ -z "$cutoff" ]; then return 1; fi
  awk -v c="$cutoff" -v d="$dep" '$0 >= c && $0 ~ "fix=" d " status=success" {found=1; exit} END {exit !found}' "$FIX_LOG" 2>/dev/null
}

# --- Normalize a dep spec to object format ---
# Input: string like "jq" → {binary:"jq",package:"jq",manager:"rpm"}
# Input: object like {"binary":"jq",...} → pass through
# Output: JSON object

normalize_dep() {
  local input="$1"
  # If starts with {, it's already an object
  if echo "$input" | jq -e 'type == "object"' >/dev/null 2>&1; then
    echo "$input"
    return
  fi
  # It's a string — convert to object with defaults
  # Handle version constraints: "rtk>=0.35" → {binary:"rtk",version:">=0.35",...}
  local base_name version_constraint
  base_name=$(echo "$input" | sed 's/[>=<].*//')
  version_constraint=$(echo "$input" | grep -oE '[>=<]+[0-9.]+' || echo "")
  if [ -n "$version_constraint" ]; then
    jq -n --arg bn "$base_name" --arg vc "$version_constraint" --arg pk "$base_name" --arg mgr "$PACKAGE_MANAGER" \
      '{binary:$bn, version:$vc, package:$pk, manager:$mgr}'
  else
    jq -n --arg bn "$base_name" --arg pk "$base_name" --arg mgr "$PACKAGE_MANAGER" \
      '{binary:$bn, package:$pk, manager:$mgr}'
  fi
}

# --- Input validation ---
# Reject names that could be used for command injection or supply-chain attacks.

validate_name() {
  local val="$1" label="$2"
  if [ -z "$val" ]; then
    echo "[tokenless-env-fix] BLOCKED: empty ${label}"
    return 1
  fi
  if [ "${#val}" -gt 128 ]; then
    echo "[tokenless-env-fix] BLOCKED: ${label} too long (${#val} chars): ${val:0:32}..."
    return 1
  fi
  if ! echo "$val" | grep -qE '^[a-zA-Z0-9][a-zA-Z0-9._@/+-]*$'; then
    echo "[tokenless-env-fix] BLOCKED: invalid ${label}: ${val}"
    return 1
  fi
}

# --- Package manager install functions ---
# Each installs a package via the declared manager.
# Returns 0 on success, 1 on failure.

install_via_system() {
  local package="$1"
  # Refresh package index before first install on apt-based systems
  case "$PACKAGE_MANAGER" in
    apt)  if [ "$_APT_UPDATED" != true ]; then $SUDO_PREFIX apt-get update -qq 2>/dev/null || log_fix "apt-get update failed (network issue?)"; _APT_UPDATED=true; fi ;;
  esac
  # Try detected system manager first, then others as fallback (Alinux dnf/yum > apt > apk)
  case "$PACKAGE_MANAGER" in
    dnf)  $SUDO_PREFIX dnf install -y "$package" 2>/dev/null || $SUDO_PREFIX yum install -y "$package" 2>/dev/null || $SUDO_PREFIX apt-get install -y "$package" 2>/dev/null || $SUDO_PREFIX apk add "$package" 2>/dev/null ;;
    yum)  $SUDO_PREFIX yum install -y "$package" 2>/dev/null || $SUDO_PREFIX dnf install -y "$package" 2>/dev/null || $SUDO_PREFIX apt-get install -y "$package" 2>/dev/null || $SUDO_PREFIX apk add "$package" 2>/dev/null ;;
    apt)  $SUDO_PREFIX apt-get install -y "$package" 2>/dev/null || $SUDO_PREFIX dnf install -y "$package" 2>/dev/null || $SUDO_PREFIX yum install -y "$package" 2>/dev/null || $SUDO_PREFIX apk add "$package" 2>/dev/null ;;
    apk)  $SUDO_PREFIX apk add "$package" 2>/dev/null || $SUDO_PREFIX dnf install -y "$package" 2>/dev/null || $SUDO_PREFIX yum install -y "$package" 2>/dev/null || $SUDO_PREFIX apt-get install -y "$package" 2>/dev/null ;;
    *)    $SUDO_PREFIX yum install -y "$package" 2>/dev/null || $SUDO_PREFIX dnf install -y "$package" 2>/dev/null || $SUDO_PREFIX apt-get install -y "$package" 2>/dev/null || $SUDO_PREFIX apk add "$package" 2>/dev/null ;;
  esac
}

install_via_rpm() {
  $SUDO_PREFIX yum install -y "$1" 2>/dev/null || $SUDO_PREFIX dnf install -y "$1" 2>/dev/null || $SUDO_PREFIX rpm -ivh "$1" 2>/dev/null
}

install_via_pip() {
  local package="$1"
  local pip_name="${2:-$package}"
  local pip_cmd=""
  command -v pip3 &>/dev/null && pip_cmd="pip3" || { command -v pip &>/dev/null && pip_cmd="pip"; }
  if [ -z "$pip_cmd" ]; then return 1; fi

  # Stage 1: default mirror
  $pip_cmd install "$pip_name" 2>/dev/null
  hash -r
  if command -v "$package" &>/dev/null; then return 0; fi

  # pip reported success but binary missing (stale metadata) — uninstall + reinstall
  $pip_cmd uninstall -y "$pip_name" 2>/dev/null || true
  $pip_cmd install "$pip_name" 2>/dev/null
  hash -r
  if command -v "$package" &>/dev/null; then return 0; fi

  # Stage 2: purge cache and retry
  $pip_cmd cache purge 2>/dev/null
  $pip_cmd uninstall -y "$pip_name" 2>/dev/null || true
  $pip_cmd install --no-cache-dir "$pip_name" 2>/dev/null
  hash -r
  if command -v "$package" &>/dev/null; then return 0; fi

  # Stage 3: fallback to official PyPI (mirror may be broken/sync-lag)
  $pip_cmd uninstall -y "$pip_name" 2>/dev/null || true
  $pip_cmd install --no-cache-dir --index-url https://pypi.org/simple/ "$pip_name" 2>/dev/null
  hash -r
  if command -v "$package" &>/dev/null; then return 0; fi

  return 1
}

install_via_uv() {
  local package="$1"
  local uv_name="${2:-$package}"
  uv tool install "$uv_name" 2>/dev/null || uv pip install "$uv_name" 2>/dev/null
}

install_via_npm() {
  local package="$1"
  local npm_name="${2:-$package}"
  $SUDO_PREFIX npm install -g "$npm_name" 2>/dev/null
}

install_via_npx() {
  # npx doesn't install — just verifies availability
  local package="$1"
  npx -y "$package" --version 2>/dev/null >/dev/null
}

install_via_cargo() {
  cargo install "$1" --locked 2>/dev/null
}

install_via_cargo_build() {
  # Build from local Cargo.toml manifest, copy binary to /usr/local/bin
  local manifest="$1"
  local binary="$2"
  local features="${3:-}"
  if [ ! -f "$manifest" ]; then
    echo "[tokenless-env-fix] BLOCKED: manifest not found: $manifest"
    return 1
  fi
  local -a cargo_args=("--release" "--manifest-path" "$manifest")
  if [ -n "$features" ]; then
    cargo_args+=("--features" "$features")
  fi
  cargo build "${cargo_args[@]}"
  # Find the built binary
  local target_dir
  target_dir=$(dirname "$manifest")/target/release
  if [ -x "${target_dir}/${binary}" ]; then
    $SUDO_PREFIX cp "${target_dir}/${binary}" /usr/local/bin/"${binary}" 2>/dev/null || true
    $SUDO_PREFIX chmod +x /usr/local/bin/"${binary}" 2>/dev/null || true
  fi
}

install_via_symlink() {
  local binary="$1"
  local source="$2"
  # Only allow symlinks from trusted installation directories
  case "$source" in
    /usr/lib/anolisa/*|/usr/libexec/anolisa/*|/usr/share/anolisa/*|/usr/local/lib/anolisa/*|/usr/local/libexec/anolisa/*|/usr/local/share/anolisa/*)
      ;;
    "$HOME"/.local/share/anolisa/*)
      ;;
    *)
      echo "[tokenless-env-fix] BLOCKED: symlink source not in trusted path: $source"
      return 1
      ;;
  esac
  if [ ! -f "$source" ]; then
    echo "[tokenless-env-fix] BLOCKED: symlink source does not exist: $source"
    return 1
  fi
  $SUDO_PREFIX ln -sf "$source" /usr/local/bin/"$binary" 2>/dev/null || true
  chmod +x "$source" 2>/dev/null || true
}

install_via_path() {
  local path_dir="$1"
  if [[ ":$PATH:" != *":${path_dir}:"* ]]; then
    export PATH="${path_dir}:${PATH}"
    local shell_rc="${HOME}/.bashrc"
    [ -f "${HOME}/.zshrc" ] && shell_rc="${HOME}/.zshrc"
    grep -Fq "export PATH=\"${path_dir}" "$shell_rc" 2>/dev/null || echo "export PATH=\"${path_dir}:\$PATH\"" >> "$shell_rc"
  fi
}

install_via_dir() {
  mkdir -p "$1"
}

install_via_curl_pipe_sh() {
  local url="$1"
  local args="${2:-}"
  local timeout_secs="${3:-120}"
  # Only allow HTTPS URLs from trusted domains (anchored with path separator)
  local allowed_domains="^https://(github\.com/|raw\.githubusercontent\.com/|sh\.rustup\.rs(/|$)|get\.docker\.com(/|$)|cli\.run\.nu(/|$)|get\.starship\.rs(/|$)|astral\.sh(/|$))"
  if ! echo "$url" | grep -qE "$allowed_domains"; then
    echo "[tokenless-env-fix] BLOCKED: curl|sh denied — untrusted or non-HTTPS URL: $url"
    return 1
  fi
  echo "[tokenless-env-fix] NOTE: executing remote script from $url (timeout: ${timeout_secs}s)"
  if command -v curl &>/dev/null; then
    if [ -n "$args" ]; then
      timeout "$timeout_secs" curl -fsSL --max-redirs 5 "$url" 2>/dev/null | timeout "$timeout_secs" sh -s -- "$args"
    else
      timeout "$timeout_secs" curl -fsSL --max-redirs 5 "$url" 2>/dev/null | timeout "$timeout_secs" sh
    fi
  elif command -v wget &>/dev/null; then
    if [ -n "$args" ]; then
      timeout "$timeout_secs" wget --max-redirect=5 -qO- "$url" | timeout "$timeout_secs" sh -s -- "$args"
    else
      timeout "$timeout_secs" wget --max-redirect=5 -qO- "$url" | timeout "$timeout_secs" sh
    fi
  else
    return 1
  fi
}

# --- Core fix logic ---
# Given a normalized dep JSON object, attempt to install via declared manager + fallbacks.

fix_dep() {
  local dep_json="$1"
  local binary package manager version pip_name uv_name npm_name use_npx url args

  binary=$(echo "$dep_json" | jq -r '.binary // empty')
  package=$(echo "$dep_json" | jq -r '.package // empty')
  # Resolve per-manager package overrides: apt_package/apk_package
  case "$PACKAGE_MANAGER" in
    apt)  apt_pkg=$(echo "$dep_json" | jq -r '.apt_package // empty'); [ -n "$apt_pkg" ] && package="$apt_pkg" ;;
    apk)  apk_pkg=$(echo "$dep_json" | jq -r '.apk_package // empty'); [ -n "$apk_pkg" ] && package="$apk_pkg" ;;
  esac
  manager=$(echo "$dep_json" | jq -r '.manager // "rpm"')
  version=$(echo "$dep_json" | jq -r '.version // empty')
  pip_name=$(echo "$dep_json" | jq -r '.pip_name // empty')
  uv_name=$(echo "$dep_json" | jq -r '.uv_name // empty')
  npm_name=$(echo "$dep_json" | jq -r '.npm_name // empty')
  use_npx=$(echo "$dep_json" | jq -r '.use_npx // false')
  url=$(echo "$dep_json" | jq -r '.url // empty')
  args=$(echo "$dep_json" | jq -r '.args // empty')

  # Validate names before any install action
  validate_name "$binary" "binary" || return 1
  validate_name "$package" "package" || return 1

  # Fill defaults: pip_name/uv_name/npm_name default to package
  [ -z "$pip_name" ] && pip_name="$package"
  [ -z "$uv_name" ] && uv_name="$package"
  [ -z "$npm_name" ] && npm_name="$package"

  # Validate derived names
  [ -n "$pip_name" ] && { validate_name "$pip_name" "pip_name" || return 1; }
  [ -n "$uv_name" ] && { validate_name "$uv_name" "uv_name" || return 1; }
  [ -n "$npm_name" ] && { validate_name "$npm_name" "npm_name" || return 1; }

  # Skip if already available (clear hash cache first)
  hash -r
  if command -v "$binary" &>/dev/null; then
    # Check version constraint if present
    if [ -n "$version" ]; then
      local constraint_ver installed_ver
      constraint_ver=$(echo "$version" | sed 's/[>=<]//g')
      installed_ver=$("$binary" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || echo "0.0.0")

      if [ -n "$installed_ver" ] && [ "$installed_ver" != "0.0.0" ]; then
        # Simple >= check
        local i_major i_minor i_patch r_major r_minor r_patch
        IFS='.' read -r i_major i_minor i_patch <<< "$installed_ver"
        IFS='.' read -r r_major r_minor r_patch <<< "$constraint_ver"
        i_major=${i_major:-0}; i_minor=${i_minor:-0}; i_patch=${i_patch:-0}
        r_major=${r_major:-0}; r_minor=${r_minor:-0}; r_patch=${r_patch:-0}
        if [ "$i_major" -gt "$r_major" ] || \
           { [ "$i_major" -eq "$r_major" ] && [ "$i_minor" -gt "$r_minor" ]; } || \
           { [ "$i_major" -eq "$r_major" ] && [ "$i_minor" -eq "$r_minor" ] && [ "$i_patch" -ge "$r_patch" ]; }; then
          echo "[tokenless-env-fix] ${binary}: already available (v${installed_ver} satisfies ${version})"
          return 0
        fi
      fi
    else
      echo "[tokenless-env-fix] ${binary}: already available"
      return 0
    fi
  fi

  # Skip if recently fixed successfully AND binary still present
  # (handles the case where dep was fixed then later uninstalled)
  if was_recently_fixed "$binary"; then
    hash -r
    if command -v "$binary" &>/dev/null; then
      echo "[tokenless-env-fix] ${binary}: skipped (recently fixed, still present)"
      return 0
    fi
    echo "[tokenless-env-fix] ${binary}: recently fixed but missing, re-installing"
  fi

  echo "[tokenless-env-fix] ${binary}: attempting install via ${manager}..."

  # --- Primary install via declared manager ---
  local primary_ok=false
  case "$manager" in
    auto|rpm|apt|dnf|yum|apk)  install_via_system "$package" && primary_ok=true ;;
    pip)     install_via_pip "$package" "$pip_name" && primary_ok=true ;;
    uv)      install_via_uv "$package" "$uv_name" && primary_ok=true ;;
    npm)     install_via_npm "$package" "$npm_name" && primary_ok=true ;;
    npx)     install_via_npx "$package" && primary_ok=true ;;
    cargo)   install_via_cargo "$package" && primary_ok=true ;;
    symlink) local src; src=$(echo "$dep_json" | jq -r '.source // empty'); install_via_symlink "$binary" "$src" && primary_ok=true ;;
    path)    local pdir; pdir=$(echo "$dep_json" | jq -r '.source // "/usr/libexec/anolisa/tokenless"'); if [ ! -d "$pdir" ]; then pdir="/usr/lib/anolisa/tokenless"; fi; install_via_path "$pdir" && primary_ok=true ;;
    dir)     local dpath; dpath=$(echo "$dep_json" | jq -r '.source // empty'); install_via_dir "$dpath" && primary_ok=true ;;
    curl_pipe_sh) [ -n "$url" ] && install_via_curl_pipe_sh "$url" "$args" && primary_ok=true ;;
    *)
      echo "[tokenless-env-fix] ${binary}: unknown manager '${manager}'"
      ;;
  esac

  # Verify primary install (clear hash cache so newly installed binaries are discoverable)
  hash -r
  if $primary_ok && command -v "$binary" &>/dev/null; then
    log_fix "$binary" "success" "installed via ${manager}"
    echo "[tokenless-env-fix] ${binary}: installed via ${manager}"
    return 0
  fi

  # --- Fallback strategies ---
  local fallbacks
  fallbacks=$(echo "$dep_json" | jq -c '.fallback // []' 2>/dev/null || echo '[]')
  local fallback_count
  fallback_count=$(echo "$fallbacks" | jq 'length' 2>/dev/null || echo 0)

  if [ "$fallback_count" -gt 0 ]; then
    for i in $(seq 0 $((fallback_count - 1))); do
      local fb_method fb_package fb_binary fb_source fb_manifest fb_features fb_url fb_args
      fb_method=$(echo "$fallbacks" | jq -r ".[$i].method // empty")
      fb_package=$(echo "$fallbacks" | jq -r ".[$i].package // empty")
      fb_binary=$(echo "$fallbacks" | jq -r --arg def "$binary" ".[$i].binary // \$def")
      fb_source=$(echo "$fallbacks" | jq -r ".[$i].source // empty")
      fb_manifest=$(echo "$fallbacks" | jq -r ".[$i].manifest // empty")
      fb_features=$(echo "$fallbacks" | jq -r ".[$i].features // empty")
      fb_url=$(echo "$fallbacks" | jq -r ".[$i].url // empty")
      fb_args=$(echo "$fallbacks" | jq -r ".[$i].args // empty")

      echo "[tokenless-env-fix] ${binary}: trying fallback ${fb_method}..."

      local fb_ok=false
      case "$fb_method" in
        rpm|apt|dnf|yum|apk)  [ -n "$fb_package" ] && install_via_system "$fb_package" && fb_ok=true ;;
        pip)     [ -n "$fb_package" ] && install_via_pip "$fb_package" && fb_ok=true ;;
        uv)      [ -n "$fb_package" ] && install_via_uv "$fb_package" && fb_ok=true ;;
        npm)     [ -n "$fb_package" ] && install_via_npm "$fb_package" && fb_ok=true ;;
        npx)     [ -n "$fb_package" ] && install_via_npx "$fb_package" && fb_ok=true ;;
        cargo)   [ -n "$fb_package" ] && install_via_cargo "$fb_package" && fb_ok=true ;;
        cargo_build) [ -n "$fb_manifest" ] && install_via_cargo_build "$fb_manifest" "$fb_binary" "$fb_features" && fb_ok=true ;;
        symlink) [ -n "$fb_source" ] && install_via_symlink "$fb_binary" "$fb_source" && fb_ok=true ;;
        path)    local _fb_pdir="${fb_source:-/usr/libexec/anolisa/tokenless}"; if [ ! -d "$_fb_pdir" ]; then _fb_pdir="/usr/lib/anolisa/tokenless"; fi; install_via_path "$_fb_pdir" && fb_ok=true ;;
        dir)     [ -n "$fb_source" ] && install_via_dir "$fb_source" && fb_ok=true ;;
        curl_pipe_sh) [ -n "$fb_url" ] && install_via_curl_pipe_sh "$fb_url" "$fb_args" && fb_ok=true ;;
        *) echo "[tokenless-env-fix] ${binary}: unknown fallback method '${fb_method}'" ;;
      esac

      hash -r
      if $fb_ok && command -v "$fb_binary" &>/dev/null; then
        log_fix "$binary" "success" "installed via fallback ${fb_method}"
        echo "[tokenless-env-fix] ${binary}: installed via fallback ${fb_method}"
        return 0
      fi
    done
  fi

  # All strategies failed
  log_fix "$binary" "failed" "all strategies failed (primary: ${manager}, fallbacks: ${fallback_count})"
  echo "[tokenless-env-fix] ${binary}: install failed (primary: ${manager}, ${fallback_count} fallbacks exhausted)"
  return 1
}

# --- Fix from spec file ---
# Read all dep entries from tool-ready-spec.json for a given tool

fix_tool_from_spec() {
  local tool_name="$1"
  if [ ! -f "$SPEC_FILE" ]; then
    echo "[tokenless-env-fix] spec file not found: $SPEC_FILE"
    return 1
  fi
  local tool_spec
  tool_spec=$(jq -c --arg key "$tool_name" '.[$key]' "$SPEC_FILE" 2>/dev/null || echo 'null')
  if [ "$tool_spec" = "null" ] || [ -z "$tool_spec" ]; then
    echo "[tokenless-env-fix] no spec for tool: $tool_name"
    return 0
  fi

  # Collect all dep entries from required + recommended
  local all_deps
  all_deps=$(echo "$tool_spec" | jq -c --arg mgr "$PACKAGE_MANAGER" '[(.required // []) + (.recommended // []) | .[] | if type == "string" then (if test("[>=<]") then {binary: (split("[>=<]") | .[0]), version: (capture("[>=<]+[0-9.]+"; "g") | .[0]), package: (split("[>=<]") | .[0]), manager: $mgr} else {binary: ., package: ., manager: $mgr} end) else . end]' 2>/dev/null || echo '[]')

  local count
  count=$(echo "$all_deps" | jq 'length' 2>/dev/null || echo 0)

  for i in $(seq 0 $((count - 1))); do
    local dep_json
    dep_json=$(echo "$all_deps" | jq -c ".[$i]")
    fix_dep "$dep_json" || true
  done
}

# --- Main entry point ---

case "${1:-}" in
  fix)
    if [ -z "${2:-}" ]; then
      echo "Usage: tokenless-env-fix.sh fix '<json_dep_spec>'"
      echo "       tokenless-env-fix.sh fix-simple <binary> [manager]"
      exit 1
    fi
    # Determine if input is JSON or a simple name
    if echo "$2" | jq -e 'type == "object"' >/dev/null 2>&1; then
      fix_dep "$2"
    else
      # Simple name — normalize to object with optional manager
      manager="${3:-$PACKAGE_MANAGER}"
      dep_json=$(jq -n --arg bn "$2" --arg pk "$2" --arg mgr "$manager" '{binary:$bn, package:$pk, manager:$mgr}')
      fix_dep "$dep_json"
    fi
    ;;
  fix-simple)
    # Fix by binary name with optional manager (defaults to detected PACKAGE_MANAGER)
    if [ -z "${2:-}" ]; then
      echo "Usage: tokenless-env-fix.sh fix-simple <binary> [manager]"
      exit 1
    fi
    manager="${3:-$PACKAGE_MANAGER}"
    dep_json=$(jq -n --arg bn "$2" --arg pk "$2" --arg mgr "$manager" '{binary:$bn, package:$pk, manager:$mgr}')
    fix_dep "$dep_json"
    ;;
  fix-all)
    input=""
    if [ -n "${2:-}" ] && [ "$2" != "-" ]; then
      input="$2"
    else
      input=$(cat)
    fi
    # Normalize all entries
    normalized=$(echo "$input" | jq -c --arg mgr "$PACKAGE_MANAGER" '[.[] | if type == "string" then {binary: ., package: ., manager: $mgr} else . end]' 2>/dev/null || echo '[]')
    count=$(echo "$normalized" | jq 'length' 2>/dev/null || echo 0)
    for i in $(seq 0 $((count - 1))); do
      fix_dep "$(echo "$normalized" | jq -c ".[$i]")" || true
    done
    ;;
  fix-tool)
    # Fix all deps for a tool from spec file
    if [ -z "${2:-}" ]; then
      echo "Usage: tokenless-env-fix.sh fix-tool <tool_name>"
      exit 1
    fi
    fix_tool_from_spec "$2"
    ;;
  check)
    if [ ! -f "$SPEC_FILE" ]; then
      echo "[tokenless-env-fix] spec file not found: $SPEC_FILE"
      echo "Supported managers: rpm, apt, pip, uv, npm, npx, cargo, cargo_build, symlink, path, dir, curl_pipe_sh"
      exit 0
    fi
    echo "Auto-fixable dependencies (from spec):"
    # Collect all dep entries across all tools
    all_deps=$(jq -c --arg mgr "$PACKAGE_MANAGER" '[del(."_comment") | to_entries[] | select(.key != "_meta") | .value | (.required // []) + (.recommended // []) | .[] | if type == "string" then {binary: ., package: ., manager: $mgr} else . end]' "$SPEC_FILE" 2>/dev/null || echo '[]')
    count=$(echo "$all_deps" | jq 'length' 2>/dev/null || echo 0)
    dep_json="" binary="" package="" manager="" fb_count=""
    for i in $(seq 0 $((count - 1))); do
      dep_json=$(echo "$all_deps" | jq -c ".[$i]")
      binary=$(echo "$dep_json" | jq -r '.binary')
      package=$(echo "$dep_json" | jq -r '.package')
      manager=$(echo "$dep_json" | jq -r '.manager')
      fb_count=$(echo "$dep_json" | jq '.fallback // [] | length')
      echo "  ${binary} — ${manager} (package: ${package}, fallbacks: ${fb_count})"
    done
    echo ""
    echo "Detected system package manager: $PACKAGE_MANAGER"
    echo "Supported managers:"
    echo "  rpm       — system package manager (auto-detect: dnf/yum for Alinux, apt for Debian, apk for Alpine; current: $PACKAGE_MANAGER)"
    echo "  apt       — apt-get (Debian/Ubuntu)"
    echo "  pip       — pip / pip3"
    echo "  uv        — uv tool install / uv pip install"
    echo "  npm       — npm install -g"
    echo "  npx       — npx -y (verify availability)"
    echo "  cargo     — cargo install --locked"
    echo "  cargo_build — cargo build from local manifest"
    echo "  symlink   — ln -sf from source path"
    echo "  path      — add directory to PATH"
    echo "  dir       — mkdir -p"
    echo "  curl_pipe_sh — curl/wget | sh (official install scripts)"
    ;;
  *)
    echo "Usage: tokenless-env-fix.sh <command> [args]"
    echo ""
    echo "Commands:"
    echo "  fix '<json>'         Fix a single dep (JSON object or simple name)"
    echo "  fix-simple <name> [mgr]  Fix by binary name with optional manager"
    echo "  fix-all '<json_arr>' Fix multiple deps (JSON array)"
    echo "  fix-tool <name>      Fix all deps for a tool from spec file"
    echo "  check                 List all auto-fixable deps from spec"
    ;;
esac