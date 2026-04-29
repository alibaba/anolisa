#!/bin/bash
# =============================================================================
# Unified RPM build script for ANOLISA
# Usage:
#   ./scripts/rpm-build.sh <package>        Build a single package
#   ./scripts/rpm-build.sh all              Build all packages
#
# Packages: copilot-shell, agent-sec-core, os-skills, agentsight, tokenless
#
# Environment variables:
#   VERSION    Override version for .spec.in templates (default: auto-detect)
#   RPMBUILD   Path to rpmbuild binary (default: rpmbuild)
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
BUILD_DIR="${SCRIPT_DIR}/rpmbuild"
RPMBUILD="${RPMBUILD:-rpmbuild}"

# Source directories
SHELL_DIR="${ROOT_DIR}/src/copilot-shell"
SEC_DIR="${ROOT_DIR}/src/agent-sec-core"
SKILLS_DIR="${ROOT_DIR}/src/os-skills"
SIGHT_DIR="${ROOT_DIR}/src/agentsight"
TOKEN_DIR="${ROOT_DIR}/src/tokenless"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

log()  { echo -e "${CYAN}[INFO]${NC} $*" >&2; }
warn() { echo -e "${YELLOW}[WARN]${NC} $*" >&2; }
err()  { echo -e "${RED}[ERROR]${NC} $*" >&2; }
ok()   { echo -e "${GREEN}[OK]${NC} $*" >&2; }

# -----------------------------------------------------------------------------
# Install a package using the available package manager
# -----------------------------------------------------------------------------
install_package() {
    local pkg="$1"
    if command -v dnf &>/dev/null; then
        dnf install -y "$pkg"
    elif command -v yum &>/dev/null; then
        yum install -y "$pkg"
    else
        err "No supported package manager found (dnf/yum)"
        return 1
    fi
}

# -----------------------------------------------------------------------------
# Setup rpmbuild directory tree under scripts/rpmbuild/
# -----------------------------------------------------------------------------
setup_rpmbuild() {
    log "Setting up rpmbuild tree at ${BUILD_DIR}"
    mkdir -p "${BUILD_DIR}"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
}

# -----------------------------------------------------------------------------
# Parse version from a spec or spec.in file
# -----------------------------------------------------------------------------
parse_spec_version() {
    local spec_file="$1"
    grep -E '^Version:' "$spec_file" | awk '{print $2}' | tr -d ' \t'
}

# -----------------------------------------------------------------------------
# Parse name from a spec or spec.in file
# -----------------------------------------------------------------------------
parse_spec_name() {
    local spec_file="$1"
    grep -E '^Name:' "$spec_file" | awk '{print $2}' | tr -d ' \t'
}

# -----------------------------------------------------------------------------
# Process .spec.in template -> .spec (replace @VERSION@)
# Returns the path of the generated .spec file
# -----------------------------------------------------------------------------
process_spec_template() {
    local spec_in="$1"
    local version="$2"
    local spec_out="${BUILD_DIR}/SPECS/$(basename "${spec_in%.in}")"

    log "Processing template: $(basename "$spec_in") -> $(basename "$spec_out") (version=${version})"
    sed "s/@VERSION@/${version}/g" "$spec_in" > "$spec_out"
    echo "$spec_out"
}

