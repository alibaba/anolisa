#!/usr/bin/bash
set -euo pipefail

# Token-Less Unified Installation Script
# Supports: source install, RPM post-install, RPM pre-uninstall
#
# Usage:
#   ./install.sh                    # Auto-detect and configure
#   ./install.sh --source           # Force source build + installation
#   ./install.sh --install          # RPM post-install (verifies + configures if deps present)
#   ./install.sh --uninstall        # RPM pre-uninstall cleanup (full removal)
#   ./install.sh --upgrade          # RPM pre-uninstall cleanup (upgrade — no-op)
#   ./install.sh --openclaw         # Manually install OpenClaw plugin
#   ./install.sh --cosh              # Manually install copilot-shell extension
#   ./install.sh --uninstall-openclaw # Uninstall OpenClaw plugin only
#   ./install.sh --uninstall-cosh    # Uninstall copilot-shell extension only
#   ./install.sh --help             # Show help

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# ── Path auto-detection ──
# Derive all paths from where this script / tokenless binary is installed:
#   /usr/share/tokenless  (RPM)  → system paths (/usr/bin, /usr/libexec/tokenless, /usr/share/tokenless)
#   ~/.local/share/tokenless (make) → local paths  (~/.local/bin, ~/.local/share/tokenless)
# Environment variables (BIN_DIR, OPENCLAW_DIR, COSH_DIR) still override.

SHARE_DIR=""
BIN_DIR=""
LIBEXEC_DIR=""

detect_install_root() {
    # 1. Check where tokenless binary is installed
    local tokenless_path
    if tokenless_path="$(command -v tokenless 2>/dev/null)"; then
        case "$tokenless_path" in
            /usr/bin/tokenless)
                SHARE_DIR="/usr/share/tokenless"
                BIN_DIR="/usr/bin"
                LIBEXEC_DIR="/usr/libexec/tokenless"
                SOURCE_TYPE="system"
                return
                ;;
            */.local/bin/tokenless)
                SHARE_DIR="$HOME/.local/share/tokenless"
                BIN_DIR="$HOME/.local/bin"
                LIBEXEC_DIR="$HOME/.local/bin"
                SOURCE_TYPE="local"
                return
                ;;
        esac
    fi

    # 2. Check where this script itself resides
    case "$SCRIPT_DIR" in
        /usr/share/tokenless/scripts)
            SHARE_DIR="/usr/share/tokenless"
            BIN_DIR="/usr/bin"
            LIBEXEC_DIR="/usr/libexec/tokenless"
            SOURCE_TYPE="system"
            return
            ;;
        */.local/share/tokenless/scripts)
            SHARE_DIR="$HOME/.local/share/tokenless"
            BIN_DIR="$HOME/.local/bin"
            LIBEXEC_DIR="$HOME/.local/bin"
            SOURCE_TYPE="local"
            return
            ;;
    esac

    # 3. Default: local installation
    SHARE_DIR="$HOME/.local/share/tokenless"
    BIN_DIR="$HOME/.local/bin"
    LIBEXEC_DIR="$HOME/.local/bin"
    SOURCE_TYPE="local"
}

# Call directly (not in subshell) so global variables persist
detect_install_root

# Derived paths (overridable via environment variables)
# BIN_DIR and LIBEXEC_DIR are already set by detect_install_root;
# only OPENCLAW_DIR needs fallback derivation.
# COSH_EXTENSION_SRC is the system-installed source of the extension;
# for local installs we copy from the project's cosh-extension/ directory.
OPENCLAW_DIR="${OPENCLAW_DIR:-${SHARE_DIR}/adapters/openclaw}"
COSH_EXTENSION_SRC="${COSH_EXTENSION_SRC:-/usr/share/anolisa/extensions/tokenless}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info()  { echo -e "${GREEN}[INFO]${NC} $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*"; exit 1; }
step()  { echo -e "${BLUE}[STEP]${NC} $*"; }

# ============================================================================
# Installation Source Detection
# ============================================================================

get_openclaw_source() {
    local source_type="$1"
    case "$source_type" in
        system)
            echo "${SHARE_DIR}/adapters/openclaw"
            ;;
        local)
            echo "${PROJECT_DIR}/openclaw"
            ;;
        *)
            echo ""
            ;;
    esac
}

