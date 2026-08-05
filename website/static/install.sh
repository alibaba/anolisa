#!/usr/bin/env bash
# install.sh — lightweight installer for the anolisa CLI.
#
# Usage:
#   curl -fsSL https://get.agentic-os.sh | bash
#
# Environment overrides:
#   ANOLISA_VERSION      version to install      (default: stable)
#   ANOLISA_MIRROR       OSS mirror base URL     (default: https://anolisa.oss-cn-hangzhou.aliyuncs.com)
#   ANOLISA_UPDATE_URL   CLI release manifest URL (default: derived from mirror)
#   ANOLISA_INSTALL_DIR  binary install directory (default: ~/.local/bin)

set -euo pipefail

VERSION="${ANOLISA_VERSION:-stable}"
MIRROR="${ANOLISA_MIRROR:-https://anolisa.oss-cn-hangzhou.aliyuncs.com}"
UPDATE_URL="${ANOLISA_UPDATE_URL:-${MIRROR}/anolisa-releases/anolisa/v1/cli/release-manifest.toml}"
INSTALL_DIR="${ANOLISA_INSTALL_DIR:-$HOME/.local/bin}"
TMPDIR_INSTALL=""
STAGED_BINARY=""
MANIFEST_SCHEMA=""
RESOLVED_VERSION=""
ARTIFACT_URL=""
ARTIFACT_SHA256=""

log()  { printf '\033[1;32m%s\033[0m %s\n' "==>" "$*"; }
warn() { printf '\033[1;33m%s\033[0m %s\n' "warn:" "$*" >&2; }
err()  { printf '\033[1;31m%s\033[0m %s\n' "error:" "$*" >&2; exit 1; }

cleanup() {
  [ -z "$TMPDIR_INSTALL" ] || rm -rf "$TMPDIR_INSTALL"
  [ -z "$STAGED_BINARY" ] || rm -f "$STAGED_BINARY"
}

detect_platform() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Linux)  OS="linux";  MANIFEST_OS="linux" ;;
    Darwin) OS="darwin"; MANIFEST_OS="macos" ;;
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