# =============================================================================
# copilot-shell
# =============================================================================
build_copilot_shell() {
    log "=========================================="
    log "Building RPM: copilot-shell"
    log "=========================================="

    local spec_in="${SHELL_DIR}/copilot-shell.spec.in"
    if [ ! -f "$spec_in" ]; then
        err "Spec template not found: $spec_in"
        return 1
    fi

    # Version from package.json or env
    local version="${VERSION:-}"
    if [ -z "$version" ]; then
        version=$(node -p "require('${SHELL_DIR}/package.json').version" 2>/dev/null || true)
    fi
    if [ -z "$version" ]; then
        err "Cannot determine copilot-shell version. Set VERSION env or ensure package.json exists."
        return 1
    fi

    local pkg_name
    pkg_name=$(parse_spec_name "$spec_in")
    local tarball_name="${pkg_name}-${version}.tar.gz"

    # Step 1: Process spec template
    local spec_file
    spec_file=$(process_spec_template "$spec_in" "$version")

    # Step 2: Build bundle (npm install + bundle + prepare:package)
    log "Step 1/3: Building copilot-shell bundle..."
    (
        cd "$SHELL_DIR"
        npm install --ignore-scripts
        npm run bundle
        npm run prepare:package
    )

    # Step 3: Create source tarball
    log "Step 2/3: Creating source tarball ${tarball_name}..."
    local tmp_dir
    tmp_dir=$(mktemp -d)
    local pkg_dir="${tmp_dir}/${pkg_name}-${version}"
    mkdir -p "$pkg_dir"

    # Copy the necessary files (same as spec %install expects)
    cp -rp "${SHELL_DIR}/dist"/* "$pkg_dir/"
    [ -f "${SHELL_DIR}/LICENSE" ] && cp "${SHELL_DIR}/LICENSE" "$pkg_dir/"
    [ -f "${SHELL_DIR}/README.md" ] && cp "${SHELL_DIR}/README.md" "$pkg_dir/"
    # Also include full source for rpmbuild %build section
    local excludes=(
        --exclude='.git'
        --exclude='node_modules'
        --exclude='dist'
        --exclude='coverage'
        --exclude='.DS_Store'
    )
    rm -rf "$pkg_dir"
    mkdir -p "$pkg_dir"
    tar -cf - -C "$SHELL_DIR" "${excludes[@]}" . | tar -xf - -C "$pkg_dir"

    tar -czf "${BUILD_DIR}/SOURCES/${tarball_name}" -C "$tmp_dir" "${pkg_name}-${version}"
    rm -rf "$tmp_dir"

    # Step 4: rpmbuild (--nodeps: BuildRequires are handled by yum-builddep in CI)
    log "Step 3/3: Running rpmbuild..."
    "$RPMBUILD" -ba --nodeps \
        --define "_topdir ${BUILD_DIR}" \
        "$spec_file"

    ok "copilot-shell RPM built successfully"
}

# =============================================================================
# agent-sec-core
# =============================================================================
build_agent_sec_core() {
    log "=========================================="
    log "Building RPM: agent-sec-core"
    log "=========================================="

    local spec_in="${SEC_DIR}/agent-sec-core.spec.in"
    if [ ! -f "$spec_in" ]; then
        err "Spec template not found: $spec_in"
        return 1
    fi

    # Version: prefer $VERSION env (set by nightly CI), fallback to pyproject.toml
    local version="${VERSION:-}"
    if [ -z "$version" ]; then
        version=$(grep -m1 '^version' "${SEC_DIR}/agent-sec-cli/pyproject.toml" | sed 's/.*"\(.*\)"/\1/')
    fi
    if [ -z "$version" ]; then
        err "Cannot determine agent-sec-core version. Set VERSION env or ensure pyproject.toml exists."
        return 1
    fi

    local pkg_name
    pkg_name=$(parse_spec_name "$spec_in")
    local tarball_name="${pkg_name}-${version}.tar.gz"

    # Step 1: Process spec template (@VERSION@ -> actual version)
    log "Step 1/3: Preparing spec file..."
    local spec_file
    spec_file=$(process_spec_template "$spec_in" "$version")

    # Step 2: Create source tarball
    # Note: rust-toolchain.toml is intentionally excluded from the tarball.
    # The source file requires Rust 1.93.0, but rpmbuild environments may only
    # have an older Rust available (BuildRequires: rust >= 1.70). By omitting
    # rust-toolchain.toml, cargo falls back to whatever system Rust is present.
    log "Step 2/3: Creating source tarball ${tarball_name}..."
    local tmp_dir
    tmp_dir=$(mktemp -d)
    local pkg_dir="${tmp_dir}/${pkg_name}-${version}"
    mkdir -p "$pkg_dir"/{skills,linux-sandbox,agent-sec-cli,cosh-extension,openclaw-plugin,scripts}

    # skills: use cp -rp dir/. to include hidden files/directories
    cp -rp "${SEC_DIR}/skills/." "$pkg_dir/skills/"
    cp -rp "${SEC_DIR}/linux-sandbox/"* "$pkg_dir/linux-sandbox/"
    rm -f "$pkg_dir/linux-sandbox/rust-toolchain.toml"
    cp -rp "${SEC_DIR}/cosh-extension/"* "$pkg_dir/cosh-extension/"
    cp -p "${SEC_DIR}/scripts/agent-sec-cli-wrapper.sh" "$pkg_dir/scripts/"
    cp "${SEC_DIR}/Makefile" "$pkg_dir/"
    [ -f "${SEC_DIR}/LICENSE" ] && cp "${SEC_DIR}/LICENSE" "$pkg_dir/"
    [ -f "${SEC_DIR}/README.md" ] && cp "${SEC_DIR}/README.md" "$pkg_dir/"

    # openclaw-plugin (exclude node_modules and dev artifacts)
    tar -cf - -C "${SEC_DIR}" \
        --exclude='node_modules' \
        --exclude='.tsbuildinfo' \
        openclaw-plugin/ | tar -xf - -C "$pkg_dir/"

    # Include agent-sec-cli source for maturin wheel build
    # Exclude development artifacts (.venv, target, __pycache__, .egg-info, dist)
    tar -cf - -C "${SEC_DIR}" \
        --exclude='.venv' \
        --exclude='target' \
        --exclude='__pycache__' \
        --exclude='*.egg-info' \
        --exclude='dist' \
        --exclude='.pytest_cache' \
        agent-sec-cli/ | tar -xf - -C "$pkg_dir/"

    tar -czf "${BUILD_DIR}/SOURCES/${tarball_name}" -C "$tmp_dir" "${pkg_name}-${version}"
    rm -rf "$tmp_dir"

    # Step 3: rpmbuild (--nodeps: BuildRequires are handled by yum-builddep in CI)
    log "Step 3/3: Running rpmbuild..."
    "$RPMBUILD" -ba --nodeps \
        --define "_topdir ${BUILD_DIR}" \
        "$spec_file"

    ok "agent-sec-core RPM built successfully"
}

# =============================================================================
# os-skills
# =============================================================================
build_agentic_os_skills() {
    log "=========================================="
    log "Building RPM: os-skills"
    log "=========================================="

    local spec_in="${SKILLS_DIR}/os-skills.spec.in"
    if [ ! -f "$spec_in" ]; then
        err "Spec template not found: $spec_in"
        return 1
    fi

    # Version from env or default
    local version="${VERSION:-}"
    if [ -z "$version" ]; then
        # Try to read from spec changelog (first entry)
        version=$(grep -m1 -oE '[0-9]+\.[0-9]+\.[0-9]+' "$spec_in" | head -1)
    fi
    if [ -z "$version" ]; then
        version="0.0.1"
        warn "No version specified for os-skills, using default: ${version}"
    fi

    local pkg_name
    pkg_name=$(parse_spec_name "$spec_in")
    local tarball_name="${pkg_name}-${version}.tar.gz"

    # Step 1: Process spec template
    local spec_file
    spec_file=$(process_spec_template "$spec_in" "$version")

    # Step 2: Create source tarball
    log "Step 1/2: Creating source tarball ${tarball_name}..."
    local tmp_dir
    tmp_dir=$(mktemp -d)
    local pkg_dir="${tmp_dir}/${pkg_name}-${version}"
    mkdir -p "$pkg_dir"

    # Copy skill directories
    for dir in ai aliyun devops monitor-perf others security system-admin; do
        [ -d "${SKILLS_DIR}/${dir}" ] && cp -rp "${SKILLS_DIR}/${dir}" "$pkg_dir/"
    done
    
    if [ -f "${SKILLS_DIR}/LICENSE" ]; then
        cp -L "${SKILLS_DIR}/LICENSE" "$pkg_dir/"
    elif [ -f "${ROOT_DIR}/LICENSE" ]; then
        cp "${ROOT_DIR}/LICENSE" "$pkg_dir/"
    fi

    tar -czf "${BUILD_DIR}/SOURCES/${tarball_name}" -C "$tmp_dir" "${pkg_name}-${version}"
    rm -rf "$tmp_dir"

    # Step 3: rpmbuild (--nodeps: BuildRequires are handled by yum-builddep in CI)
    log "Step 2/2: Running rpmbuild..."
    "$RPMBUILD" -ba --nodeps \
        --define "_topdir ${BUILD_DIR}" \
        "$spec_file"

    ok "os-skills RPM built successfully"
}

# =============================================================================
# agentsight
# =============================================================================
build_agentsight() {
    log "=========================================="
    log "Building RPM: agentsight"
    log "=========================================="

    local spec_in="${SIGHT_DIR}/agentsight.spec.in"
    if [ ! -f "$spec_in" ]; then
        err "Spec template not found: $spec_in"
        return 1
    fi

    # Version from env or Cargo.toml
    local version="${VERSION:-}"
    if [ -z "$version" ]; then
        version=$(grep -m1 '^version' "${SIGHT_DIR}/Cargo.toml" | sed 's/version = "\(.*\)"/\1/' 2>/dev/null || true)
    fi
    if [ -z "$version" ]; then
        version=$(grep -m1 -oE '[0-9]+\.[0-9]+\.[0-9]+' "$spec_in" | head -1)
    fi
    if [ -z "$version" ]; then
        version="0.0.1"
        warn "No version specified for agentsight, using default: ${version}"
    fi

    local pkg_name
    pkg_name=$(parse_spec_name "$spec_in")
    local tarball_name="${pkg_name}-${version}.tar.gz"

    log "Step 1/3: Building agentsight..."
    if ! command -v clang &>/dev/null; then
        log "clang not found, installing..."
        install_package clang || { err "Failed to install clang"; return 1; }
    fi
    (
        cd "$SIGHT_DIR"
        cargo build --release
    )

    # Step 2: Process spec template and create tarball
    log "Step 2/3: Preparing spec and source tarball..."
    local spec_file
    spec_file=$(process_spec_template "$spec_in" "$version")

    local tmp_dir
    tmp_dir=$(mktemp -d)
    local pkg_dir="${tmp_dir}/${pkg_name}-${version}"
    mkdir -p "$pkg_dir"

    # Copy relevant files
    cp -rp "${SIGHT_DIR}/target/release/agentsight" "$pkg_dir/" 2>/dev/null || warn "Binary missing"
    [ -f "${SIGHT_DIR}/README.md" ] && cp "${SIGHT_DIR}/README.md" "$pkg_dir/"
    [ -f "${SIGHT_DIR}/README_CN.md" ] && cp "${SIGHT_DIR}/README_CN.md" "$pkg_dir/"
    [ -f "${SIGHT_DIR}/LICENSE" ] && cp "${SIGHT_DIR}/LICENSE" "$pkg_dir/"

    tar -czf "${BUILD_DIR}/SOURCES/${tarball_name}" -C "$tmp_dir" "${pkg_name}-${version}"
    rm -rf "$tmp_dir"

    log "Step 3/3: Running rpmbuild..."
    "$RPMBUILD" -ba --nodeps \
        --define "_topdir ${BUILD_DIR}" \
        "$spec_file"

    ok "agentsight RPM built successfully"
}

# =============================================================================
# tokenless
# =============================================================================
build_tokenless() {
    log "=========================================="
    log "Building RPM: tokenless"
    log "=========================================="

    local spec_in="${TOKEN_DIR}/tokenless.spec.in"
    if [ ! -f "$spec_in" ]; then
        err "Spec template not found: $spec_in"
        return 1
    fi

    # Version from env or Cargo.toml workspace
    local version="${VERSION:-}"
    if [ -z "$version" ]; then
        version=$(grep -m1 '^version' "${TOKEN_DIR}/Cargo.toml" | sed 's/version = "\(.*\)"/\1/' 2>/dev/null || true)
    fi
    if [ -z "$version" ]; then
        version=$(grep -m1 -oE '[0-9]+\.[0-9]+\.[0-9]+' "$spec_in" | head -1)
    fi
    if [ -z "$version" ]; then
        version="0.0.1"
        warn "No version specified for tokenless, using default: ${version}"
    fi

    local pkg_name
    pkg_name=$(parse_spec_name "$spec_in")
    local tarball_name="${pkg_name}-${version}.tar.gz"

    # Step 1: Process spec template
    local spec_file
    spec_file=$(process_spec_template "$spec_in" "$version")

    log "Step 1/3: Initializing submodules..."
    (
        cd "$TOKEN_DIR"
        if [ ! -d "third_party/rtk/.git" ]; then
            log "Initializing git submodules..."
            git submodule update --init
        fi
        # Build tokenless
        cargo build --release --workspace
        # Build rtk from submodule
        cargo build --release --manifest-path third_party/rtk/Cargo.toml
        # Build toon from submodule (fallback to pre-built binary if Rust < 1.88)
        cargo build --release --manifest-path third_party/toon/Cargo.toml --features cli || true
    )

    log "Step 2/3: Creating source tarball ${tarball_name}..."
    local tmp_dir
    tmp_dir=$(mktemp -d)
    local pkg_dir="${tmp_dir}/${pkg_name}"
    mkdir -p "$pkg_dir"

    # Copy full source tree, excluding build artifacts and VCS
    tar -cf - -C "$TOKEN_DIR" \
        --exclude='target' \
        --exclude='.git' \
        --exclude='.gitmodules' \
        --exclude='node_modules' \
        . | tar -xf - -C "$pkg_dir"

    tar -czf "${BUILD_DIR}/SOURCES/${tarball_name}" -C "$tmp_dir" "${pkg_name}"
    rm -rf "$tmp_dir"

    log "Step 3/3: Running rpmbuild..."
    "$RPMBUILD" -ba --nodeps \
        --define "_topdir ${BUILD_DIR}" \
        "$spec_file"

    ok "tokenless RPM built successfully"
}

# =============================================================================
# Main
# =============================================================================
usage() {
    echo "Usage: $0 <package|all>"
    echo ""
    echo "Packages:"
    echo "  copilot-shell       Build copilot-shell RPM"
    echo "  agent-sec-core      Build agent-sec-core RPM"
    echo "  os-skills           Build os-skills RPM"
    echo "  agentsight          Build agentsight RPM"
    echo "  tokenless           Build tokenless RPM"
    echo "  all                 Build all RPM packages"
    echo ""
    echo "Environment variables:"
    echo "  VERSION             Override version for .spec.in templates"
    echo "  RPMBUILD            Path to rpmbuild binary (default: rpmbuild)"
    echo ""
    echo "Output: scripts/rpmbuild/RPMS/"
}

if [ $# -lt 1 ]; then
    usage
    exit 1
fi

TARGET="$1"

# Pre-flight: check rpmbuild is available
if ! command -v "$RPMBUILD" &>/dev/null; then
    err "rpmbuild not found. Install with: yum install rpm-build (or brew install rpm on macOS)"
    exit 1
fi

setup_rpmbuild

case "$TARGET" in
    copilot-shell)
        build_copilot_shell
        ;;
    agent-sec-core)
        build_agent_sec_core
        ;;
    os-skills)
        build_agentic_os_skills
        ;;
    agentsight)
        build_agentsight
        ;;
    tokenless)
        build_tokenless
        ;;
    all)
        build_copilot_shell
        build_agent_sec_core
        build_agentic_os_skills
        build_agentsight
        build_tokenless
        ;;
    *)
        err "Unknown package: $TARGET"
        usage
        exit 1
        ;;
esac

# Print results
echo ""
log "=========================================="
log "RPM build output:"
log "=========================================="
find "${BUILD_DIR}/RPMS" "${BUILD_DIR}/SRPMS" -name "*.rpm" -type f 2>/dev/null | while read -r rpm; do
    echo "  $(basename "$rpm")  ($(du -h "$rpm" | cut -f1))"
done
echo ""
log "Output directory: ${BUILD_DIR}/RPMS/"
