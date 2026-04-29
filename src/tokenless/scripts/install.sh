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
#   ./install.sh --cosh              # Manually install copilot-shell hooks
#   ./install.sh --uninstall-openclaw # Uninstall OpenClaw plugin only
#   ./install.sh --uninstall-cosh    # Uninstall copilot-shell hooks only
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
# only OPENCLAW_DIR and COSH_DIR need fallback derivation.
OPENCLAW_DIR="${OPENCLAW_DIR:-${SHARE_DIR}/adapters/openclaw}"
COSH_DIR="${COSH_DIR:-${SHARE_DIR}/adapters/cosh}"

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
# Copilot-Shell Hooks Configuration (Shared)
# ============================================================================

# Configure copilot-shell hooks (idempotent)
configure_cosh_hooks() {
    local hook_source_dir="${1:-$COSH_DIR}"
    local settings_file=""

    # Detect settings file
    if [ -f "$HOME/.copilot-shell/settings.json" ]; then
        settings_file="$HOME/.copilot-shell/settings.json"
    elif [ -f "$HOME/.qwen-code/settings.json" ]; then
        settings_file="$HOME/.qwen-code/settings.json"
    fi

    if [ -z "$settings_file" ]; then
        warn "No copilot-shell settings file found"
        return 1
    fi

    info "Configuring copilot-shell hooks from: $hook_source_dir"

    # Copy hook scripts only if source differs from destination
    if [ "$hook_source_dir" != "$COSH_DIR" ] && [ -d "$hook_source_dir" ]; then
        mkdir -p "$COSH_DIR"
        cp "$hook_source_dir"/tokenless-*.sh "$COSH_DIR/" 2>/dev/null || true
        chmod +x "$COSH_DIR"/tokenless-*.sh 2>/dev/null || true
        info "  Copied hook scripts to $COSH_DIR"
    elif [ ! -d "$COSH_DIR" ]; then
        warn "Hook directory not found: $COSH_DIR"
    fi

    # Configure settings.json using jq
    if command -v jq &>/dev/null; then
        local temp_file
        temp_file=$(mktemp)

        # Remove existing tokenless hooks first, then add fresh ones (idempotent)
        jq '
            .hooks.PreToolUse = (.hooks.PreToolUse // [] | map(select(.hooks // [] | map(.command // "") | join("") | contains("tokenless") | not))) |
            .hooks.PostToolUse = (.hooks.PostToolUse // [] | map(select(.hooks // [] | map(.command // "") | join("") | contains("tokenless") | not))) |
            .hooks.BeforeModel = (.hooks.BeforeModel // [] | map(select(.hooks // [] | map(.command // "") | join("") | contains("tokenless") | not))) |
            .hooks = (.hooks // {}) |
            .hooks.PreToolUse = .hooks.PreToolUse + [
                {
                    "matcher": "Shell",
                    "hooks": [
                        {
                            "type": "command",
                            "command": "'"$COSH_DIR"'/tokenless-rewrite.sh",
                            "name": "tokenless-rewrite",
                            "timeout": 5000
                        }
                    ]
                }
            ] |
            .hooks.PostToolUse = .hooks.PostToolUse + [
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": "'"$COSH_DIR"'/tokenless-compress-response.sh",
                            "name": "tokenless-compress-response",
                            "timeout": 10000
                        }
                    ]
                }
            ] |
            .hooks.BeforeModel = .hooks.BeforeModel + [
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": "'"$COSH_DIR"'/tokenless-compress-schema.sh",
                            "name": "tokenless-compress-schema",
                            "timeout": 10000
                        }
                    ]
                }
            ]
        ' "$settings_file" > "$temp_file" 2>/dev/null

        if [ $? -eq 0 ] && [ -s "$temp_file" ]; then
            mv "$temp_file" "$settings_file"
            info "  Updated settings: $settings_file"
        else
            rm -f "$temp_file"
            warn "jq processing failed"
            return 1
        fi
    else
        warn "jq not available, skipping automatic configuration"
        return 1
    fi
}

# Clean up copilot-shell hooks
cleanup_cosh_hooks() {
    local is_upgrade="${1:-0}"

    if [ "$is_upgrade" -eq 1 ]; then
        info "Upgrade operation detected, preserving configuration"
        return 0
    fi

    info "Cleaning up copilot-shell hooks configuration..."

    for settings_file in "$HOME/.copilot-shell/settings.json" "$HOME/.qwen-code/settings.json"; do
        if [ ! -f "$settings_file" ]; then
            continue
        fi

        if ! grep -q "tokenless" "$settings_file" 2>/dev/null; then
            continue
        fi

        # Backup
        local backup_file="${settings_file}.tokenless_backup.$(date +%Y%m%d%H%M%S)"
        cp "$settings_file" "$backup_file"
        info "  Backed up: $backup_file"

        # Clean up using jq
        if command -v jq &>/dev/null; then
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
                info "  Cleaned up: $settings_file"
            else
                rm -f "$temp_file"
                warn "jq processing failed for $settings_file"
            fi
        else
            warn "jq not available, cannot clean up $settings_file"
        fi
    done

    # Remove hook scripts directory (only for local installation)
    if [ "$is_upgrade" -eq 0 ] && [ -d "$COSH_DIR" ] && [ "$SOURCE_TYPE" = "local" ]; then
        rm -rf "$COSH_DIR"
        info "  Removed hook scripts directory: $COSH_DIR"
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

    # Setup copilot-shell hooks (guarded internally)
    info "Installing copilot-shell hooks..."
    if [ -d "$PROJECT_DIR/hooks/copilot-shell" ]; then
        configure_cosh_hooks "$PROJECT_DIR/hooks/copilot-shell" || true
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

    # Clean up copilot-shell hooks
    cleanup_cosh_hooks 0

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
    echo "  Copilot-Shell Hooks:"
    echo "    ${COSH_DIR}/tokenless-rewrite.sh"
    echo "    ${COSH_DIR}/tokenless-compress-response.sh (includes TOON encoding)"
    echo "    ${COSH_DIR}/tokenless-compress-schema.sh"
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
    --cosh              Manually setup copilot-shell hooks only
    --uninstall-openclaw  Uninstall OpenClaw plugin only
    --uninstall-cosh    Uninstall copilot-shell hooks only
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
    COSH_DIR             copilot-shell hooks dir (auto-detected from installation root)

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
            COSH_DIR="${SHARE_DIR}/adapters/cosh"
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
            cleanup_cosh_hooks 0
            ;;
        --upgrade)
            info "Upgrade scenario — preserving existing configuration and stats."
            ;;
        --openclaw)
            setup_openclaw "$SOURCE_TYPE"
            ;;
        --cosh)
            if [ -f "$HOME/.copilot-shell/settings.json" ] || [ -f "$HOME/.qwen-code/settings.json" ]; then
                configure_cosh_hooks "${SHARE_DIR}/adapters/cosh"
            else
                warn "copilot-shell/qwen-code not installed, nothing to configure"
            fi
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
                    if [ -f "$HOME/.copilot-shell/settings.json" ] || [ -f "$HOME/.qwen-code/settings.json" ]; then
                        configure_cosh_hooks "${SHARE_DIR}/adapters/cosh" || true
                    else
                        info "copilot-shell/qwen-code not installed, skipping hooks configuration"
                    fi
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