# ============================================================================
# OpenClaw Plugin Setup
# ============================================================================

setup_openclaw() {
    local source_type="$1"

    if ! command -v openclaw &>/dev/null; then
        info "OpenClaw not installed, skipping plugin configuration"
        return 0
    fi

    local openclaw_src
    openclaw_src="$(get_openclaw_source "$source_type")"

    if [ -z "$openclaw_src" ] || [ ! -d "$openclaw_src" ]; then
        warn "OpenClaw source directory not found: $openclaw_src"
        return 1
    fi

    info "Configuring OpenClaw plugin..."
    info "  Source: $openclaw_src"

    # Install plugin files to ~/.openclaw/extensions/tokenless/
    local ext_dir="$HOME/.openclaw/extensions/tokenless"
    mkdir -p "$ext_dir"

    cp "${openclaw_src}/index.ts" "$ext_dir/" 2>/dev/null || true
    cp "${openclaw_src}/openclaw.plugin.json" "$ext_dir/"
    cp "${openclaw_src}/package.json" "$ext_dir/"
    info "  Copied plugin files to $ext_dir"

    # Compile TypeScript to JavaScript
    if command -v npx &>/dev/null; then
        if npx --yes esbuild "${ext_dir}/index.ts" --bundle --platform=node --format=esm --outfile="${ext_dir}/index.js" 2>/dev/null; then
            info "  Compiled index.ts -> index.js (esbuild)"
        else
            sed 's/: any//g; s/: string//g; s/: boolean | null/: any/g; s/: Record<string, unknown>//g; s/: { [^}]*}//g' "${ext_dir}/index.ts" > "${ext_dir}/index.js"
            info "  Compiled index.ts -> index.js (sed fallback)"
        fi
    else
        sed 's/: any//g; s/: string//g; s/: boolean | null/: any/g; s/: Record<string, unknown>//g; s/: { [^}]*}//g' "${ext_dir}/index.ts" > "${ext_dir}/index.js"
        info "  Compiled index.ts -> index.js (sed fallback)"
    fi

    # Register plugin in openclaw.json
    local openclaw_config="$HOME/.openclaw/openclaw.json"
    if [ -f "$openclaw_config" ] && command -v jq &>/dev/null; then
        local temp_file
        temp_file=$(mktemp)
        jq '
            .plugins.enabled = true |
            .plugins.entries["tokenless-openclaw"] = {"enabled": true} |
            .plugins.allow = (.plugins.allow // [] | map(select(. != "tokenless-openclaw")) + ["tokenless-openclaw"])
        ' "$openclaw_config" > "$temp_file" 2>/dev/null
        if [ -s "$temp_file" ]; then
            mv "$temp_file" "$openclaw_config"
            info "  Registered tokenless-openclaw in $openclaw_config"
        else
            rm -f "$temp_file"
            warn "  Failed to update openclaw.json"
        fi
    else
        warn "  jq not found — manually add tokenless-openclaw to $openclaw_config"
    fi
}

cleanup_openclaw() {
    local is_upgrade="${1:-0}"

    if [ "$is_upgrade" -eq 1 ]; then
        info "Upgrade detected, preserving OpenClaw plugin"
        return 0
    fi

    info "Cleaning up OpenClaw plugin..."

    # Remove extension directory
    local ext_dir="$HOME/.openclaw/extensions/tokenless"
    if [ -d "$ext_dir" ]; then
        rm -rf "$ext_dir"
        info "  Removed $ext_dir"
    fi

    # Unregister from openclaw.json
    local openclaw_config="$HOME/.openclaw/openclaw.json"
    if [ -f "$openclaw_config" ] && command -v jq &>/dev/null; then
        local temp_file
        temp_file=$(mktemp)
        jq '
            del(.plugins.entries["tokenless-openclaw"]) |
            .plugins.allow = (.plugins.allow // [] | map(select(. != "tokenless-openclaw")))
        ' "$openclaw_config" > "$temp_file" 2>/dev/null
        if [ -s "$temp_file" ]; then
            mv "$temp_file" "$openclaw_config"
            info "  Unregistered tokenless-openclaw from $openclaw_config"
        else
            rm -f "$temp_file"
            warn "  Failed to update openclaw.json"
        fi
    fi
}

# ============================================================================
# Copilot-Shell Extension Setup
# ============================================================================

# Detect copilot-shell extensions directory
_detect_cosh_extensions_dir() {
    # copilot-shell and qwen-code store extensions in different locations.
    # Return the first existing one, or the default copilot-shell path.
    for dir in "$HOME/.copilot-shell/extensions" "$HOME/.qwen-code/extensions"; do
        if [ -d "$dir" ]; then
            echo "$dir"
            return
        fi
    done
    # Default: use copilot-shell path (will be created if needed)
    echo "$HOME/.copilot-shell/extensions"
}

# Install cosh extension (idempotent)
install_cosh_extension() {
    local extension_src="${1:-$COSH_EXTENSION_SRC}"

    if [ ! -d "$extension_src" ]; then
        warn "Extension source directory not found: $extension_src"
        return 1
    fi

    local extensions_dir
    extensions_dir="$(_detect_cosh_extensions_dir)"
    local target_dir="${extensions_dir}/tokenless"

    info "Installing copilot-shell extension..."
    info "  Source: $extension_src"
    info "  Target: $target_dir"

    # Copy entire extension directory
    rm -rf "$target_dir" 2>/dev/null || true
    mkdir -p "$extensions_dir"
    cp -r "$extension_src" "$target_dir"
    chmod +x "$target_dir/hooks/"*.py 2>/dev/null || true
    info "  Extension installed to $target_dir"

    # Also clean up legacy bash hooks from settings.json (migration)
    _cleanup_legacy_cosh_hooks
}

# Uninstall cosh extension
uninstall_cosh_extension() {
    local is_upgrade="${1:-0}"

    if [ "$is_upgrade" -eq 1 ]; then
        info "Upgrade operation detected, preserving extension"
        return 0
    fi

    info "Uninstalling copilot-shell extension..."

    # Remove extension directories from all known locations
    for extensions_dir in "$HOME/.copilot-shell/extensions" "$HOME/.qwen-code/extensions"; do
        local target_dir="${extensions_dir}/tokenless"
        if [ -d "$target_dir" ]; then
            rm -rf "$target_dir"
            info "  Removed $target_dir"
        fi
    done

    # Also clean up legacy bash hooks from settings.json
    _cleanup_legacy_cosh_hooks
}

# Clean up legacy bash hook entries from settings.json (migration helper)
_cleanup_legacy_cosh_hooks() {
    for settings_file in "$HOME/.copilot-shell/settings.json" "$HOME/.qwen-code/settings.json"; do
        if [ ! -f "$settings_file" ]; then
            continue
        fi

        if ! grep -q "tokenless" "$settings_file" 2>/dev/null; then
            continue
        fi

        if ! command -v jq &>/dev/null; then
            warn "jq not available, cannot clean up legacy hook entries in $settings_file"
            continue
        fi

        local temp_file
        temp_file=$(mktemp)

        jq '
            .hooks.PreToolUse = (.hooks.PreToolUse // [] | map(select(.hooks // [] | map(.command // "") | join("") | contains("tokenless") | not))) |
            .hooks.PostToolUse = (.hooks.PostToolUse // [] | map(select(.hooks // [] | map(.command // "") | join("") | contains("tokenless") | not))) |
            .hooks.BeforeModel = (.hooks.BeforeModel // [] | map(select(.hooks // [] | map(.command // "") | join("") | contains("tokenless") | not))) |
            if .hooks.PreToolUse == [] then del(.hooks.PreToolUse) else . end |
            if .hooks.PostToolUse == [] then del(.hooks.PostToolUse) else . end |
            if .hooks.BeforeModel == [] then del(.hooks.BeforeModel) else . end |
            if (.hooks | length) == 0 then del(.hooks) else . end
        ' "$settings_file" > "$temp_file" 2>/dev/null

        if [ $? -eq 0 ] && [ -s "$temp_file" ]; then
            mv "$temp_file" "$settings_file"
            info "  Cleaned up legacy hook entries in $settings_file"
        else
            rm -f "$temp_file"
            warn "  Failed to clean up $settings_file"
        fi
    done

    # Remove legacy hook scripts directory
    local legacy_cosh_dir="${SHARE_DIR}/adapters/cosh"
    if [ -d "$legacy_cosh_dir" ]; then
        rm -rf "$legacy_cosh_dir"
        info "  Removed legacy hook scripts directory: $legacy_cosh_dir"
    fi
}

# ============================================================================
# Source Installation
# ============================================================================

install_from_source() {
    step "Building from source..."

    # Check prerequisites
    info "Checking prerequisites..."

    if ! command -v cargo &>/dev/null; then
        error "Rust toolchain not found. Install from https://rustup.ru"
    fi
    info "  Rust: $(rustc --version)"

    if ! command -v git &>/dev/null; then
        error "Git not found."
    fi

    # Initialize submodules
    info "Initializing git submodules..."
    cd "$PROJECT_DIR"
    git submodule update --init --recursive

    # Build
    info "Building tokenless..."
    cargo build --release

    info "Building rtk..."
    cargo build --release --manifest-path third_party/rtk/Cargo.toml

    info "Building toon..."
    cargo build --release --manifest-path third_party/toon/Cargo.toml --features cli

    # Install binaries
    info "Installing tokenless to $BIN_DIR..."
    mkdir -p "$BIN_DIR"
    cp target/release/tokenless "$BIN_DIR/"
    chmod +x "$BIN_DIR/tokenless"

    info "Installing rtk and toon helpers to $LIBEXEC_DIR..."
    mkdir -p "$LIBEXEC_DIR"
    cp third_party/rtk/target/release/rtk "$LIBEXEC_DIR/"
    cp third_party/toon/target/release/toon "$LIBEXEC_DIR/"
    chmod +x "$LIBEXEC_DIR/rtk" "$LIBEXEC_DIR/toon"

    # Setup OpenClaw (guarded internally)
    setup_openclaw "local" || true

    # Setup copilot-shell extension (guarded internally)
    info "Installing copilot-shell extension..."
    if [ -d "$PROJECT_DIR/cosh-extension" ]; then
        install_cosh_extension "$PROJECT_DIR/cosh-extension" || true
    fi
}

# ============================================================================
# RPM Post-Install Configuration
# ============================================================================

rpm_postinstall() {
    :
}

# ============================================================================
# RPM Pre-Uninstall Cleanup
# ============================================================================

rpm_preuninstall() {
    info "=========================================="
    info "Token-Less Pre-Uninstallation Cleanup"
    info "=========================================="

    # Clean up OpenClaw plugin
    cleanup_openclaw 0

    # Clean up copilot-shell extension
    uninstall_cosh_extension 0

    # Clean up stats data
    if [ -d "$HOME/.tokenless" ]; then
        rm -rf "$HOME/.tokenless"
        info "  Removed stats data: $HOME/.tokenless"
    fi

    info "=========================================="
    info "Cleanup completed"
    info "=========================================="
}

# ============================================================================
# Verification
# ============================================================================

verify_installation() {
    info "Verifying installation..."

    local verify_ok=true
    local tokenless_path
    local rtk_path
    local toon_path

    tokenless_path="${BIN_DIR}/tokenless"
    rtk_path="${LIBEXEC_DIR}/rtk"
    toon_path="${LIBEXEC_DIR}/toon"

    if "$tokenless_path" --version &>/dev/null; then
        info "  tokenless: $($tokenless_path --version)"
    else
        warn "  tokenless: verification failed"
        verify_ok=false
    fi

    if "$rtk_path" --version &>/dev/null; then
        info "  rtk: $($rtk_path --version)"
    else
        warn "  rtk: verification failed"
        verify_ok=false
    fi

    if "$toon_path" --version &>/dev/null; then
        info "  toon: $($toon_path --version)"
    else
        warn "  toon: verification failed"
        verify_ok=false
    fi

    # PATH check (only for local installation)
    if [ "$SOURCE_TYPE" = "local" ]; then
        if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
            warn "$BIN_DIR is not in your PATH. Add it:"
            warn "  echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.bashrc"
        fi
    fi

    echo ""
    echo "============================================"
    echo "  Token-Less Installation Complete!"
    echo "============================================"
    echo ""
    if [ "$SOURCE_TYPE" = "system" ]; then
        echo "  Installation Mode: System-wide (RPM)"
    else
        echo "  Installation Mode: Local (Source)"
    fi
    echo ""
    echo "  Binaries:"
    echo "    tokenless -> ${BIN_DIR}/tokenless"
    echo "    rtk       -> ${LIBEXEC_DIR}/rtk"
    echo "    toon      -> ${LIBEXEC_DIR}/toon"
    echo ""
    echo "  OpenClaw Plugin:"
    echo "    ${OPENCLAW_DIR}/"
    echo ""
    local extensions_dir
    extensions_dir="$(_detect_cosh_extensions_dir)"

    echo "  Copilot-Shell Extension:"
    echo "    ${extensions_dir}/tokenless/"
    echo ""
    if [ "$verify_ok" = true ]; then
        echo "  Status: All checks passed"
    else
        echo "  Status: Some checks failed (see warnings above)"
    fi
    echo ""
}

# ============================================================================
# Help and Usage
# ============================================================================

show_help() {
    cat << EOF
Token-Less Unified Installation Script

USAGE:
    $(basename "$0") [OPTIONS]

OPTIONS:
    (no argument)       Auto-detect installation source and install
    --source            Force source installation (build + install + plugins)
    --install           RPM post-installation configuration (%post scriptlet)
    --uninstall         RPM pre-uninstallation cleanup (full removal)
    --upgrade           RPM pre-uninstallation cleanup (upgrade scenario)
    --openclaw          Manually setup OpenClaw plugin only
    --cosh              Manually install copilot-shell extension
    --uninstall-openclaw  Uninstall OpenClaw plugin only
    --uninstall-cosh    Uninstall copilot-shell extension only
    --help, -h          Show this help message

EXAMPLES:
    # Auto-detect and install
    ./install.sh

    # Force source installation
    ./install.sh --source

    # RPM package installation (called by yum/rpm)
    ./install.sh --install

    # RPM package uninstallation (called by yum/rpm)
    ./install.sh --uninstall
    ./install.sh --upgrade

ENVIRONMENT VARIABLES:
    BIN_DIR              tokenless binary dir (auto-detected: /usr/bin for RPM, ~/.local/bin for local)
    LIBEXEC_DIR          helper binary dir (auto-detected: /usr/libexec/tokenless for RPM, ~/.local/bin for local)
    OPENCLAW_DIR         OpenClaw plugin dir (auto-detected from installation root)
    COSH_EXTENSION_SRC   copilot-shell extension source dir (auto-detected from installation root)

EOF
}

# ============================================================================
# Main Entry Point
# ============================================================================

main() {
    local mode="${1:-}"

    case "$mode" in
        --source)
            # Force local installation paths for source build
            SOURCE_TYPE="local"
            SHARE_DIR="$HOME/.local/share/tokenless"
            BIN_DIR="$HOME/.local/bin"
            LIBEXEC_DIR="$HOME/.local/bin"
            OPENCLAW_DIR="${SHARE_DIR}/adapters/openclaw"
            install_from_source
            verify_installation
            ;;
        --install)
            rpm_postinstall
            ;;
        --uninstall)
            rpm_preuninstall
            ;;
        --uninstall-openclaw)
            cleanup_openclaw 0
            ;;
        --uninstall-cosh)
            uninstall_cosh_extension 0
            ;;
        --upgrade)
            info "Upgrade scenario — preserving existing configuration and stats."
            ;;
        --openclaw)
            setup_openclaw "$SOURCE_TYPE"
            ;;
        --cosh)
            install_cosh_extension "${COSH_EXTENSION_SRC}"
            ;;
        --help|-h)
            show_help
            exit 0
            ;;
        "")
            case "$SOURCE_TYPE" in
                system)
                    info "Detected system-wide installation."
                    if command -v openclaw &>/dev/null; then
                        setup_openclaw "system" || true
                    else
                        info "OpenClaw not installed, skipping plugin configuration"
                    fi
                    install_cosh_extension "${COSH_EXTENSION_SRC}" || true
                    verify_installation
                    ;;
                local)
                    install_from_source
                    verify_installation
                    ;;
                *)
                    error "Cannot determine installation source."
                    ;;
            esac
            ;;
        *)
            error "Unknown option: $mode"
            echo ""
            show_help
            exit 1
            ;;
    esac
}

# Run main function
main "$@"