resolve_stable_release() {
  local manifest_file="${TMPDIR_INSTALL}/release-manifest.toml"
  local record

  log "resolving stable release for ${MANIFEST_OS}/${ARCH_SHORT}"
  if ! curl -fsSL --connect-timeout 15 --max-time 60 \
    -o "$manifest_file" "$UPDATE_URL"; then
    err "failed to download release manifest from ${UPDATE_URL}"
  fi

  if ! record="$(
    awk -v wanted_os="$MANIFEST_OS" -v wanted_arch="$ARCH_SHORT" '
      function value(line) {
        sub(/^[^=]*=[[:space:]]*/, "", line)
        sub(/[[:space:]]*$/, "", line)
        sub(/^"/, "", line)
        sub(/"$/, "", line)
        return line
      }

      function emit() {
        if (!found &&
            artifact_os == wanted_os &&
            artifact_arch == wanted_arch &&
            artifact_url != "" &&
            artifact_sha256 != "") {
          print schema_version "\t" release_version "\t" \
            artifact_url "\t" artifact_sha256
          found = 1
        }
      }

      /^[[:space:]]*#/ || /^[[:space:]]*$/ {
        next
      }

      !in_artifact && /^[[:space:]]*schema_version[[:space:]]*=/ {
        schema_version = value($0)
        next
      }

      !in_artifact && /^[[:space:]]*version[[:space:]]*=/ {
        release_version = value($0)
        next
      }

      /^[[:space:]]*\[\[artifacts\]\][[:space:]]*$/ {
        emit()
        if (found) {
          exit 0
        }
        in_artifact = 1
        artifact_os = ""
        artifact_arch = ""
        artifact_url = ""
        artifact_sha256 = ""
        next
      }

      in_artifact && /^[[:space:]]*os[[:space:]]*=/ \
        { artifact_os = value($0); next }
      in_artifact && /^[[:space:]]*arch[[:space:]]*=/ \
        { artifact_arch = value($0); next }
      in_artifact && /^[[:space:]]*url[[:space:]]*=/ \
        { artifact_url = value($0); next }
      in_artifact && /^[[:space:]]*sha256[[:space:]]*=/ \
        { artifact_sha256 = value($0); next }

      END {
        emit()
        if (!found) {
          exit 1
        }
      }
    ' "$manifest_file"
  )"; then
    err "release manifest has no artifact for ${MANIFEST_OS}/${ARCH_SHORT}"
  fi

  IFS="$(printf '\t')" read -r \
    MANIFEST_SCHEMA RESOLVED_VERSION ARTIFACT_URL ARTIFACT_SHA256 <<< "$record"

  [ "$MANIFEST_SCHEMA" = "1" ] ||
    err "unsupported release manifest schema: ${MANIFEST_SCHEMA:-missing}"
  [ -n "$RESOLVED_VERSION" ] ||
    err "release manifest does not declare a version"
  case "$ARTIFACT_URL" in
    https://*) ;;
    *) err "release manifest contains an unsupported artifact URL" ;;
  esac
  if [ "${#ARTIFACT_SHA256}" -ne 64 ]; then
    err "release manifest contains an invalid SHA256 digest"
  fi
  case "$ARTIFACT_SHA256" in
    *[!0-9a-fA-F]*) err "release manifest contains an invalid SHA256 digest" ;;
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

  TMPDIR_INSTALL="$(mktemp -d)"
  trap cleanup EXIT

  local artifact release_dir label tar_url sha_url expected_sha
  expected_sha=""
  if [ "$VERSION" = "stable" ]; then
    resolve_stable_release
    artifact="${ARTIFACT_URL##*/}"
    artifact="${artifact%%\?*}"
    tar_url="$ARTIFACT_URL"
    expected_sha="$ARTIFACT_SHA256"
    label="$RESOLVED_VERSION"
  else
    artifact="anolisa-cli-${VERSION}-${TARGET}.tar.gz"
    release_dir="$VERSION"
    label="$VERSION"
    local base_url="${MIRROR}/anolisa-releases/anolisa/v1/cli/releases/${release_dir}/artifacts/${OS}/${ARCH_SHORT}"
    tar_url="${base_url}/${artifact}"
    sha_url="${tar_url}.sha256.txt"
  fi

  log "installing anolisa ${label} (${TARGET})"

  log "downloading ${artifact}"
  if ! curl -fSL --connect-timeout 15 --max-time 300 --progress-bar \
    -o "${TMPDIR_INSTALL}/${artifact}" "$tar_url"; then
    err "download failed — check version/platform or set ANOLISA_MIRROR"
  fi

  log "verifying checksum"
  if [ -n "$expected_sha" ]; then
    sha256_verify "${TMPDIR_INSTALL}/${artifact}" "$expected_sha"
  elif expected_sha="$(
    curl -fsSL --connect-timeout 15 --max-time 60 "$sha_url" 2>/dev/null |
      awk '{print $1}'
  )"; then
    sha256_verify "${TMPDIR_INSTALL}/${artifact}" "$expected_sha"
  else
    warn "checksum file not available, skipping verification"
  fi

  log "extracting binary"
  tar -xzf "${TMPDIR_INSTALL}/${artifact}" -C "$TMPDIR_INSTALL"

  mkdir -p "$INSTALL_DIR"
  STAGED_BINARY="$(mktemp "${INSTALL_DIR}/.anolisa.XXXXXX")"
  install -m 0755 "${TMPDIR_INSTALL}/anolisa" "$STAGED_BINARY"

  local installed_version
  if ! installed_version="$("$STAGED_BINARY" --version 2>&1)"; then
    err "downloaded binary failed validation: ${installed_version}"
  fi
  if [ -n "$RESOLVED_VERSION" ] &&
    [ "$installed_version" != "anolisa ${RESOLVED_VERSION}" ]; then
    err "downloaded binary version does not match release manifest"
  fi

  mv -f "$STAGED_BINARY" "${INSTALL_DIR}/anolisa"
  STAGED_BINARY=""
  log "installed to ${INSTALL_DIR}/anolisa"

  case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
      warn "${INSTALL_DIR} is not in your PATH"
      echo "    add it with:  export PATH=\"${INSTALL_DIR}:\$PATH\""
      ;;
  esac

  log "$installed_version"
  log "done"
}

main
