#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────
# build-all.sh  –  ANOLISA unified build script
#
# Usage:
#   ./scripts/build-all.sh                                    # install deps + build + install (default)
#   ./scripts/build-all.sh --no-install                       # install deps + build, skip installation
#   ./scripts/build-all.sh --ignore-deps                      # build + install, skip dep install
#   ./scripts/build-all.sh --deps-only                        # install deps only
#   ./scripts/build-all.sh --component cosh                   # deps + build + install copilot-shell only
#   ./scripts/build-all.sh --uninstall                        # uninstall all components
#   ./scripts/build-all.sh --uninstall --component cosh       # uninstall copilot-shell only
#   ./scripts/build-all.sh --help
#
# Components (build order):
#   cosh     copilot-shell      (Node.js / TypeScript)
#   skills   os-skills          (Markdown skill definitions, no compilation)
#   sec-core agent-sec-core     (Security CLI + sandbox + hooks)
#   tokenless tokenless         (Rust compression library, cross-platform)
#   ws-ckpt  ws-ckpt           (Rust workspace checkpoint daemon)
#   memory   agent-memory       (Rust MCP filesystem memory server, Linux only)
#   sight    agentsight         (eBPF / Rust, Linux only, NOT built by default)
# ──────────────────────────────────────────────────────────────────
set -euo pipefail

# ─── colors ───

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

# ─── paths ───

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ─── defaults ───

INSTALL_DEPS=true
DEPS_ONLY=false
DO_INSTALL=true
DO_UNINSTALL=false
DRY_RUN=false
INTERACTIVE=false
NON_INTERACTIVE=false
INSTALL_MODE="user"
COMPONENTS=()

SYSTEM_PREFIX="/usr"
SYSTEM_BIN_DIR="/usr/local/bin"
NPM_REGISTRY="${NPM_REGISTRY:-https://registry.npmmirror.com}"
INSTALL_PREFIX="$HOME/.local"
INSTALL_BIN_DIR="$INSTALL_PREFIX/bin"
USER_BIN_DIR="$INSTALL_PREFIX/bin"
USER_LIB_DIR="$INSTALL_PREFIX/lib"
USER_LIBEXEC_DIR="$INSTALL_PREFIX/libexec"
USER_SHARE_DIR="$INSTALL_PREFIX/share"
USER_DOC_DIR="$INSTALL_PREFIX/share/doc"

USER_COSH_DIR="$HOME/.copilot-shell"
USER_COSH_EXTENSIONS_DIR="$USER_COSH_DIR/extensions"
USER_COSH_SKILLS_DIR="$USER_COSH_DIR/skills"
INSTALL_EXTENSIONS_DIR="$USER_COSH_EXTENSIONS_DIR"

# sec-core install paths are loaded from src/agent-sec-core/Makefile after
# INSTALL_PROFILE is resolved, so build-all does not duplicate its defaults.
SEC_CORE_BIN_DIR=""
SEC_CORE_LIB_DIR=""
SEC_CORE_RUST_TOOLCHAIN="1.93.0"

# ─── output / staging ───

OUTPUT_DIR="$PROJECT_ROOT/target"
LOG_FILE="$OUTPUT_DIR/build.log"

if [[ ! -t 1 || -n "${NO_COLOR:-}" ]]; then
    RED=''
    GREEN=''
    YELLOW=''
    BLUE=''
    CYAN=''
    BOLD=''
    DIM=''
    NC=''
fi

# ─── helpers ───

info()  { echo -e "${BLUE}[info]${NC}  $*"; }
ok()    { echo -e "${GREEN}[ok]${NC}    $*"; }
warn()  { echo -e "${YELLOW}[warn]${NC}  $*"; }
err()   { echo -e "${RED}[error]${NC} $*"; }
step()  { echo -e "\n${CYAN}${BOLD}==> $*${NC}"; }

cmd_exists() { command -v "$1" &>/dev/null; }

# Extract first semver (X.Y.Z) from a string.
# Examples: "rustc 1.91.0 (abc 2024)" -> "1.91.0", "v22.21.1" -> "22.21.1"
extract_ver() {
    echo "$1" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1
}

# ver_gte "1.91.0" "1.80.0" -> true (actual >= required)
ver_gte() {
    printf '%s\n%s' "$2" "$1" | sort -V -C
}

die() { err "$@"; exit 1; }

as_root() {
    if [[ "$(id -u)" -eq 0 ]]; then
        "$@"
    else
        sudo "$@"
    fi
}

run_cmd() {
    if $DRY_RUN; then
        echo "DRY-RUN: $*"
    else
        "$@"
    fi
}

component_target_dir() {
    echo "$OUTPUT_DIR/$1"
}

component_install_root() {
    echo "$(component_target_dir "$1")/install-root"
}

stage_component_make_install() {
    local component="$1" dir="$2"; shift 2
    local stage_root
    stage_root="$(component_target_dir "$component")"

    [[ -d "$dir" ]] || die "Directory not found: $dir"

    if $DRY_RUN; then
        echo "DRY-RUN: rm -rf $stage_root"
        echo "DRY-RUN: mkdir -p $stage_root"
        echo "DRY-RUN: (cd $dir && make install DESTDIR=$stage_root INSTALL_PROFILE=system PREFIX= BINDIR=/bin $*)"
        return 0
    fi

    rm -rf "$stage_root"
    mkdir -p "$stage_root"

    cd "$dir"
    run_logged "stage ${component} -> target/${component}" \
        make install DESTDIR="$stage_root" INSTALL_PROFILE=system \
            PREFIX="" BINDIR="/bin" "$@"
}

system_staged_install() {
    local component="$1" stage_root="$2"
    [[ -d "$stage_root" ]] || die "Staged install root not found: $stage_root"

    if $DRY_RUN; then
        echo "DRY-RUN: cp -a $stage_root/. /"
    else
        info "Installing ${component} from ${stage_root} to / ..."
        as_root cp -a "$stage_root/." /
    fi
}

run_component_make_install() {
    local component="$1" dir="$2"; shift 2
    [[ -d "$dir" ]] || die "Directory not found: $dir"

    if $DRY_RUN; then
        if [[ "$INSTALL_MODE" == "system" ]]; then
            local stage_root
            stage_root="$(component_install_root "$component")"
            echo "DRY-RUN: rm -rf $stage_root"
            echo "DRY-RUN: mkdir -p $stage_root"
            echo "DRY-RUN: (cd $dir && make install DESTDIR=$stage_root INSTALL_PROFILE=system PREFIX=$SYSTEM_PREFIX BINDIR=$SYSTEM_BIN_DIR SERVICE_BINDIR=$SYSTEM_BIN_DIR $*)"
            echo "DRY-RUN: cp -a $stage_root/. /"
        else
            echo "DRY-RUN: (cd $dir && make install INSTALL_PROFILE=user PREFIX=$INSTALL_PREFIX $*)"
        fi
        return 0
    fi

    cd "$dir"

    if [[ "$INSTALL_MODE" == "system" ]]; then
        local stage_root
        stage_root="$(component_install_root "$component")"
        rm -rf "$stage_root"
        mkdir -p "$stage_root"
        run_logged "stage system install ${component} -> target/${component}/install-root" \
            make install DESTDIR="$stage_root" INSTALL_PROFILE=system \
                PREFIX="$SYSTEM_PREFIX" BINDIR="$SYSTEM_BIN_DIR" \
                SERVICE_BINDIR="$SYSTEM_BIN_DIR" "$@"
        system_staged_install "$component" "$stage_root"
    else
        run_logged "make install (${component})" \
            make install INSTALL_PROFILE=user PREFIX="$INSTALL_PREFIX" "$@"
    fi
}

run_component_make_uninstall() {
    local component="$1" dir="$2"; shift 2
    [[ -d "$dir" ]] || die "Directory not found: $dir"

    if $DRY_RUN; then
        if [[ "$INSTALL_MODE" == "system" ]]; then
            echo "DRY-RUN: (cd $dir && sudo make uninstall INSTALL_PROFILE=system PREFIX=$SYSTEM_PREFIX BINDIR=$SYSTEM_BIN_DIR SERVICE_BINDIR=$SYSTEM_BIN_DIR $*)"
        else
            echo "DRY-RUN: (cd $dir && make uninstall INSTALL_PROFILE=user PREFIX=$INSTALL_PREFIX $*)"
        fi
        return 0
    fi

    cd "$dir"

    if [[ "$INSTALL_MODE" == "system" ]]; then
        run_logged "make uninstall (${component})" \
            as_root make uninstall INSTALL_PROFILE=system \
                PREFIX="$SYSTEM_PREFIX" BINDIR="$SYSTEM_BIN_DIR" \
                SERVICE_BINDIR="$SYSTEM_BIN_DIR" "$@"
    else
        run_logged "make uninstall (${component})" \
            make uninstall INSTALL_PROFILE=user PREFIX="$INSTALL_PREFIX" "$@"
    fi
}

sec_core_cmd() {
    if [[ "$INSTALL_MODE" == "system" ]]; then
        as_root "$@"
    else
        "$@"
    fi
}

copy_tree() {
    local src="$1" dst="$2"
    [[ -d "$src" ]] || die "Directory not found: $src"
    if $DRY_RUN; then
        echo "DRY-RUN: copy tree $src -> $dst"
        return 0
    fi
    mkdir -p "$dst"
    cp -rp "$src/." "$dst/"
}

copy_file() {
    local src="$1" dst="$2" mode="${3:-0644}"
    [[ -f "$src" ]] || die "File not found: $src"
    if $DRY_RUN; then
        echo "DRY-RUN: install -p -m $mode $src $dst"
        return 0
    fi
    mkdir -p "$(dirname "$dst")"
    install -p -m "$mode" "$src" "$dst"
}

stage_skill_dirs() {
    local src_root="$1" dst_root="$2" skill_dir skill_name
    [[ -d "$src_root" ]] || die "Directory not found: $src_root"
    if $DRY_RUN; then
        echo "DRY-RUN: stage flattened skills from $src_root -> $dst_root"
        return 0
    fi
    mkdir -p "$dst_root"
    while IFS= read -r skill_file; do
        skill_dir="$(dirname "$skill_file")"
        skill_name="$(basename "$skill_dir")"
        mkdir -p "$dst_root/$skill_name"
        cp -rp "$skill_dir/." "$dst_root/$skill_name/"
    done < <(find "$src_root" -name "SKILL.md" -type f | sort)
}

install_skill_dirs_flat() {
    local src_root="$1" dst_root="$2" skill_dir skill_name
    [[ -d "$src_root" ]] || die "Directory not found: $src_root"
    if $DRY_RUN; then
        echo "DRY-RUN: install flattened skills from $src_root -> $dst_root"
        return 0
    fi
    sec_core_cmd install -d -m 0755 "$dst_root"
    while IFS= read -r skill_file; do
        skill_dir="$(dirname "$skill_file")"
        skill_name="$(basename "$skill_dir")"
        sec_core_cmd rm -rf "$dst_root/$skill_name"
        sec_core_cmd install -d -m 0755 "$dst_root/$skill_name"
        sec_core_cmd cp -rp "$skill_dir/." "$dst_root/$skill_name/"
    done < <(find "$src_root" -name "SKILL.md" -type f | sort)
}

