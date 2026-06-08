#!/usr/bin/env bash
# install.sh — lightweight installer for the anolisa CLI.
#
# Usage:
#   curl -fsSL https://anolisa.oss-cn-hangzhou.aliyuncs.com/install.sh | bash
#
# Environment overrides:
#   ANOLISA_VERSION      version to install      (default: stable)
#   ANOLISA_MIRROR       OSS mirror base URL     (default: https://anolisa.oss-cn-hangzhou.aliyuncs.com)
#   ANOLISA_INSTALL_DIR  binary install directory (default: ~/.local/bin)

set -euo pipefail

VERSION="${ANOLISA_VERSION:-stable}"
MIRROR="${ANOLISA_MIRROR:-https://anolisa.oss-cn-hangzhou.aliyuncs.com}"
INSTALL_DIR="${ANOLISA_INSTALL_DIR:-$HOME/.local/bin}"

log()  { printf '\033[1;32m%s\033[0m %s\n' "==>" "$*"; }
warn() { printf '\033[1;33m%s\033[0m %s\n' "warn:" "$*" >&2; }
err()  { printf '\033[1;31m%s\033[0m %s\n' "error:" "$*" >&2; exit 1; }

detect_platform() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Linux)  OS="linux" ;;
    Darwin) OS="darwin" ;;
    *)      err "unsupported OS: $os (only Linux and macOS are supported)" ;;
  esac

  case "$arch" in
    x86_64|amd64)   ARCH="x86_64";  ARCH_SHORT="x86_64" ;;
    aarch64|arm64)   ARCH="aarch64"; ARCH_SHORT="aarch64" ;;
    *)               err "unsupported architecture: $arch" ;;
  esac

  if [ "$OS" = "darwin" ] && [ "$ARCH" = "x86_64" ]; then
    err "macOS x86_64 is not supported; only Apple Silicon (arm64) is available"
  fi

  case "$OS" in
    linux)  TARGET="${ARCH}-unknown-linux-gnu" ;;
    darwin) TARGET="${ARCH}-apple-darwin" ;;
  esac
}

sha256_verify() {
  local file="$1" expected="$2"
  local actual
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$file" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  else
    warn "sha256sum/shasum not found, skipping checksum verification"
    return 0
  fi
  if [ "$actual" != "$expected" ]; then
    err "sha256 mismatch (expected: $expected, got: $actual)"
  fi
  log "checksum verified"
}

main() {
  detect_platform
  command -v curl >/dev/null 2>&1 || err "curl is required but not found"
  command -v tar  >/dev/null 2>&1 || err "tar is required but not found"

  local artifact release_dir label
  if [ "$VERSION" = "stable" ]; then
    artifact="anolisa-cli-${TARGET}.tar.gz"
    release_dir="stable"
    label="stable"
  else
    artifact="anolisa-cli-${VERSION}-${TARGET}.tar.gz"
    release_dir="$VERSION"
    label="$VERSION"
  fi

  local base_url="${MIRROR}/anolisa-releases/anolisa/v1/cli/releases/${release_dir}/artifacts/${OS}/${ARCH_SHORT}"
  local tar_url="${base_url}/${artifact}"
  local sha_url="${tar_url}.sha256.txt"

  log "installing anolisa ${label} (${TARGET})"

  TMPDIR_INSTALL="$(mktemp -d)"
  trap 'rm -rf "$TMPDIR_INSTALL"' EXIT

  log "downloading ${artifact}"
  if ! curl -fSL --progress-bar -o "${TMPDIR_INSTALL}/${artifact}" "$tar_url"; then
    err "download failed — check version/platform or set ANOLISA_MIRROR"
  fi

  log "verifying checksum"
  local expected_sha
  if expected_sha="$(curl -fsSL "$sha_url" 2>/dev/null | awk '{print $1}')"; then
    sha256_verify "${TMPDIR_INSTALL}/${artifact}" "$expected_sha"
  else
    warn "checksum file not available, skipping verification"
  fi

  log "extracting binary"
  tar -xzf "${TMPDIR_INSTALL}/${artifact}" -C "$TMPDIR_INSTALL"

  mkdir -p "$INSTALL_DIR"
  install -m 0755 "${TMPDIR_INSTALL}/anolisa" "${INSTALL_DIR}/anolisa"
  log "installed to ${INSTALL_DIR}/anolisa"

  case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
      warn "${INSTALL_DIR} is not in your PATH"
      echo "    add it with:  export PATH=\"${INSTALL_DIR}:\$PATH\""
      ;;
  esac

  if "${INSTALL_DIR}/anolisa" --version >/dev/null 2>&1; then
    log "$("${INSTALL_DIR}/anolisa" --version)"
  fi

  log "done"
}

main