remove_skill_dirs_flat() {
    local src_root="$1" dst_root="$2" skill_dir skill_name
    [[ -d "$src_root" ]] || return 0
    if $DRY_RUN; then
        echo "DRY-RUN: remove flattened skills from $dst_root using $src_root"
        return 0
    fi
    while IFS= read -r skill_file; do
        skill_dir="$(dirname "$skill_file")"
        skill_name="$(basename "$skill_dir")"
        sec_core_cmd rm -rf "$dst_root/$skill_name"
    done < <(find "$src_root" -name "SKILL.md" -type f | sort)
}

stage_adapter_manifest() {
    local comp="$1" src="$2"
    [[ -f "$src" ]] || return 0
    if $DRY_RUN; then
        echo "DRY-RUN: stage adapter manifest $src -> target/$comp"
        return 0
    fi
    copy_file "$src" "$(component_target_dir "$comp")/share/anolisa/adapters/$comp/manifest.json" 0644
    copy_file "$src" "$(component_target_dir "$comp")/adapter-manifest.json" 0644
}

# Run a command, redirect all output (stdout+stderr) to LOG_FILE.
# Shows an animated spinner on the same line while the command runs,
# then replaces it with ok / FAILED.
run_logged() {
    local desc="$1"; shift

    if $DRY_RUN; then
        echo "DRY-RUN: $desc: $*"
        return 0
    fi

    mkdir -p "$(dirname "$LOG_FILE")"
    "$@" >> "$LOG_FILE" 2>&1 &
    local pid=$!

    local spin='-\|/' i=0
    while kill -0 "$pid" 2>/dev/null; do
        printf "\r    ${DIM}%-52s${NC}  ${CYAN}%s${NC}" "$desc" "${spin:$((i % 4)):1}"
        i=$((i + 1))
        sleep 0.1
    done

    local rc=0
    wait "$pid" || rc=$?
    if [[ $rc -eq 0 ]]; then
        printf "\r    ${DIM}%-52s${NC}  ${GREEN}ok${NC}\n" "$desc"
    else
        printf "\r    ${DIM}%-52s${NC}  ${RED}FAILED${NC}\n" "$desc"
        warn "Failed: $*"
        info "Full output: $LOG_FILE"
        return $rc
    fi
}

run_logged_timeout() {
    local seconds="$1"; shift
    local desc="$1"; shift

    if cmd_exists timeout; then
        run_logged "$desc" timeout "$seconds" "$@"
    else
        run_logged "$desc" "$@"
    fi
}

makefile_var() {
    local dir="$1" profile="$2" var="$3"
    make -s -C "$dir" INSTALL_PROFILE="$profile" VAR="$var" -f - print-var <<'MAKE_EOF'
include Makefile
print-var:
	@printf '%s\n' "$($(VAR))"
MAKE_EOF
}

load_sec_core_make_paths() {
    local dir="$PROJECT_ROOT/src/agent-sec-core"
    [[ -f "$dir/Makefile" ]] || return 0

    SEC_CORE_BIN_DIR="$(makefile_var "$dir" "$INSTALL_MODE" BINDIR)" || \
        die "Failed to read BINDIR from sec-core Makefile"
    SEC_CORE_LIB_DIR="$(makefile_var "$dir" "$INSTALL_MODE" LIBDIR)" || \
        die "Failed to read LIBDIR from sec-core Makefile"
}

ensure_user_mode() {
    case "$INSTALL_MODE" in
        user)
            INSTALL_PREFIX="$HOME/.local"
            INSTALL_BIN_DIR="$INSTALL_PREFIX/bin"
            ;;
        system)
            INSTALL_PREFIX="$SYSTEM_PREFIX"
            INSTALL_BIN_DIR="$SYSTEM_BIN_DIR"
            ;;
        *)
            die "Invalid install mode: $INSTALL_MODE"
            ;;
    esac

    USER_BIN_DIR="$INSTALL_PREFIX/bin"
    USER_LIB_DIR="$INSTALL_PREFIX/lib"
    USER_LIBEXEC_DIR="$INSTALL_PREFIX/libexec"
    USER_SHARE_DIR="$INSTALL_PREFIX/share"
    USER_DOC_DIR="$INSTALL_PREFIX/share/doc"

    USER_COSH_DIR="$HOME/.copilot-shell"
    USER_COSH_EXTENSIONS_DIR="$USER_COSH_DIR/extensions"
    USER_COSH_SKILLS_DIR="$USER_COSH_DIR/skills"
    INSTALL_EXTENSIONS_DIR="$USER_COSH_EXTENSIONS_DIR"
    [[ "$INSTALL_MODE" == "system" ]] && INSTALL_EXTENSIONS_DIR="/usr/share/anolisa/extensions"

    load_sec_core_make_paths
}

system_service_dir() {
    if [[ -d /usr/lib/systemd/system || "$INSTALL_MODE" == "system" ]]; then
        echo "/usr/lib/systemd/system"
    else
        echo "/etc/systemd/system"
    fi
}

systemd_is_available() {
    cmd_exists systemctl && [[ -d /run/systemd/system ]]
}

refresh_systemd_service() {
    local service="$1"

    [[ "$INSTALL_MODE" == "system" ]] || return 0
    if $DRY_RUN; then
        echo "DRY-RUN: systemctl daemon-reload"
        echo "DRY-RUN: systemctl enable $service"
        echo "DRY-RUN: systemctl restart $service"
        return 0
    fi

    if ! systemd_is_available; then
        warn "systemd is not active; installed ${service} but skipped enable/restart"
        return 0
    fi

    as_root systemctl daemon-reload || warn "systemctl daemon-reload failed"
    as_root systemctl enable "$service" || warn "systemctl enable $service failed"
    as_root systemctl restart "$service" || warn "systemctl restart $service failed"
}

stop_systemd_service() {
    local service="$1"

    [[ "$INSTALL_MODE" == "system" ]] || return 0
    if $DRY_RUN; then
        echo "DRY-RUN: systemctl stop $service"
        echo "DRY-RUN: systemctl disable $service"
        echo "DRY-RUN: systemctl daemon-reload"
        return 0
    fi

    if ! systemd_is_available; then
        return 0
    fi

    as_root systemctl stop "$service" 2>/dev/null || true
    as_root systemctl disable "$service" 2>/dev/null || true
    as_root systemctl daemon-reload || warn "systemctl daemon-reload failed"
}

stop_systemd_service_for_install() {
    local service="$1"

    [[ "$INSTALL_MODE" == "system" ]] || return 0
    if $DRY_RUN; then
        echo "DRY-RUN: systemctl stop $service"
        return 0
    fi

    if ! systemd_is_available; then
        return 0
    fi

    as_root systemctl stop "$service" 2>/dev/null || true
}

# ─── distro detection ───

DISTRO_ID=""        # alinux, ubuntu, fedora, centos, anolis, etc.
DISTRO_VER=""       # 4, 24.04, 9, etc.
DISTRO_VER_MAJOR="" # 4, 24, 9, etc.
PKG_BASE=""         # rpm | deb
PKG_INSTALL=""

detect_distro() {
    [[ -f /etc/os-release ]] || die "Cannot detect distro (no /etc/os-release). Linux only."
    # shellcheck source=/dev/null
    source /etc/os-release
    DISTRO_ID="${ID:-}"
    DISTRO_VER="${VERSION_ID:-}"
    DISTRO_VER_MAJOR="${DISTRO_VER%%.*}"
    local id_like="${ID_LIKE:-}"

    if [[ "$DISTRO_ID" =~ ^(fedora|rhel|centos|anolis|alinux)$ ]] || [[ "$id_like" =~ (fedora|rhel) ]]; then
        PKG_BASE="rpm"
        if cmd_exists dnf; then PKG_INSTALL="dnf install -y"
        elif cmd_exists yum; then PKG_INSTALL="yum install -y"
        else die "Neither dnf nor yum found"; fi
    elif [[ "$DISTRO_ID" =~ ^(debian|ubuntu)$ ]] || [[ "$id_like" =~ debian ]]; then
        PKG_BASE="deb"
        PKG_INSTALL="apt-get install -y"
    else
        die "Unsupported distro: ${PRETTY_NAME:-$DISTRO_ID}. Supported: Fedora/RHEL/CentOS/Anolis/Alinux, Debian/Ubuntu."
    fi

    ok "Distro: ${PRETTY_NAME:-$DISTRO_ID} (${PKG_BASE}, id=${DISTRO_ID}, ver=${DISTRO_VER})"
}

# ─── component helpers ───

# Default components (sight is excluded — it is optional and provides audit
# capabilities only; use --component sight to include it explicitly).
DEFAULT_COMPONENTS=(cosh skills sec-core tokenless ws-ckpt memory)
ALL_COMPONENTS=(cosh skills sec-core tokenless ws-ckpt memory sight)

active_components() {
    if [[ ${#COMPONENTS[@]} -eq 0 ]]; then
        printf '%s\n' "${DEFAULT_COMPONENTS[@]}"
    else
        printf '%s\n' "${COMPONENTS[@]}"
    fi
}

join_by() {
    local sep="$1"; shift
    local first=true item
    for item in "$@"; do
        if $first; then
            printf '%s' "$item"
            first=false
        else
            printf '%s%s' "$sep" "$item"
        fi
    done
}

selected_components_text() {
    local items=()
    while IFS= read -r item; do
        items+=("$item")
    done < <(active_components)
    join_by ", " "${items[@]}"
}

is_valid_component() {
    local c="$1" v
    for v in "${ALL_COMPONENTS[@]}"; do
        [[ "$v" == "$c" ]] && return 0
    done
    return 1
}

want_component() {
    local c="$1"
    if [[ ${#COMPONENTS[@]} -eq 0 ]]; then
        local d
        for d in "${DEFAULT_COMPONENTS[@]}"; do
            if [[ "$d" == "$c" ]]; then return 0; fi
        done
        return 1
    fi
    local x
    for x in "${COMPONENTS[@]}"; do
        if [[ "$x" == "$c" ]]; then return 0; fi
    done
    return 1
}

# ─── dependency installation ───

# Query the highest version of a package available in the configured system repositories.
# Prints semver string (e.g. "20.18.0") or nothing if the package is not found.
query_repo_ver() {
    local pkg="$1"
    if [[ "$PKG_BASE" == "rpm" ]]; then
        # dnf list output example: "nodejs.x86_64    1:20.18.0-1.alnx4    appstream"
        local raw
        raw=$(dnf list "$pkg" 2>/dev/null | grep -E "^${pkg}\." | tail -1)
        [[ -z "$raw" ]] && raw=$(yum list "$pkg" 2>/dev/null | grep -E "^${pkg}\." | tail -1)
        if [[ -n "$raw" ]]; then
            local nvr
            nvr=$(echo "$raw" | awk '{print $2}')
            nvr="${nvr#*:}"   # strip epoch (e.g. "1:20.18.0-1" → "20.18.0-1")
            extract_ver "$nvr"
            return
        fi
    elif [[ "$PKG_BASE" == "deb" ]]; then
        # apt-cache policy output: "  Candidate: 18.19.0+dfsg-6ubuntu5"
        local candidate
        candidate=$(apt-cache policy "$pkg" 2>/dev/null | sed -n 's/.*Candidate: *//p')
        if [[ -n "$candidate" && "$candidate" != "(none)" ]]; then
            extract_ver "$candidate"
            return
        fi
    fi
}

install_node() {
    step "Node.js (for copilot-shell)"
    local REQUIRED="20.0.0"

    local node_pkg="nodejs" npm_pkg="npm"

    _node_ver_ok() {
        cmd_exists node || return 1
        local v
        v=$(extract_ver "$(node -v 2>/dev/null)" || echo "")
        [[ -n "$v" ]] && ver_gte "$v" "$REQUIRED"
    }

    _source_nvm() {
        export NVM_DIR="${NVM_DIR:-$HOME/.nvm}"
        # shellcheck source=/dev/null
        if [[ -s "$NVM_DIR/nvm.sh" ]]; then source "$NVM_DIR/nvm.sh"; fi
    }

    _configure_npm_mirror

    if _node_ver_ok; then
        ok "Node.js $(node -v) already installed, skipping"
        return 0
    fi

    local repo_ver
    repo_ver=$(query_repo_ver "$node_pkg")
    if [[ -n "$repo_ver" ]] && ver_gte "$repo_ver" "$REQUIRED"; then
        info "Repository provides $node_pkg $repo_ver (>= $REQUIRED), installing via $PKG_BASE ..."
        if [[ "$PKG_BASE" == "deb" ]]; then sudo apt-get update -y 2>/dev/null || true; fi
        sudo $PKG_INSTALL $node_pkg $npm_pkg 2>/dev/null || true
        if _node_ver_ok; then
            ok "Node.js $(node -v) installed via package manager"
            return 0
        fi
        warn "Package manager install did not satisfy version requirement"
    else
        info "Repository $node_pkg${repo_ver:+ $repo_ver} does not meet >= $REQUIRED"
    fi

    info "Installing Node.js via nvm ..."

    if [[ "${SHELL}" == */zsh ]]; then touch "$HOME/.zshrc"; else touch "$HOME/.bashrc"; fi

    if ! cmd_exists nvm; then _source_nvm; fi

    if ! cmd_exists nvm; then
        info "Installing nvm ..."
        local NVM_VERSION="v0.40.3"
        export NVM_DIR="${NVM_DIR:-$HOME/.nvm}"
        # Disable interactive git prompts so clone fails fast instead of hanging
        export GIT_TERMINAL_PROMPT=0
        export GIT_ASKPASS=/bin/true
        local _nvm_script

        # Probe GitHub reachability (the official install.sh internally runs
        # `git clone github.com`, which hangs indefinitely when GitHub is
        # blocked — so we only try it when GitHub is actually reachable).
        local _github_ok=false
        if curl -sSf --connect-timeout 5 --max-time 10 \
                -o /dev/null https://github.com 2>/dev/null; then
            _github_ok=true
        fi

        if $_github_ok; then
            _nvm_script=$(mktemp /tmp/nvm-install-XXXXXX.sh)
            curl -fsSL --connect-timeout 10 --max-time 30 \
                "https://raw.githubusercontent.com/nvm-sh/nvm/${NVM_VERSION}/install.sh" \
                -o "$_nvm_script" 2>/dev/null || true
            [[ -s "$_nvm_script" ]] && bash "$_nvm_script" 2>/dev/null || true
            rm -f "$_nvm_script"
            _source_nvm
        else
            info "GitHub not reachable, skipping official installer"
        fi

        if ! cmd_exists nvm; then
            warn "Cloning nvm from Gitee mirror ..."
            if [[ -d "$NVM_DIR" && ! -s "$NVM_DIR/nvm.sh" ]]; then
                rm -rf "$NVM_DIR"
            fi
            if [[ ! -d "$NVM_DIR" ]]; then
                git clone --depth=1 --branch "$NVM_VERSION" \
                    https://gitee.com/mirrors/nvm.git "$NVM_DIR" 2>/dev/null \
                    || git clone https://gitee.com/mirrors/nvm.git "$NVM_DIR" 2>/dev/null || true
                if [[ -d "$NVM_DIR/.git" ]]; then
                    (cd "$NVM_DIR" && \
                        git checkout "$NVM_VERSION" 2>/dev/null \
                        || git checkout "$(git describe --abbrev=0 --tags --match "v[0-9]*" 2>/dev/null)" 2>/dev/null \
                        || true)
                fi
            fi
            local _rc="$HOME/.bashrc"
            [[ "${SHELL}" == */zsh ]] && _rc="$HOME/.zshrc"
            if [[ -s "$NVM_DIR/nvm.sh" ]] && ! grep -q 'NVM_DIR' "$_rc" 2>/dev/null; then
                {
                    echo ''
                    echo 'export NVM_DIR="$HOME/.nvm"'
                    echo '[ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh"'
                    echo '[ -s "$NVM_DIR/bash_completion" ] && \. "$NVM_DIR/bash_completion"'
                } >> "$_rc"
            fi
            _source_nvm
        fi
    fi
    cmd_exists nvm || die "Failed to install nvm"

    nvm install 20 || die "nvm install 20 failed; check network or mirror settings"

    _configure_npm_mirror

    if _node_ver_ok; then
        ok "Node.js $(node -v), npm $(npm -v)"
        info "nvm was sourced for this session; open a new terminal (or run: source ~/.bashrc) to persist"
    else
        die "Failed to install Node.js >= $REQUIRED"
    fi
}

install_build_tools() {
    step "Build tools (make, g++)"

    local missing=()
    if ! cmd_exists make; then missing+=("make"); fi
    if ! cmd_exists patch; then missing+=("patch"); fi

    if [[ "$PKG_BASE" == "rpm" ]]; then
        if ! cmd_exists g++; then missing+=("gcc-c++"); fi
    else
        if ! cmd_exists g++; then missing+=("g++"); fi
    fi

    if [[ ${#missing[@]} -eq 0 ]]; then
        ok "Build tools already installed, skipping"
        return 0
    fi

    info "Installing: ${missing[*]}"
    # shellcheck disable=SC2086
    sudo $PKG_INSTALL "${missing[@]}"
    ok "Build tools installed"
}

install_rust() {
    step "Rust (for agent-sec-core, agentsight, tokenless, ws-ckpt, agent-memory)"
    local REQUIRED="1.91.0"

    local rust_pkg="rust" cargo_pkg="cargo"
    if [[ "$PKG_BASE" == "deb" ]]; then rust_pkg="rustc"; fi

    _source_cargo() {
        # shellcheck source=/dev/null
        if [[ -f "$HOME/.cargo/env" ]]; then source "$HOME/.cargo/env"; fi
    }

    _rust_ver_ok() {
        cmd_exists rustc && cmd_exists cargo || return 1
        local v
        v=$(extract_ver "$(rustc --version 2>/dev/null)" || echo "")
        [[ -n "$v" ]] && ver_gte "$v" "$REQUIRED"
    }

    _source_cargo
    _configure_cargo_mirror

    if _rust_ver_ok; then
        ok "Rust $(extract_ver "$(rustc --version)") already installed, skipping"
        return 0
    fi

    # If rustc exists but too old and rustup is available, try updating first.
    # Use a stable-channel mirror only for this command; the global
    # RUSTUP_DIST_SERVER remains selected for sec-core's pinned Rust toolchain.
    if cmd_exists rustup; then
        info "Updating via rustup ..."
        local stable_picked stable_dist stable_update_root
        stable_picked=$(_pick_rustup_stable_mirror 2>/dev/null || echo "")
        if [[ -n "$stable_picked" ]]; then
            stable_dist="${stable_picked%%|*}"
            stable_update_root="${stable_picked##*|}"
            info "Rust stable channel mirror: ${stable_dist}"
            RUSTUP_DIST_SERVER="$stable_dist" \
                RUSTUP_UPDATE_ROOT="$stable_update_root" \
                rustup update stable || warn "rustup update stable failed; continuing with other Rust install methods"
        else
            rustup update stable || warn "rustup update stable failed; continuing with other Rust install methods"
        fi
        _source_cargo
        if _rust_ver_ok; then
            ok "Rust updated to $(extract_ver "$(rustc --version)") via rustup"
            return 0
        fi
    fi

    local repo_ver=""
    repo_ver=$(query_repo_ver "$rust_pkg")

    # DEB repos may ship versioned packages (rustc-1.XX) — pick the best one
    if [[ "$PKG_BASE" == "deb" ]]; then
        if [[ -z "$repo_ver" ]] || ! ver_gte "$repo_ver" "$REQUIRED"; then
            local best_pkg="" best_ver="" p pv
            while IFS= read -r p; do
                [[ -z "$p" ]] && continue
                pv=$(query_repo_ver "$p")
                [[ -z "$pv" ]] && continue
                if ver_gte "$pv" "$REQUIRED"; then
                    if [[ -z "$best_ver" ]] || ver_gte "$pv" "$best_ver"; then
                        best_pkg="$p"; best_ver="$pv"
                    fi
                fi
            done < <(apt-cache search '^rustc-[0-9]' 2>/dev/null | awk '{print $1}' | sort -V)
            if [[ -n "$best_pkg" ]]; then
                rust_pkg="$best_pkg"
                cargo_pkg="${best_pkg/rustc/cargo}"
                repo_ver="$best_ver"
            fi
        fi
    fi

    if [[ -n "$repo_ver" ]] && ver_gte "$repo_ver" "$REQUIRED"; then
        info "Repository provides $rust_pkg $repo_ver (>= $REQUIRED), installing via $PKG_BASE ..."
        sudo $PKG_INSTALL "$rust_pkg" "$cargo_pkg" gcc make || true

        # For versioned DEB packages (e.g. rustc-1.91), set up alternatives
        if [[ "$PKG_BASE" == "deb" && "$rust_pkg" != "rustc" ]]; then
            local suffix="${rust_pkg#rustc-}"
            if cmd_exists update-alternatives; then
                sudo update-alternatives --install /usr/bin/cargo cargo "/usr/bin/cargo-${suffix}" 100 2>/dev/null || true
            fi
        fi

        if _rust_ver_ok; then
            ok "Rust $(extract_ver "$(rustc --version)") installed via package manager"
            info "Note: agent-sec-core pins Rust ${SEC_CORE_RUST_TOOLCHAIN} via rust-toolchain.toml; rustup will auto-download if needed"
            return 0
        fi
        warn "Package manager install did not satisfy version requirement"
    else
        info "Repository ${rust_pkg}${repo_ver:+ $repo_ver} does not meet >= $REQUIRED"
    fi

    info "Installing Rust via rustup ..."
    sudo $PKG_INSTALL gcc make 2>/dev/null || true

    # Multi-level mirror fallback: official → Aliyun internal → Aliyun public → rsproxy.cn
    local _rustup_script
    _rustup_script=$(mktemp /tmp/rustup-init-XXXXXX.sh)
    curl --proto '=https' --tlsv1.2 -sSf --connect-timeout 15 --max-time 120 \
        https://sh.rustup.rs \
        -o "$_rustup_script" 2>/dev/null || true
    [[ -s "$_rustup_script" ]] && sh "$_rustup_script" -y 2>/dev/null || true
    rm -f "$_rustup_script"
    _source_cargo
    if ! cmd_exists rustc; then
        warn "rustup.rs unreachable, trying China mirrors ..."
        _rustup_script=$(mktemp /tmp/rustup-init-XXXXXX.sh)
        curl -sSf --connect-timeout 15 --max-time 60 \
            http://mirrors.cloud.aliyuncs.com/repo/rust/rustup-init.sh \
            -o "$_rustup_script" 2>/dev/null || true
        [[ -s "$_rustup_script" ]] && sh "$_rustup_script" -y 2>/dev/null || true
        rm -f "$_rustup_script"
        _source_cargo
    fi
    if ! cmd_exists rustc; then
        _rustup_script=$(mktemp /tmp/rustup-init-XXXXXX.sh)
        curl --proto '=https' --tlsv1.2 -sSf --connect-timeout 15 --max-time 120 \
            https://mirrors.aliyun.com/repo/rust/rustup-init.sh \
            -o "$_rustup_script" 2>/dev/null || true
        [[ -s "$_rustup_script" ]] && sh "$_rustup_script" -y 2>/dev/null || true
        rm -f "$_rustup_script"
        _source_cargo
    fi
    if ! cmd_exists rustc; then
        _rustup_script=$(mktemp /tmp/rustup-init-XXXXXX.sh)
        curl --proto '=https' --tlsv1.2 -sSf --connect-timeout 15 --max-time 120 \
            https://rsproxy.cn/rustup-init.sh \
            -o "$_rustup_script" 2>/dev/null || true
        [[ -s "$_rustup_script" ]] && sh "$_rustup_script" -y 2>/dev/null || true
        rm -f "$_rustup_script"
        _source_cargo
    fi

    if _rust_ver_ok; then
        ok "Rust $(extract_ver "$(rustc --version)"), cargo $(extract_ver "$(cargo --version)")"
    else
        die "Failed to install Rust >= $REQUIRED"
    fi
}

_configure_npm_mirror() {
    if [[ -z "${NVM_NODEJS_ORG_MIRROR:-}" ]]; then
        export NVM_NODEJS_ORG_MIRROR="https://npmmirror.com/mirrors/node/"
    fi
    export npm_config_registry="${npm_config_registry:-$NPM_REGISTRY}"
    export npm_config_replace_registry_host="${npm_config_replace_registry_host:-always}"

    if ! cmd_exists npm; then return 0; fi
    local current
    current=$(npm config get registry 2>/dev/null || echo "")
    if [[ "$current" == "$NPM_REGISTRY" || "$current" == "$NPM_REGISTRY/" ]]; then return 0; fi
    if [[ -n "$current" && "$current" != "https://registry.npmjs.org/" ]]; then
        info "Using npm registry for this build: $current"
        return 0
    fi
    npm config set registry "$NPM_REGISTRY"
    ok "npm registry mirror configured: $NPM_REGISTRY"
}

# Probe candidate rustup dist mirrors and pick the first reachable one.
# Returns the chosen base URL via stdout, or empty string on failure.
_rustup_host_triple() {
    if cmd_exists rustc; then
        rustc -vV 2>/dev/null | awk '/^host:/ { print $2; exit }'
        return 0
    fi

    case "$(uname -m 2>/dev/null || echo unknown)" in
        x86_64|amd64) echo "x86_64-unknown-linux-gnu" ;;
        aarch64|arm64) echo "aarch64-unknown-linux-gnu" ;;
        *) echo "x86_64-unknown-linux-gnu" ;;
    esac
}

_rustup_probe_path() {
    local host
    host="$(_rustup_host_triple)"
    echo "dist/rust-${SEC_CORE_RUST_TOOLCHAIN}-${host}.tar.gz.sha256"
}

_rustup_channel_path() {
    echo "dist/channel-rust-${SEC_CORE_RUST_TOOLCHAIN}.toml"
}

_rustup_dist_has_toolchain() {
    local base="$1"
    local toolchain_path="$2"
    local channel_path
    channel_path="$(_rustup_channel_path)"

    curl -sSfL --connect-timeout 3 --max-time 6 -o /dev/null \
        "$base/$channel_path" 2>/dev/null || return 1
    curl -sSfL --connect-timeout 3 --max-time 6 -o /dev/null \
        "$base/$toolchain_path" 2>/dev/null
}

_pick_rustup_mirror() {
    local candidates=(
        "https://rsproxy.cn|https://rsproxy.cn/rustup"
        "https://mirror.sjtu.edu.cn/rust-static|https://mirror.sjtu.edu.cn/rust-static/rustup"
        "https://mirrors.ustc.edu.cn/rust-static|https://mirrors.ustc.edu.cn/rust-static/rustup"
        "https://static.rust-lang.org|https://static.rust-lang.org/rustup"
    )
    # Probe both the versioned channel manifest and a real toolchain tarball
    # checksum. Some mirrors expose only one of them while rustup needs both.
    local probe_path
    probe_path="$(_rustup_probe_path)"
    local entry base
    for entry in "${candidates[@]}"; do
        base="${entry%%|*}"
        if _rustup_dist_has_toolchain "$base" "$probe_path"; then
            echo "$entry"
            return 0
        fi
    done
    return 1
}

_rustup_stable_dist_available() {
    local base="$1"

    curl -sSfL --connect-timeout 3 --max-time 6 -o /dev/null \
        "$base/dist/channel-rust-stable.toml.sha256" 2>/dev/null
}

_pick_rustup_stable_mirror() {
    local candidates=(
        "https://mirrors.tuna.tsinghua.edu.cn/rustup|https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup"
        "https://rsproxy.cn|https://rsproxy.cn/rustup"
        "https://mirrors.ustc.edu.cn/rust-static|https://mirrors.ustc.edu.cn/rust-static/rustup"
        "https://mirror.sjtu.edu.cn/rust-static|https://mirror.sjtu.edu.cn/rust-static/rustup"
        "https://static.rust-lang.org|https://static.rust-lang.org/rustup"
    )
    local entry base
    for entry in "${candidates[@]}"; do
        base="${entry%%|*}"
        if _rustup_stable_dist_available "$base"; then
            echo "$entry"
            return 0
        fi
    done
    return 1
}

_configure_cargo_mirror() {
    local _aliyun_internal=false
    if curl -sSf --connect-timeout 3 http://mirrors.cloud.aliyuncs.com/ &>/dev/null; then
        _aliyun_internal=true
    fi

    # Ensures rustup downloads from a reachable mirror (e.g. when
    # rust-toolchain.toml triggers an auto-install of a pinned version).
    # This is CRITICAL: when cargo build encounters rust-toolchain.toml,
    # rustup silently downloads the pinned toolchain (7+ components, ~300MB)
    # from the configured dist server — defaulting to static.rust-lang.org,
    # which is effectively unreachable from China and causes long hangs.
    local picked dist update_root probe_path
    probe_path="$(_rustup_probe_path)"
    if [[ -n "${RUSTUP_DIST_SERVER:-}" ]]; then
        if _rustup_dist_has_toolchain "$RUSTUP_DIST_SERVER" "$probe_path"; then
            info "RUSTUP_DIST_SERVER=${RUSTUP_DIST_SERVER}"
        else
            warn "RUSTUP_DIST_SERVER=${RUSTUP_DIST_SERVER} cannot serve Rust ${SEC_CORE_RUST_TOOLCHAIN}; selecting fallback mirror"
            picked=$(_pick_rustup_mirror 2>/dev/null || echo "")
            if [[ -n "$picked" ]]; then
                dist="${picked%%|*}"
                update_root="${picked##*|}"
                export RUSTUP_DIST_SERVER="$dist"
                export RUSTUP_UPDATE_ROOT="$update_root"
                info "RUSTUP_DIST_SERVER=${RUSTUP_DIST_SERVER}"
            else
                warn "No fallback rustup mirror verified for ${SEC_CORE_RUST_TOOLCHAIN}"
            fi
        fi
    else
        picked=$(_pick_rustup_mirror 2>/dev/null || echo "")
        if [[ -n "$picked" ]]; then
            dist="${picked%%|*}"
            update_root="${picked##*|}"
            export RUSTUP_DIST_SERVER="$dist"
            export RUSTUP_UPDATE_ROOT="$update_root"
            info "RUSTUP_DIST_SERVER=${RUSTUP_DIST_SERVER}"
        else
            # No mirror reachable — fall back to rsproxy.cn and let rustup surface any error
            export RUSTUP_DIST_SERVER="https://rsproxy.cn"
            export RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup"
            warn "No rustup mirror reachable; falling back to ${RUSTUP_DIST_SERVER}"
        fi
    fi

    local cargo_home="${CARGO_HOME:-$HOME/.cargo}"
    local cargo_config="$cargo_home/config.toml"
    local cargo_config_legacy="$cargo_home/config"
    # Skip if user already has a custom registry configured
    if [[ -f "$cargo_config" ]] && grep -q '\[source\.' "$cargo_config" 2>/dev/null; then
        info "Existing cargo registry config found, skipping crates.io mirror setup"
        return 0
    fi
    if [[ -f "$cargo_config_legacy" ]] && grep -q '\[source\.' "$cargo_config_legacy" 2>/dev/null; then
        info "Existing cargo registry config found, skipping crates.io mirror setup"
        return 0
    fi

    local mirror_url
    if $_aliyun_internal; then
        mirror_url="sparse+http://mirrors.cloud.aliyuncs.com/crates.io-index/"
        info "Using Aliyun internal crates.io mirror"
    else
        mirror_url="sparse+https://mirrors.aliyun.com/crates.io-index/"
        info "Using Aliyun public crates.io mirror"
    fi

    mkdir -p "$cargo_home"
    if ! grep -q '\[source\.crates-io\]' "$cargo_config" 2>/dev/null; then
        cat >> "$cargo_config" <<EOF

[source.crates-io]
replace-with = 'aliyun'
[source.aliyun]
registry = "$mirror_url"
EOF
    fi
    ok "crates.io mirror configured in $cargo_config"
}

_configure_git_mirror() {
    # Configure a reachable GitHub mirror for git operations when
    # github.com is blocked (e.g. ECS instances in China).
    # IMPORTANT: we write to --global (not --local) so that git-clone processes
    # (e.g. justfile setup-rtk) also inherit the insteadOf rule.
    local repo_dir="${1:-.}"

    if curl -sSf --connect-timeout 3 --max-time 6 -o /dev/null https://github.com 2>/dev/null; then
        return 0
    fi

    local existing
    existing=$(git config --global --get-regexp 'url\..*insteadOf' 2>/dev/null | grep -i github | head -1 || true)
    if [[ -n "$existing" ]]; then
        info "Git insteadOf already configured: $existing"
        return 0
    fi

    info "GitHub unreachable, probing mirrors ..."
    local mirror_base mirror_full
    local candidates=(
        "https://gh-proxy.com"
        "https://ghps.cc"
        "https://mirror.ghproxy.com"
        "https://ghproxy.com"
        "https://gitclone.com"
    )
    local c
    for c in "${candidates[@]}"; do
        if curl -sSf --connect-timeout 3 --max-time 6 -o /dev/null "$c/" 2>/dev/null; then
            mirror_base="${c}/"
            mirror_full="${c}/https://github.com/"
            break
        fi
    done

    if [[ -z "${mirror_base:-}" ]]; then
        warn "All GitHub mirrors unreachable; submodule clone may fail"
        return 0
    fi

    git config --global "url.${mirror_full}.insteadOf" "https://github.com/"
    ok "Git mirror (global): $mirror_base"
}

_configure_uv_mirror() {
    # Configure mirrors for uv (and pip3 as fallback).
    # uv respects these env vars and ~/.config/uv/uv.toml.
    local aliyun_pypi="https://mirrors.aliyun.com/pypi/simple/"
    local python_install_mirror="${UV_PYTHON_INSTALL_MIRROR:-https://mirror.nju.edu.cn/github-release/astral-sh/python-build-standalone}"

    export UV_INDEX_URL="$aliyun_pypi"
    export UV_DEFAULT_INDEX="$aliyun_pypi"
    export UV_PYTHON_INSTALL_MIRROR="$python_install_mirror"
    export PIP_INDEX_URL="$aliyun_pypi"

    local uv_cfg="$HOME/.config/uv/uv.toml"
    if [[ ! -f "$uv_cfg" ]]; then
        mkdir -p "$(dirname "$uv_cfg")"
        cat > "$uv_cfg" <<EOF
# uv configuration — managed by build-all.sh
python-install-mirror = "$python_install_mirror"

[[index]]
url = "https://mirrors.aliyun.com/pypi/simple/"
default = true
EOF
        ok "uv PyPI mirror configured: $aliyun_pypi"
        ok "uv Python install mirror configured: $python_install_mirror"
        return 0
    fi

    if ! grep -Eq '^[[:space:]]*python-install-mirror[[:space:]]*=' "$uv_cfg" 2>/dev/null; then
        local tmp_cfg
        tmp_cfg=$(mktemp)
        {
            echo "python-install-mirror = \"$python_install_mirror\""
            echo ""
            cat "$uv_cfg"
        } > "$tmp_cfg" && mv "$tmp_cfg" "$uv_cfg"
        ok "uv Python install mirror configured: $python_install_mirror"
    fi

    if ! grep -q 'mirrors.aliyun.com/pypi/simple/' "$uv_cfg" 2>/dev/null; then
        cat >> "$uv_cfg" <<'EOF'

[[index]]
url = "https://mirrors.aliyun.com/pypi/simple/"
default = true
EOF
        ok "uv PyPI mirror configured: $aliyun_pypi"
    fi
}

install_uv() {
    step "uv (Python package manager, for agent-sec-core)"

    if cmd_exists uv; then
        ok "uv $(extract_ver "$(uv --version 2>/dev/null)") already installed, skipping"
        return 0
    fi

    if cmd_exists pip3; then
        info "Trying: pip3 install uv ..."
        pip3 install uv 2>/dev/null || true
        if cmd_exists uv; then
            ok "uv $(extract_ver "$(uv --version 2>/dev/null)") installed via pip3"
            return 0
        fi
    fi

    if ! cmd_exists pipx; then
        info "Trying to install pipx via package manager ..."
        sudo $PKG_INSTALL pipx 2>/dev/null || true
    fi
    if cmd_exists pipx; then
        info "Trying: pipx install uv ..."
        pipx ensurepath 2>/dev/null || true
        export PATH="$HOME/.local/bin:$PATH"
        pipx install uv 2>/dev/null || true
        if cmd_exists uv; then
            ok "uv $(extract_ver "$(uv --version 2>/dev/null)") installed via pipx"
            return 0
        fi
    fi

    info "Installing uv via upstream installer ..."
    local _uv_script
    _uv_script=$(mktemp /tmp/uv-install-XXXXXX.sh)
    curl -LsSf --connect-timeout 15 --max-time 60 \
        https://astral.sh/uv/install.sh \
        -o "$_uv_script" 2>/dev/null || true
    [[ -s "$_uv_script" ]] && sh "$_uv_script" 2>/dev/null || true
    rm -f "$_uv_script"
    if [[ -f "$HOME/.local/bin/env" ]]; then
        # shellcheck source=/dev/null
        source "$HOME/.local/bin/env"
    fi
    export PATH="$HOME/.local/bin:$PATH"
    if ! cmd_exists uv; then
        warn "astral.sh unreachable, trying GitHub mirror ..."
        _uv_script=$(mktemp /tmp/uv-install-XXXXXX.sh)
        curl -LsSf --connect-timeout 15 --max-time 60 \
            https://github.com/astral-sh/uv/releases/latest/download/uv-installer.sh \
            -o "$_uv_script" 2>/dev/null || true
        [[ -s "$_uv_script" ]] && sh "$_uv_script" 2>/dev/null || true
        rm -f "$_uv_script"
        if [[ -f "$HOME/.local/bin/env" ]]; then
            # shellcheck source=/dev/null
            source "$HOME/.local/bin/env"
        fi
    fi

    if cmd_exists uv; then
        ok "uv $(extract_ver "$(uv --version 2>/dev/null)")"
    else
        die "Failed to install uv"
    fi
}

check_ebpf_deps() {
    step "eBPF dependencies (for agentsight)"

    info "AgentSight requires clang, llvm, and libbpf headers from your system package manager."

    local missing=()

    if ! cmd_exists clang; then missing+=("clang"); fi
    if ! cmd_exists llvm-config && ! cmd_exists llvm-config-*; then missing+=("llvm"); fi

    if [[ "$PKG_BASE" == "rpm" ]]; then
        local pkgs=("libbpf-devel" "libbpf-static" "elfutils-libelf-devel" "zlib-devel" "openssl-devel" "perl" "perl-core" "perl-IPC-Cmd" "perl-FindBin" "pkg-config")
        local pkg
        for pkg in "${pkgs[@]}"; do
            if ! rpm -q "$pkg" &>/dev/null; then
                missing+=("$pkg")
            fi
        done

        if [[ ${#missing[@]} -eq 0 ]]; then
            ok "All eBPF packages present"
        else
            warn "Missing eBPF packages: ${missing[*]}"
            info "Install with: ${BOLD}sudo dnf install -y ${missing[*]}${NC}"

            if $INSTALL_DEPS; then
                info "Installing missing eBPF packages ..."
                # shellcheck disable=SC2086
                sudo $PKG_INSTALL "${missing[@]}"
                ok "eBPF packages installed"
            fi
        fi

    elif [[ "$PKG_BASE" == "deb" ]]; then
        local pkgs=("libbpf-dev" "libelf-dev" "zlib1g-dev" "libssl-dev" "perl")
        local kver
        kver=$(uname -r 2>/dev/null || echo "")
        if [[ -n "$kver" ]]; then
            pkgs+=("linux-headers-${kver}")
        fi
        local pkg
        for pkg in "${pkgs[@]}"; do
            if ! dpkg -s "$pkg" &>/dev/null 2>&1; then
                missing+=("$pkg")
            fi
        done

        if [[ ${#missing[@]} -eq 0 ]]; then
            ok "All eBPF packages present"
        else
            warn "Missing eBPF packages: ${missing[*]}"
            info "Install with: ${BOLD}sudo apt-get install -y ${missing[*]}${NC}"

            if $INSTALL_DEPS; then
                info "Updating package index ..."
                sudo apt-get update -y
                info "Installing missing eBPF packages ..."
                sudo $PKG_INSTALL "${missing[@]}"
                ok "eBPF packages installed"
            fi
        fi
    fi

    if [[ -f /sys/kernel/btf/vmlinux ]]; then
        ok "Kernel BTF support available"
    else
        warn "Kernel BTF not found (/sys/kernel/btf/vmlinux). agentsight requires CONFIG_DEBUG_INFO_BTF=y"
    fi
}

# ─── top-level dep installer ───

install_just() {
    step "just (command runner, for tokenless rtk setup)"

    if cmd_exists just; then
        ok "just already installed, skipping"
        return 0
    fi

    # just may have been installed alongside rustup; source cargo env first
    # shellcheck source=/dev/null
    [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"

    if cmd_exists cargo; then
        info "Installing just via cargo install ..."
        cargo install just 2>/dev/null || true
        if cmd_exists just; then
            ok "just installed via cargo install"
            return 0
        fi
    fi

    warn "'just' is required for tokenless build (rtk clone+patch). Install manually: cargo install just"
}

do_install_deps() {
    if $DRY_RUN; then
        step "Dependency plan"
        echo "DRY-RUN: detect Linux distribution and package manager"
        if want_component cosh || want_component sec-core; then
            echo "DRY-RUN: check/install Node.js and build tools if needed"
        fi
        if want_component sec-core || want_component sight || want_component tokenless || want_component ws-ckpt || want_component memory; then
            echo "DRY-RUN: check/install Rust toolchain if needed"
        fi
        if want_component tokenless; then
            echo "DRY-RUN: check/install just if needed"
        fi
        if want_component sec-core; then
            echo "DRY-RUN: check/install uv and configure Python mirrors if needed"
        fi
        if want_component sight; then
            echo "DRY-RUN: check agentsight eBPF dependencies"
        fi
        ok "Dependency setup plan generated"
        return 0
    fi

    step "Detecting system"
    detect_distro

    if want_component cosh || want_component sec-core; then
        install_node
        install_build_tools
    fi

    if want_component sec-core || want_component sight || want_component tokenless || want_component ws-ckpt || want_component memory; then
        install_rust
    fi

    if want_component tokenless; then
        install_just
    fi

    if want_component sec-core; then
        _configure_uv_mirror
        install_uv
    fi

    if want_component sight; then
        check_ebpf_deps
    fi

    echo ""
    ok "Dependency setup complete"
}

# ─── build functions ───

build_cosh() {
    step "Building copilot-shell"
    local dir="$PROJECT_ROOT/src/copilot-shell"
    [[ -d "$dir" ]] || die "Directory not found: $dir"
    cd "$dir"

    run_logged "npm install (deps)" make deps
    run_logged "esbuild + bundle" make build

    if $DRY_RUN; then
        stage_component_make_install "copilot-shell" "$dir"
        ok "copilot-shell build plan generated"
        return 0
    fi

    if [[ -f dist/cli.js ]]; then
        stage_component_make_install "copilot-shell" "$dir"
        ok "copilot-shell built successfully"
    else
        warn "Expected artifact dist/cli.js not found"
    fi
}

build_skills() {
    step "Preparing os-skills"
    local dir="$PROJECT_ROOT/src/os-skills"
    [[ -d "$dir" ]] || die "Directory not found: $dir"
    cd "$dir"

    local count=0
    count=$(find . -name "SKILL.md" 2>/dev/null | wc -l)
    count=$((count + 0)) # trim whitespace

    info "Found ${count} skill definitions (install step will deploy by mode)"

    stage_component_make_install "os-skills" "$dir"

    if $DRY_RUN; then
        ok "os-skills stage plan generated for $(component_target_dir os-skills)"
        return 0
    fi

    stage_adapter_manifest "os-skills" "$PROJECT_ROOT/src/os-skills/adapters/adapter-manifest.json"
    ok "os-skills staged to $(component_target_dir os-skills)"
}

build_sec_core() {
    step "Building agent-sec-core"
    local dir="$PROJECT_ROOT/src/agent-sec-core"
    [[ -d "$dir" ]] || die "Directory not found: $dir"
    cd "$dir"

    local component_root build_dir
    component_root="$(component_target_dir sec-core)"
    build_dir="$component_root/build"

    if $DRY_RUN; then
        echo "DRY-RUN: rm -rf $component_root"
        echo "DRY-RUN: mkdir -p $component_root"
        echo "DRY-RUN: (cd $dir && make build-all BUILD_DIR=$build_dir)"
        ok "agent-sec-core build plan generated"
        return 0
    fi

    rm -rf "$component_root"
    mkdir -p "$component_root"

    info "make build-all (sandbox + CLI + sec-core assets) ..."
    run_logged_timeout "${AGENT_SEC_BUILD_TIMEOUT:-1200}" \
        "make build-all (agent-sec-core)" \
        make build-all BUILD_DIR="$build_dir"

    if [[ -d "$build_dir/share" ]]; then
        rm -rf "$component_root/share"
        cp -a "$build_dir/share" "$component_root/share"
    fi

    local bin="$build_dir/linux-sandbox"
    if [[ -f "$bin" ]]; then
        ok "agent-sec-core built successfully"
    else
        warn "Expected artifact $bin not found"
    fi
}

build_sight() {
    step "Building agentsight"
    local dir="$PROJECT_ROOT/src/agentsight"
    [[ -d "$dir" ]] || die "Directory not found: $dir"
    cd "$dir"

    if [[ -f Makefile ]] && grep -q 'build' Makefile; then
        stage_component_make_install "agentsight" "$dir" \
            SERVICE_BINDIR="$SYSTEM_BIN_DIR" SETCAP=0 \
            NPM_REGISTRY="$NPM_REGISTRY" NPM_REPLACE_REGISTRY_HOST=always
        if $DRY_RUN; then
            ok "agentsight build plan generated"
            return 0
        fi
    else
        run_logged "cargo build (agentsight)" cargo build --release
        if $DRY_RUN; then
            echo "DRY-RUN: copy target/release/agentsight -> $(component_target_dir agentsight)/bin/agentsight"
            ok "agentsight build plan generated"
            return 0
        fi
        copy_file target/release/agentsight "$(component_target_dir agentsight)/bin/agentsight" 0755
    fi

    local bin="target/release/agentsight"
    if [[ -f "$bin" || -f "$(component_target_dir agentsight)/bin/agentsight" ]]; then
        ok "agentsight built successfully"
    else
        warn "Expected artifact $bin not found"
    fi
}

build_tokenless() {
    step "Building tokenless"
    local dir="$PROJECT_ROOT/src/tokenless"
    [[ -d "$dir" ]] || die "Directory not found: $dir"
    cd "$dir"

    # rtk setup is handled by Makefile build-tokenless target (just setup-rtk),
    # but 'just' must be available before make install runs.
    if ! $DRY_RUN; then
        if ! command -v just &>/dev/null; then
            die "'just' is required for tokenless build (rtk clone+patch orchestration). Install: cargo install just"
        fi
    else
        info "DRY-RUN: just setup-rtk would be called by Makefile build-tokenless"
    fi

    info "make install (tokenless workspace) ..."
    stage_component_make_install "tokenless" "$dir"
    if $DRY_RUN; then
        ok "tokenless build plan generated"
        return 0
    fi

    local component_root bin rtk_bin toon_bin
    component_root="$(component_target_dir tokenless)"
    bin="$component_root/bin/tokenless"
    rtk_bin="$component_root/libexec/anolisa/tokenless/rtk"
    toon_bin="$component_root/libexec/anolisa/tokenless/toon"
    if [[ -f "$bin" ]] && [[ -f "$rtk_bin" ]] && [[ -f "$toon_bin" ]]; then
        if [[ ! -d "$component_root/share/anolisa/adapters/tokenless" ]]; then
            warn "tokenless adapter resources staged empty"
        fi
        if [[ ! -d "$component_root/share/anolisa/extensions/tokenless" ]]; then
            warn "tokenless cosh extension staged empty"
        fi
        stage_adapter_manifest "tokenless" "$PROJECT_ROOT/src/tokenless/adapters/tokenless/manifest.json"
        ok "tokenless, rtk, and toon built successfully"
    else
        [[ -f "$bin" ]]     || warn "Expected artifact $bin not found"
        [[ -f "$rtk_bin" ]] || warn "Expected artifact $rtk_bin not found"
        [[ -f "$toon_bin" ]] || warn "Expected artifact $toon_bin not found"
    fi
}

build_wsckpt() {
    step "Building ws-ckpt"
    local dir="$PROJECT_ROOT/src/ws-ckpt"
    [[ -d "$dir" ]] || die "Directory not found: $dir"
    cd "$dir"

    stage_component_make_install "ws-ckpt" "$dir"
    if $DRY_RUN; then
        ok "ws-ckpt build plan generated"
        return 0
    fi

    local component_root bin
    component_root="$(component_target_dir ws-ckpt)"
    bin="$component_root/bin/ws-ckpt"
    if [[ -f "$bin" ]]; then
        stage_adapter_manifest "ws-ckpt" "$PROJECT_ROOT/src/ws-ckpt/adapter-manifest.json"
        ok "ws-ckpt built successfully"
    else
        warn "Expected artifact $bin not found"
    fi
}

build_agent_memory() {
    step "Building agent-memory"
    local dir="$PROJECT_ROOT/src/agent-memory"
    [[ -d "$dir" ]] || die "Directory not found: $dir"
    cd "$dir"

    # agent-memory needs cmake (for git2's vendored libgit2) and libsystemd
    # headers (for the journald audit fan-out); both are missing from the
    # default toolchain installs above.
    if ! $DRY_RUN; then
        local missing=()
        cmd_exists cmake || missing+=("cmake")
        if [[ "$PKG_BASE" == "rpm" ]] && ! rpm -q systemd-devel &>/dev/null; then
            missing+=("systemd-devel")
        elif [[ "$PKG_BASE" == "deb" ]] && ! dpkg -s libsystemd-dev &>/dev/null 2>&1; then
            missing+=("libsystemd-dev")
        fi
        if [[ ${#missing[@]} -gt 0 ]]; then
            warn "agent-memory native deps missing: ${missing[*]}"
            info "Install with: ${BOLD}sudo $PKG_INSTALL ${missing[*]}${NC}"
        fi
    fi

    stage_component_make_install "agent-memory" "$dir"
    if $DRY_RUN; then
        ok "agent-memory build plan generated"
        return 0
    fi

    local component_root bin
    component_root="$(component_target_dir agent-memory)"
    bin="$component_root/bin/agent-memory"
    if [[ -f "$bin" ]]; then
        ok "agent-memory built successfully"
    else
        warn "Expected artifact $bin not found"
    fi
}

do_build() {
    # shellcheck source=/dev/null
    [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
    # shellcheck source=/dev/null
    [[ -s "$HOME/.nvm/nvm.sh" ]] && { export NVM_DIR="$HOME/.nvm"; source "$HOME/.nvm/nvm.sh"; }
    export PATH="$HOME/.local/bin:$PATH"

    if $DRY_RUN; then
        if want_component sec-core || want_component sight || want_component tokenless || want_component ws-ckpt || want_component memory; then
            echo "DRY-RUN: configure cargo mirror for this build"
        fi
        if want_component cosh || want_component sec-core || want_component sight; then
            echo "DRY-RUN: configure npm registry for this build"
        fi
        if want_component sec-core; then
            echo "DRY-RUN: configure uv mirrors for this build"
        fi
        if want_component tokenless; then
            echo "DRY-RUN: configure git mirror for this build"
        fi
        echo "DRY-RUN: rm -rf $OUTPUT_DIR"
        echo "DRY-RUN: mkdir -p $OUTPUT_DIR"
    else
        if want_component sec-core || want_component sight || want_component tokenless || want_component ws-ckpt || want_component memory; then
            _configure_cargo_mirror
        fi
        if want_component cosh || want_component sec-core || want_component sight; then
            _configure_npm_mirror
        fi
        if want_component sec-core; then
            _configure_uv_mirror
        fi
        if want_component tokenless; then
            _configure_git_mirror "$PROJECT_ROOT"
        fi

        rm -rf "$OUTPUT_DIR"
        mkdir -p "$OUTPUT_DIR"

        : > "$LOG_FILE"
        info "Build log → $LOG_FILE"
    fi

    if want_component cosh;      then build_cosh;         fi
    if want_component skills;    then build_skills;       fi
    if want_component sec-core;  then build_sec_core;     fi
    if want_component tokenless; then build_tokenless;    fi
    if want_component ws-ckpt;   then build_wsckpt;       fi
    if want_component memory;    then build_agent_memory;   fi
    if want_component sight;     then build_sight;        fi
}

# ─── install functions ───

install_cosh() {
    step "Installing copilot-shell"
    local dir="$PROJECT_ROOT/src/copilot-shell"
    run_component_make_install "copilot-shell" "$dir"
    if $DRY_RUN; then
        ok "copilot-shell install plan generated"
    else
        ok "copilot-shell installed to ${INSTALL_BIN_DIR}/{cosh,co,copilot}"
    fi
}

install_skills() {
    step "Installing os-skills"
    local dir="$PROJECT_ROOT/src/os-skills"
    run_component_make_install "os-skills" "$dir"
    local skills_dir="/usr/share/anolisa/skills"
    [[ "$INSTALL_MODE" == "user" ]] && skills_dir="$USER_COSH_SKILLS_DIR"
    if $DRY_RUN; then
        ok "os-skills install plan generated for ${skills_dir}"
    else
        ok "os-skills installed to ${skills_dir}"
    fi
}

install_sec_core_runtime_deps() {
    if cmd_exists bwrap && { cmd_exists gpg || cmd_exists gpg2; } && cmd_exists jq; then
        return 0
    fi

    if [[ "$INSTALL_MODE" != "system" ]]; then
        cmd_exists bwrap || warn "bubblewrap not found; linux-sandbox may not run until it is installed."
        if ! cmd_exists gpg && ! cmd_exists gpg2; then
            warn "gpg/gpg2 not found; skill signature setup will need GnuPG."
        fi
        cmd_exists jq || warn "jq not found; sec-core helper scripts may need jq."
        return 0
    fi

    if [[ -z "$PKG_INSTALL" ]]; then
        detect_distro
    fi

    if ! cmd_exists bwrap; then
        info "Installing runtime dependency: bubblewrap ..."
        as_root $PKG_INSTALL bubblewrap || warn "bubblewrap not installed (linux-sandbox runtime dep)"
    fi
    if ! cmd_exists gpg && ! cmd_exists gpg2; then
        local gpg_pkg="gnupg2"
        [[ "$PKG_BASE" == "deb" ]] && gpg_pkg="gnupg"
        info "Installing runtime dependency: ${gpg_pkg} ..."
        as_root $PKG_INSTALL "$gpg_pkg" || warn "${gpg_pkg} not installed (skill signature verification)"
    fi
    if ! cmd_exists jq; then
        info "Installing runtime dependency: jq ..."
        as_root $PKG_INSTALL jq || warn "jq not installed (sec-core helper/signing dependency)"
    fi
}

install_sec_core() {
    step "Installing agent-sec-core"

    local staged build_dir
    staged="$(component_target_dir sec-core)"
    build_dir="$staged/build"

    local dir="$PROJECT_ROOT/src/agent-sec-core"
    [[ -d "$dir" ]] || die "Directory not found: $dir"

    if $DRY_RUN; then
        if [[ "$INSTALL_MODE" == "system" ]]; then
            echo "DRY-RUN: sudo env PATH=\$PATH UV_PYTHON_INSTALL_MIRROR=\${UV_PYTHON_INSTALL_MIRROR:-} make -C $dir install BUILD_DIR=$build_dir INSTALL_PROFILE=system"
        else
            echo "DRY-RUN: make -C $dir install BUILD_DIR=$build_dir INSTALL_PROFILE=user"
        fi
        echo "DRY-RUN: check/install sec-core runtime dependencies"
        ok "agent-sec-core install plan generated for $SEC_CORE_BIN_DIR and $SEC_CORE_LIB_DIR"
        return 0
    fi

    [[ -d "$build_dir" ]] || die "Build directory not found: $build_dir"
    [[ -f "$build_dir/linux-sandbox" ]] || die "Built linux-sandbox not found: $build_dir/linux-sandbox"
    [[ -d "$build_dir/cosh-extension" ]] || die "Built cosh extension not found: $build_dir/cosh-extension"
    [[ -d "$build_dir/openclaw-plugin" ]] || die "Built OpenClaw plugin not found: $build_dir/openclaw-plugin"
    [[ -d "$build_dir/hermes-plugin" ]] || die "Built hermes-plugin not found: $build_dir/hermes-plugin"
    [[ -d "$build_dir/skills" ]] || die "Built sec-core skills not found: $build_dir/skills"
    find "$build_dir/wheels" -maxdepth 1 -name 'agent_sec_cli-*.whl' -type f | grep -q . || \
        die "Built agent-sec-cli wheel not found under $build_dir/wheels"
    cmd_exists uv || die "uv not found; install dependencies first or run without --ignore-deps"

    _configure_uv_mirror

    if [[ "$INSTALL_MODE" == "system" ]]; then
        run_logged "make install (agent-sec-core)" \
            as_root env PATH="$PATH" \
                UV_PYTHON_INSTALL_MIRROR="${UV_PYTHON_INSTALL_MIRROR:-}" \
                make -C "$dir" install \
                BUILD_DIR="$build_dir" INSTALL_PROFILE=system
    else
        run_logged "make install (agent-sec-core)" \
            make -C "$dir" install \
                BUILD_DIR="$build_dir" INSTALL_PROFILE=user
    fi

    install_sec_core_runtime_deps

    ok "agent-sec-core installed to $SEC_CORE_BIN_DIR and $SEC_CORE_LIB_DIR"
    if [[ "$INSTALL_MODE" != "system" ]]; then
        info "Make sure $SEC_CORE_BIN_DIR is in PATH before starting integrations."
    fi
}

install_sight() {
    step "Installing agentsight"
    local dir="$PROJECT_ROOT/src/agentsight"
    local setcap_arg="SETCAP=0"
    if [[ "$INSTALL_MODE" == "system" ]]; then
        setcap_arg="SETCAP=0"
        stop_systemd_service_for_install agentsight.service
    fi
    run_component_make_install "agentsight" "$dir" "$setcap_arg"
    if [[ "$INSTALL_MODE" == "system" ]]; then
        if cmd_exists setcap; then
            run_cmd as_root setcap cap_bpf,cap_perfmon=ep "$INSTALL_BIN_DIR/agentsight" || \
                warn "setcap failed; agentsight trace may need sudo"
        else
            warn "setcap not found; agentsight trace may need sudo"
        fi
        refresh_systemd_service agentsight.service
    else
        warn "agentsight user install skips systemd/setcap; trace/audit may need sudo or manual setcap."
    fi
    if $DRY_RUN; then
        ok "agentsight install plan generated for ${INSTALL_BIN_DIR}/agentsight"
    else
        ok "agentsight installed to ${INSTALL_BIN_DIR}/agentsight"
    fi
}

install_tokenless() {
    step "Installing tokenless"
    local dir="$PROJECT_ROOT/src/tokenless"
    run_component_make_install "tokenless" "$dir"
    if $DRY_RUN; then
        ok "tokenless install plan generated for ${INSTALL_BIN_DIR}/"
    else
        ok "tokenless installed to ${INSTALL_BIN_DIR}/"
    fi
}

install_wsckpt_runtime_deps() {
    [[ "$INSTALL_MODE" == "system" ]] || return 0

    if $DRY_RUN; then
        echo "DRY-RUN: check/install ws-ckpt runtime dependency: btrfs-progs"
        return 0
    fi

    if cmd_exists mkfs.btrfs; then
        return 0
    fi

    if [[ -z "$PKG_INSTALL" ]]; then
        detect_distro
    fi

    info "Installing runtime dependency: btrfs-progs ..."
    as_root $PKG_INSTALL btrfs-progs || \
        warn "btrfs-progs not installed; ws-ckpt btrfs-loop backend may not start"
}

install_wsckpt() {
    step "Installing ws-ckpt"
    local dir="$PROJECT_ROOT/src/ws-ckpt"
    if [[ "$INSTALL_MODE" == "system" ]]; then
        stop_systemd_service_for_install ws-ckpt.service
    fi
    run_component_make_install "ws-ckpt" "$dir"
    if [[ "$INSTALL_MODE" == "system" ]]; then
        install_wsckpt_runtime_deps
        refresh_systemd_service ws-ckpt.service
    else
        info "Skipping ws-ckpt systemd service in user mode; use --system for service management."
    fi
    if $DRY_RUN; then
        ok "ws-ckpt install plan generated for ${INSTALL_BIN_DIR}/"
    else
        ok "ws-ckpt installed to ${INSTALL_BIN_DIR}/"
    fi
}

install_agent_memory() {
    step "Installing agent-memory"
    local dir="$PROJECT_ROOT/src/agent-memory"
    run_component_make_install "agent-memory" "$dir"
    # agent-memory ships a per-user systemd template
    # (anolisa-memory@.service); intentionally NOT enabled by default so
    # users can opt-in with `systemctl --user enable anolisa-memory@$USER`.
    if $DRY_RUN; then
        ok "agent-memory install plan generated for ${INSTALL_BIN_DIR}/"
    else
        ok "agent-memory installed to ${INSTALL_BIN_DIR}/"
    fi
}

do_install() {
    step "Installing components (mode=${INSTALL_MODE})"
    if want_component cosh;      then install_cosh;         fi
    if want_component skills;    then install_skills;       fi
    if want_component sec-core;  then install_sec_core;     fi
    if want_component tokenless; then install_tokenless;    fi
    if want_component ws-ckpt;   then install_wsckpt;       fi
    if want_component memory;    then install_agent_memory; fi
    if want_component sight;     then install_sight;        fi
}

# ─── uninstall functions ───

uninstall_cosh() {
    step "Uninstalling copilot-shell"
    local dir="$PROJECT_ROOT/src/copilot-shell"
    run_component_make_uninstall "copilot-shell" "$dir" || true
    if $DRY_RUN; then
        ok "copilot-shell uninstall plan generated"
    else
        ok "copilot-shell uninstalled"
    fi
}

uninstall_skills() {
    step "Uninstalling os-skills"
    local dir="$PROJECT_ROOT/src/os-skills"
    run_component_make_uninstall "os-skills" "$dir" || true
    if $DRY_RUN; then
        ok "os-skills uninstall plan generated"
    else
        ok "os-skills uninstalled"
    fi
}

uninstall_sec_core() {
    step "Uninstalling agent-sec-core"
    local dir="$PROJECT_ROOT/src/agent-sec-core"
    [[ -d "$dir" ]] || die "Directory not found: $dir"

    if $DRY_RUN; then
        if [[ "$INSTALL_MODE" == "system" ]]; then
            echo "DRY-RUN: sudo make -C $dir uninstall INSTALL_PROFILE=system"
        else
            echo "DRY-RUN: make -C $dir uninstall INSTALL_PROFILE=user"
        fi
        ok "agent-sec-core uninstall plan generated (mode=${INSTALL_MODE})"
        return 0
    fi

    if [[ "$INSTALL_MODE" == "system" ]]; then
        run_logged "make uninstall (agent-sec-core)" \
            as_root make -C "$dir" uninstall INSTALL_PROFILE=system || true
    else
        run_logged "make uninstall (agent-sec-core)" \
            make -C "$dir" uninstall INSTALL_PROFILE=user || true
    fi
    ok "agent-sec-core install removed (mode=${INSTALL_MODE})"
}

uninstall_sight() {
    step "Uninstalling agentsight"
    stop_systemd_service agentsight.service
    local dir="$PROJECT_ROOT/src/agentsight"
    run_component_make_uninstall "agentsight" "$dir" || true
    if $DRY_RUN; then
        ok "agentsight uninstall plan generated"
    else
        ok "agentsight uninstalled"
    fi
}

uninstall_tokenless() {
    step "Uninstalling tokenless"
    local dir="$PROJECT_ROOT/src/tokenless"
    run_component_make_uninstall "tokenless" "$dir" || true
    if $DRY_RUN; then
        ok "tokenless, rtk, and toon uninstall plan generated"
    else
        ok "tokenless, rtk, and toon uninstalled"
    fi
}

uninstall_wsckpt() {
    step "Uninstalling ws-ckpt"
    stop_systemd_service ws-ckpt.service
    local dir="$PROJECT_ROOT/src/ws-ckpt"
    run_component_make_uninstall "ws-ckpt" "$dir" || true
    if $DRY_RUN; then
        ok "ws-ckpt uninstall plan generated"
    else
        ok "ws-ckpt uninstalled"
    fi
}

uninstall_agent_memory() {
    step "Uninstalling agent-memory"
    local dir="$PROJECT_ROOT/src/agent-memory"
    run_component_make_uninstall "agent-memory" "$dir" || true
    if $DRY_RUN; then
        ok "agent-memory uninstall plan generated"
    else
        ok "agent-memory uninstalled"
    fi
}

do_uninstall() {
    step "Uninstalling components"
    if want_component cosh;      then uninstall_cosh;         fi
    if want_component skills;    then uninstall_skills;       fi
    if want_component sec-core;  then uninstall_sec_core;     fi
    if want_component tokenless; then uninstall_tokenless;    fi
    if want_component ws-ckpt;   then uninstall_wsckpt;       fi
    if want_component memory;    then uninstall_agent_memory; fi
    if want_component sight;     then uninstall_sight;        fi

    if [[ -d "$INSTALL_EXTENSIONS_DIR" ]] && [[ -z "$(ls -A "$INSTALL_EXTENSIONS_DIR" 2>/dev/null)" ]]; then
        if $DRY_RUN; then
            echo "DRY-RUN: remove empty $INSTALL_EXTENSIONS_DIR"
        elif [[ "$INSTALL_MODE" == "system" ]]; then
            as_root rm -rf "$INSTALL_EXTENSIONS_DIR"
        else
            rm -rf "$INSTALL_EXTENSIONS_DIR"
        fi
        if $DRY_RUN; then
            info "Empty $INSTALL_EXTENSIONS_DIR would be removed"
        else
            info "Removed empty $INSTALL_EXTENSIONS_DIR"
        fi
    fi
}

print_output_summary() {
    step "Output"

    if $DRY_RUN; then
        info "Dry-run mode: target/ is not changed."
        return 0
    fi

    if [[ ! -d "$OUTPUT_DIR" ]]; then
        warn "No target/ directory found"
        return 0
    fi

    local total
    total=$(find "$OUTPUT_DIR" -type f 2>/dev/null | wc -l | tr -d ' ')
    info "$total files staged → $OUTPUT_DIR"

    local component_dir component_files
    for component_dir in "$OUTPUT_DIR"/*; do
        [[ -d "$component_dir" ]] || continue
        component_files=$(find "$component_dir" -type f 2>/dev/null | wc -l | tr -d ' ')
        info "  $(basename "$component_dir"): ${component_files} files → $component_dir"
    done
}

prompt_choice() {
    # `read -p` writes the prompt to stderr, so command substitution
    # ($(prompt_choice ...)) only captures the printf'd answer below.
    local prompt="$1" default="$2" answer
    read -r -p "$prompt [$default]: " answer
    printf '%s' "${answer:-$default}"
}

run_interactive_wizard() {
    [[ -t 0 ]] || die "--interactive requires a TTY. Use --non-interactive for automation."

    echo -e "${BOLD}ANOLISA interactive setup${NC}"
    echo "Choose the build flow. Press Enter to accept defaults."
    echo ""

    local choice comps confirm
    echo "1) Build and install"
    echo "2) Build only"
    echo "3) Install dependencies only"
    echo "4) Uninstall"
    choice="$(prompt_choice "Action" "1")"
    case "$choice" in
        1)
            DO_INSTALL=true
            DEPS_ONLY=false
            DO_UNINSTALL=false
            ;;
        2)
            DO_INSTALL=false
            DEPS_ONLY=false
            DO_UNINSTALL=false
            ;;
        3)
            DO_INSTALL=false
            DEPS_ONLY=true
            INSTALL_DEPS=true
            DO_UNINSTALL=false
            ;;
        4)
            DO_UNINSTALL=true
            ;;
        *) die "Invalid action choice: $choice" ;;
    esac

    echo ""
    echo "1) User install (~/.local, ~/.copilot-shell)"
    echo "2) System install (/usr/local/bin, /usr/share/anolisa)"
    choice="$(prompt_choice "Install mode" "$([[ "$INSTALL_MODE" == "system" ]] && echo 2 || echo 1)")"
    case "$choice" in
        1) INSTALL_MODE="user" ;;
        2) INSTALL_MODE="system" ;;
        *) die "Invalid install mode choice: $choice" ;;
    esac

    echo ""
    echo "1) Default components: $(join_by ", " "${DEFAULT_COMPONENTS[@]}")"
    echo "2) All components: $(join_by ", " "${ALL_COMPONENTS[@]}")"
    echo "3) Custom list"
    choice="$(prompt_choice "Components" "$([[ ${#COMPONENTS[@]} -gt 0 ]] && echo 3 || echo 1)")"
    case "$choice" in
        1) COMPONENTS=("${DEFAULT_COMPONENTS[@]}") ;;
        2) COMPONENTS=("${ALL_COMPONENTS[@]}") ;;
        3)
            comps="$(prompt_choice "Comma-separated components" "$(selected_components_text)")"
            COMPONENTS=()
            comps="${comps//,/ }"
            for comp in $comps; do
                if is_valid_component "$comp"; then
                    COMPONENTS+=("$comp")
                else
                    die "Unknown component: $comp"
                fi
            done
            [[ ${#COMPONENTS[@]} -gt 0 ]] || die "No components selected"
            ;;
        *) die "Invalid component choice: $choice" ;;
    esac

    if ! $DO_UNINSTALL && ! $DEPS_ONLY; then
        echo ""
        choice="$(prompt_choice "Install/check dependencies" "$($INSTALL_DEPS && echo y || echo n)")"
        case "$choice" in
            y|Y|yes|YES) INSTALL_DEPS=true ;;
            n|N|no|NO) INSTALL_DEPS=false ;;
            *) die "Invalid dependency choice: $choice" ;;
        esac
    fi

    echo ""
    ensure_user_mode
    step "Selected flow"
    if $DO_UNINSTALL; then
        info "Action: uninstall"
    elif $DEPS_ONLY; then
        info "Action: dependencies only"
    elif $DO_INSTALL; then
        info "Action: build and install"
    else
        info "Action: build only"
    fi
    info "Mode: ${INSTALL_MODE}"
    info "Components: $(selected_components_text)"
    info "Dependencies: $($INSTALL_DEPS && echo enabled || echo skipped)"
    info "Install: $($DO_INSTALL && echo enabled || echo skipped)"
    echo ""
    confirm="$(prompt_choice "Continue" "y")"
    case "$confirm" in
        y|Y|yes|YES) ;;
        *) ok "Cancelled"; exit 0 ;;
    esac
}

# ─── usage ───

usage() {
    cat <<EOF
$(echo -e "${BOLD}ANOLISA Build Script${NC}")

$(echo -e "${BOLD}Usage:${NC}")
  $0 [OPTIONS]

$(echo -e "${BOLD}Options:${NC}")
    --no-install            Skip installing built components
    --install-mode <mode>   Install mode: user or system (default: user)
    --usr, --system         Use system install mode
    --ignore-deps           Skip dependency installation
    --deps-only             Install dependencies only, do not build
    --uninstall             Remove installed files (skips build; combine with --component to target one)
    --dry-run               Print actions without changing files or systemd state
    --interactive           Open a guided terminal flow before running
    --non-interactive       Explicit no-prompt mode; same as default, useful in CI to assert intent
    --all                   Include optional components such as sight
    --component <name>      Build/uninstall specific component (can be repeated).
                                                    Valid names: cosh, skills, sec-core, sight, tokenless, ws-ckpt
                                                    Default (no --component): cosh, skills, sec-core, tokenless, ws-ckpt
                                                    (sight is optional; use --all or --component sight)
    -h, --help              Show this help

$(echo -e "${BOLD}Examples:${NC}")
    $0                                             # Install deps + build + install to user paths
    $0 --interactive                               # Guided terminal flow
    $0 --non-interactive                           # Explicit automation mode (same as default)
    $0 --install-mode user                         # Explicit user install mode
    $0 --no-install                                # Install deps + build (skip installation)
    $0 --ignore-deps                               # Build + install (skip dep install)
    $0 --deps-only                                 # Install deps only
    $0 --all                                       # Build + install default components and agentsight
    $0 --component cosh                            # Install deps + build + install copilot-shell
    $0 --no-install                                # Build target/ staging only
    $0 --component sec-core                          # Build + install sec-core to user paths
    $0 --system --component sec-core                 # Build + install sec-core to FHS system paths
    $0 --ignore-deps --component sec-core            # Build + install sec-core to user paths (no dep install)
    $0 --uninstall                                 # Uninstall all default components
    $0 --uninstall --component cosh                # Uninstall copilot-shell only
    $0 --uninstall --component tokenless --component ws-ckpt
                                                     # Uninstall tokenless and ws-ckpt

$(echo -e "${BOLD}Components:${NC}")
  cosh     copilot-shell      Node.js / TypeScript AI terminal assistant       [default]
  skills   os-skills          Markdown skill definitions                         [default]
  sec-core agent-sec-core     Security CLI + sandbox + hooks                    [default]
  tokenless tokenless         Rust token compression library (cross-platform)   [default]
  ws-ckpt  ws-ckpt           Rust workspace checkpoint daemon                   [default]
  sight    agentsight         eBPF observability/audit agent (Linux only)        [optional]

$(echo -e "${BOLD}What this script does:${NC}")
  1. Detects installed toolchains and queries system repositories for available versions
  2. Installs via system package manager (dnf/yum/apt) when repository versions meet requirements
  3. Falls back to upstream installers (nvm, rustup, uv) when system packages don't suffice
  4. Builds default components in order: cosh -> skills -> sec-core -> tokenless -> ws-ckpt
     (sight is optional — add --all or --component sight to include it)
  5. Installs components to the selected profile layout
         - prefix: ${INSTALL_PREFIX}
         - binaries: ${INSTALL_BIN_DIR}
         - cosh extensions: ${INSTALL_EXTENSIONS_DIR}
         - docs (component-native): ${USER_DOC_DIR}
  6. Reports artifact locations at the end

$(echo -e "${BOLD}Note:${NC}")
  For agentsight eBPF probes, clang and libbpf headers must be installed via your
  system package manager. The script will detect and warn if they are missing.
EOF
    exit 0
}

# ─── argument parsing ───

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --no-install)
                DO_INSTALL=false
                shift
                ;;
            --ignore-deps)
                INSTALL_DEPS=false
                shift
                ;;
            --install-mode)
                [[ -n "${2:-}" ]] || die "--install-mode requires a value: user|system"
                case "$2" in
                    user|system) INSTALL_MODE="$2" ;;
                    *) die "Invalid --install-mode: $2. Valid: user, system" ;;
                esac
                shift 2
                ;;
            --usr|--system)
                INSTALL_MODE="system"
                shift
                ;;
            --dry-run)
                DRY_RUN=true
                shift
                ;;
            --interactive)
                INTERACTIVE=true
                shift
                ;;
            --non-interactive)
                NON_INTERACTIVE=true
                shift
                ;;
            --deps-only)
                DEPS_ONLY=true
                INSTALL_DEPS=true
                shift
                ;;
            --all)
                COMPONENTS=("${ALL_COMPONENTS[@]}")
                shift
                ;;
            --component)
                [[ -n "${2:-}" ]] || die "--component requires a value ($(join_by ", " "${ALL_COMPONENTS[@]}"))"
                if is_valid_component "$2"; then
                    COMPONENTS+=("$2")
                else
                    die "Unknown component: $2. Valid: $(join_by ", " "${ALL_COMPONENTS[@]}")"
                fi
                shift 2
                ;;
            --uninstall)
                DO_UNINSTALL=true
                shift
                ;;
            -h|--help)
                usage
                ;;
            *)
                die "Unknown option: $1. Use --help for usage."
                ;;
        esac
    done

    if $DEPS_ONLY; then
        INSTALL_DEPS=true
    fi
    if $INTERACTIVE && $NON_INTERACTIVE; then
        die "--interactive and --non-interactive cannot be used together"
    fi
}

# ─── main ───

main() {
    parse_args "$@"
    if $INTERACTIVE; then
        run_interactive_wizard
    else
        ensure_user_mode
    fi

    echo -e "${BOLD}ANOLISA Build Script${NC}"
    echo -e "${DIM}Project root: ${PROJECT_ROOT}${NC}"
    echo -e "${DIM}Mode: ${INSTALL_MODE}${NC}"

    if $DO_UNINSTALL; then
        do_uninstall
        echo ""
        ok "Done"
        exit 0
    fi

    if $INSTALL_DEPS; then
        do_install_deps
    fi

    if $DEPS_ONLY; then
        echo ""
        info "Deps-only mode, skipping build."
        exit 0
    fi

    do_build
    print_output_summary

    if $DO_INSTALL; then
        do_install
    fi

    echo ""
    ok "Done"
}

main "$@"
