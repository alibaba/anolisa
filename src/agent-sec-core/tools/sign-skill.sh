#!/bin/bash
#
# Skill Manifest and Signature Generator
#
# This script generates .skill-meta/Manifest.json and .skill-meta/.skill.sig
# files for skill directories. It also provides helpers for first-time setup
# (GPG key generation, public key export) so that self-deployed ANOLISA
# installations can sign skills easily.
#
# Usage:
#   Single mode: ./sign-skill.sh <skill_dir> [--skill-name NAME] [--force]
#   Batch  mode: ./sign-skill.sh --batch [parent_dir] [--force]
#   Init   mode: ./sign-skill.sh --init [--trusted-keys-dir DIR]
#   Export key:  ./sign-skill.sh --export-key [DIR]
#   Check deps:  ./sign-skill.sh --check
#
# In batch mode, <parent_dir> is scanned and every immediate subdirectory is
# treated as an individual skill directory to sign.
#
# Init mode generates a local GPG signing key (if none exists) and exports the
# public key to the verifier's trusted-keys directory.
#
# Environment Variables:
#   GPG_PRIVATE_KEY  - ASCII-armored GPG private key used for signing.
#                      The key will be imported into the local GPG keyring
#                      automatically before signing. Typically provided in CI/CD
#                      environments where the keyring is not pre-configured.
#   GPG_PASSPHRASE   - Passphrase for the GPG key (optional).
#
# Full guide: tools/SIGNING_GUIDE.md | tools/SIGNING_GUIDE_CN.md
#

set -e

MANIFEST_FILENAME="Manifest.json"
SIGNATURE_FILENAME=".skill.sig"
SIGNING_DIR=".skill-meta"
HASH_ALGORITHM="SHA256"
MANIFEST_VERSION="0.1"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m' # No Color

# Fixed signing identity (not user-configurable)
SIGN_KEY_EMAIL="anolisa-deploy@$(hostname -s 2>/dev/null || echo localhost)"
SIGN_KEY_NAME="ANOLISA Local Deploy Key"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Default path for deployed skills
DEFAULT_SKILLS_DIR="$HOME/.copilot-shell/skills"

# Default path for trusted public keys (verifier reads from here)
DEFAULT_TRUSTED_KEYS_DIR="$HOME/.copilot-shell/skills/agent-sec-core/scripts/asset-verify/trusted-keys"

# Path to the deployed verifier config (inside agent-sec-core skill).
# Batch mode auto-registers the signed directory here so that the verifier
# picks it up without manual config.conf editing.
DEPLOY_CONFIG_CONF="$HOME/.copilot-shell/skills/agent-sec-core/scripts/asset-verify/config.conf"

# Resolve gpg binary: prefer 'gpg', fall back to 'gpg2' (RHEL/Alinux minimal)
if command -v gpg &>/dev/null; then
    GPG=gpg
elif command -v gpg2 &>/dev/null; then
    GPG=gpg2
else
    GPG=gpg  # will fail later with a clear error via --check
fi

# Resolved GPG key identifier used for signing.  Set after key generation
# or GPG_PRIVATE_KEY import; empty means "let gpg pick its default".
GPG_SIGN_KEY=""

# Function to compute SHA256 hash of a file
compute_file_hash() {
    local file_path="$1"
    if command -v sha256sum &> /dev/null; then
        sha256sum "$file_path" | awk '{print $1}'
    else
        shasum -a 256 "$file_path" | awk '{print $1}'
    fi
}

# Function to generate manifest JSON
generate_manifest() {
    local skill_dir="$1"
    local skill_name="$2"
    local files_array=""
    local first=true

    # Find all files, compute hashes, build JSON array
    while IFS= read -r -d '' file; do
        local rel_path="${file#$skill_dir/}"

        # Skip hidden files and files in hidden directories
        # (this also excludes .skill-meta/Manifest.json and .skill-meta/.skill.sig)
        local skip=false
        local check_path="$rel_path"
        while [[ "$check_path" != "." ]] && [[ "$check_path" != "/" ]] && [[ -n "$check_path" ]]; do
            local part=$(basename "$check_path")
            if [[ "$part" == .* ]]; then
                skip=true
                break
            fi
            check_path=$(dirname "$check_path")
        done

        if [[ "$skip" == true ]]; then
            continue
        fi

        local file_hash=$(compute_file_hash "$file")

        # Add comma if not first element
        if [[ "$first" == true ]]; then
            first=false
        else
            files_array+=","
        fi

        # Use jq to safely encode the path and hash into JSON
        local file_entry
        file_entry=$(jq -n --arg path "$rel_path" --arg hash "$file_hash" '{path: $path, hash: $hash}')
        files_array+="$file_entry"
    done < <(find "$skill_dir" -type f -print0 | sort -z)

    local created_at=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

    # Use jq to safely construct the entire manifest JSON
    jq -n \
        --arg version "$MANIFEST_VERSION" \
        --arg skill_name "$skill_name" \
        --arg algorithm "$HASH_ALGORITHM" \
        --arg created_at "$created_at" \
        --argjson files "[$files_array]" \
        '{version: $version, skill_name: $skill_name, algorithm: $algorithm, created_at: $created_at, files: $files}'
}

# Function to sign manifest using GPG
sign_manifest() {
    local manifest_path="$1"
    local signature_path="$2"

    local cmd=("$GPG" --batch --yes --armor --detach-sign --output "$signature_path")

    # Pin signing key so the correct key is used when multiple exist
    if [[ -n "$GPG_SIGN_KEY" ]]; then
        cmd+=(--default-key "$GPG_SIGN_KEY")
    fi

    # Add passphrase if provided via environment variable
    if [[ -n "${GPG_PASSPHRASE:-}" ]]; then
        cmd+=(--pinentry-mode loopback --passphrase-fd 0)
    fi

    cmd+=("$manifest_path")

    if [[ -n "${GPG_PASSPHRASE:-}" ]]; then
        if ! "${cmd[@]}" <<<"$GPG_PASSPHRASE" 2>/dev/null; then
            echo -e "${RED}ERROR: Failed to sign manifest${NC}" >&2
            return 1
        fi
    else
        if ! "${cmd[@]}" 2>/dev/null; then
            echo -e "${RED}ERROR: Failed to sign manifest${NC}" >&2
            return 1
        fi
    fi

    return 0
}

# Ensure a skills directory is registered in the deployed config.conf.
# This must be called BEFORE signing agent-sec-core, because config.conf
# is part of that skill and will be included in its manifest hash.
ensure_config_dir_entry() {
    local dir_to_add="$1"
    local config_file="$DEPLOY_CONFIG_CONF"

    if [[ ! -f "$config_file" ]]; then
        echo -e "${YELLOW}NOTE: config.conf not found at $config_file — skipping auto-register${NC}"
        return 0
    fi

    # Already registered?  Use awk to check only inside the skills_dir
    # array for an exact (whitespace-trimmed) match, avoiding substring
    # false positives from grep -F.
    if awk -v target="$dir_to_add" '
        /skills_dir[[:space:]]*=/ { in_list=1; next }
        in_list && /^[[:space:]]*\]/ { exit 1 }
        in_list {
            line=$0; gsub(/^[[:space:]]+|[[:space:],]+$/, "", line)
            if (line == target) { found=1; exit 0 }
        }
        END { exit (found ? 0 : 1) }
    ' "$config_file" 2>/dev/null; then
        echo "Skills directory already in config.conf: $dir_to_add"
        return 0
    fi

    # Preserve original file permissions across the temp-file swap.
    local orig_mode
    orig_mode=$(stat -c '%a' "$config_file" 2>/dev/null) \
        || orig_mode=$(stat -f '%Lp' "$config_file" 2>/dev/null) \
        || orig_mode=""

    # Insert entry before the first ']' (end of skills_dir array).
    # The pattern tolerates an optionally-indented closing bracket.
    local tmp_file
    tmp_file=$(mktemp)
    awk -v entry="    $dir_to_add" '
        /^[[:space:]]*\]/ && !done { print entry; done=1 }
        { print }
    ' "$config_file" > "$tmp_file" && mv "$tmp_file" "$config_file"

    # Restore original permissions if we captured them.
    if [[ -n "$orig_mode" ]]; then
        chmod "$orig_mode" "$config_file" 2>/dev/null
    fi

    if grep -qF "$dir_to_add" "$config_file" 2>/dev/null; then
        echo -e "${GREEN}Added skills directory to config.conf: $dir_to_add${NC}"
    else
        echo -e "${YELLOW}WARNING: Could not update config.conf — please add '$dir_to_add' manually${NC}"
    fi
}

# Function to show usage
show_usage() {
    echo -e "${BOLD}Skill Manifest and Signature Generator${NC}"
    echo ""
    echo "Usage:"
    echo "  $0 <skill_dir> [--skill-name NAME] [--force]"
    echo "  $0 --batch [parent_dir] [--force]"
    echo "  $0 --init [--trusted-keys-dir DIR]"
    echo "  $0 --export-key [DIR]"
    echo "  $0 --check"
    echo ""
    echo "Modes:"
    echo "  (default)           Sign a single skill directory"
    echo "  --batch [DIR]       Sign every subdirectory under DIR"
    echo "                      (default: $DEFAULT_SKILLS_DIR)"
    echo "  --init              One-time setup: generate GPG key + export public key"
    echo "  --export-key [DIR]  Export signing public key to DIR"
    echo "                      (default: $DEFAULT_TRUSTED_KEYS_DIR)"
    echo "  --check             Check prerequisites (gpg, jq) and exit"
    echo ""
    echo "Options:"
    echo "  --skill-name NAME       Skill name (defaults to directory name)"
    echo "  --force                 Overwrite existing manifest and signature files"
    echo "  --trusted-keys-dir DIR  Where to export the public key (used with --init)"
    echo "                          (default: $DEFAULT_TRUSTED_KEYS_DIR)"
    echo "  -h, --help              Show this help message"
    echo ""
    echo "Quick Start (self-deployment):"
    echo "  $0 --init"
    echo "  $0 --batch --force"
    echo "  python3 /path/to/verifier.py"
    echo ""
    echo "Environment Variables:"
    echo "  GPG_PRIVATE_KEY   ASCII-armored GPG private key (for CI/CD auto-import)"
    echo "  GPG_PASSPHRASE    Passphrase for the GPG key (optional)"
    echo ""
    echo "Full guide: ${SCRIPT_DIR}/SIGNING_GUIDE.md"
}

# Sign a single skill directory. Accepts: skill_dir, skill_name, force
sign_single_skill() {
    local skill_dir="$1"
    local skill_name="$2"
    local force="$3"

    # Resolve absolute path
    skill_dir=$(cd "$skill_dir" 2>/dev/null && pwd) || true

    if [[ ! -d "$skill_dir" ]]; then
        echo -e "${RED}ERROR: Skill directory does not exist: $skill_dir${NC}" >&2
        return 1
    fi

    # Set default skill name
    if [[ -z "$skill_name" ]]; then
        skill_name=$(basename "$skill_dir")
    fi

    local signing_dir="$skill_dir/$SIGNING_DIR"
    local manifest_path="$signing_dir/$MANIFEST_FILENAME"
    local signature_path="$signing_dir/$SIGNATURE_FILENAME"

    # Check if files already exist
    if [[ "$force" == false ]]; then
        if [[ -f "$manifest_path" ]]; then
            echo -e "${YELLOW}WARNING: $SIGNING_DIR/$MANIFEST_FILENAME already exists in $skill_name. Use --force to overwrite.${NC}"
            return 1
        fi
        if [[ -f "$signature_path" ]]; then
            echo -e "${YELLOW}WARNING: $SIGNING_DIR/$SIGNATURE_FILENAME already exists in $skill_name. Use --force to overwrite.${NC}"
            return 1
        fi
    fi

    echo "Generating manifest for skill: $skill_name"
    echo "Skill directory: $skill_dir"

    # Ensure .skill-meta directory exists
    mkdir -p "$signing_dir"

    # Generate and save manifest
    generate_manifest "$skill_dir" "$skill_name" > "$manifest_path"
    echo -e "  ${GREEN}[CREATED]${NC} $SIGNING_DIR/$MANIFEST_FILENAME"

    # Sign manifest
    if sign_manifest "$manifest_path" "$signature_path"; then
        echo -e "  ${GREEN}[CREATED]${NC} $SIGNING_DIR/$SIGNATURE_FILENAME"
    else
        echo -e "  ${RED}[ERROR]${NC} Failed to create $SIGNING_DIR/$SIGNATURE_FILENAME"
        return 1
    fi

    return 0
}

# ─── prerequisite checks ───

check_prerequisites() {
    local ok=true

    echo "Checking prerequisites ..."

    if command -v "$GPG" &>/dev/null; then
        local gpg_ver
        gpg_ver=$($GPG --version 2>/dev/null | head -1)
        echo -e "  ${GREEN}[OK]${NC} $gpg_ver ($GPG)"
    else
        echo -e "  ${RED}[MISSING]${NC} gpg / gpg2 (gnupg2)"
        ok=false
    fi

    if command -v jq &>/dev/null; then
        local jq_ver
        jq_ver=$(jq --version 2>/dev/null || echo "jq")
        echo -e "  ${GREEN}[OK]${NC} $jq_ver"
    else
        echo -e "  ${RED}[MISSING]${NC} jq"
        ok=false
    fi

    if command -v sha256sum &>/dev/null; then
        echo -e "  ${GREEN}[OK]${NC} sha256sum"
    elif command -v shasum &>/dev/null; then
        echo -e "  ${GREEN}[OK]${NC} shasum"
    else
        echo -e "  ${RED}[MISSING]${NC} sha256sum / shasum"
        ok=false
    fi

    if [[ "$ok" == false ]]; then
        echo ""
        echo -e "${YELLOW}Install missing packages:${NC}"
        echo "  RHEL / Anolis / Alinux:  sudo yum install -y gnupg2 jq coreutils"
        echo "  Debian / Ubuntu:         sudo apt-get install -y gnupg jq coreutils"
        return 1
    fi

    echo -e "${GREEN}All prerequisites satisfied.${NC}"
    return 0
}

# ─── init: one-time signing setup ───

do_init() {
    local trusted_keys_dir="${1:-$DEFAULT_TRUSTED_KEYS_DIR}"

    echo ""
    echo -e "${BOLD}================================${NC}"
    echo -e "${BOLD} Skill Signing Setup (--init)${NC}"
    echo -e "${BOLD}================================${NC}"
    echo ""

    # 1. Prerequisites
    if ! check_prerequisites; then
        exit 1
    fi
    echo ""

    # 2. Generate GPG key if needed
    if "$GPG" --list-secret-keys "$SIGN_KEY_EMAIL" &>/dev/null 2>&1; then
        echo -e "${GREEN}GPG signing key already exists for: $SIGN_KEY_EMAIL${NC}"
    else
        echo "Generating GPG signing key ..."
        echo "  Name:  $SIGN_KEY_NAME"
        echo "  Email: $SIGN_KEY_EMAIL"
        echo ""

        "$GPG" --batch --gen-key <<GPGEOF
Key-Type: RSA
Key-Length: 4096
Name-Real: $SIGN_KEY_NAME
Name-Email: $SIGN_KEY_EMAIL
Expire-Date: 2y
%no-protection
%commit
GPGEOF

        if "$GPG" --list-secret-keys "$SIGN_KEY_EMAIL" &>/dev/null 2>&1; then
            echo -e "${GREEN}GPG key generated successfully.${NC}"
        else
            echo -e "${RED}ERROR: Failed to generate GPG key.${NC}" >&2
            exit 1
        fi
    fi

    # 3. Pin the signing key for subsequent operations
    GPG_SIGN_KEY="$SIGN_KEY_EMAIL"

    # 4. Export public key to trusted-keys directory
    echo ""
    do_export_key "$trusted_keys_dir"

    # 5. Print next steps
    echo ""
    echo -e "${BOLD}================================${NC}"
    echo -e "${BOLD} Setup Complete${NC}"
    echo -e "${BOLD}================================${NC}"
    echo ""
    echo "Next steps:"
    echo ""
    echo "  1. Sign your skills (batch):"
    echo "     $0 --batch <skills_dir> --force"
    echo ""
    echo "  2. Verify signatures:"
    echo "     agent-sec-cli verify"
    echo ""
}

# ─── export public key ───

do_export_key() {
    local output_dir="${1:-$DEFAULT_TRUSTED_KEYS_DIR}"

    if ! "$GPG" --list-secret-keys "$SIGN_KEY_EMAIL" &>/dev/null 2>&1; then
        echo -e "${RED}ERROR: No GPG secret key found for '$SIGN_KEY_EMAIL'.${NC}" >&2
        echo "Run '$0 --init' first to generate a signing key." >&2
        return 1
    fi

    mkdir -p "$output_dir"

    local safe_name
    safe_name=$(echo "$SIGN_KEY_EMAIL" | tr '@.' '-')
    local output_file="$output_dir/${safe_name}.asc"

    "$GPG" --armor --export "$SIGN_KEY_EMAIL" > "$output_file"

    if [[ -s "$output_file" ]]; then
        echo -e "${GREEN}Public key exported: $output_file${NC}"
    else
        echo -e "${RED}ERROR: Failed to export public key for $SIGN_KEY_EMAIL${NC}" >&2
        rm -f "$output_file"
        return 1
    fi
}

# ─── main ───

main() {
    local skill_dir=""
    local skill_name=""
    local force=false
    local batch=false
    local batch_dir=""
    local mode=""            # "", "init", "export-key", "check"
    local trusted_keys_dir=""
    local export_key_dir=""

    # Import GPG private key from environment variable if provided
    if [[ -n "${GPG_PRIVATE_KEY:-}" ]]; then
        # Save original shell options and disable tracing to avoid
        # exposing private key material in logs
        local _saved_opts
        _saved_opts=$(set +o)          # e.g. "set +o xtrace\nset -o errexit\n..."
        { set +x; } 2>/dev/null

        # Snapshot existing fingerprints before import
        local _fprs_before
        _fprs_before=$("$GPG" --list-secret-keys --with-colons 2>/dev/null \
            | grep '^fpr:' | cut -d':' -f10 | sort)

        echo "Importing GPG private key from environment..."
        echo "$GPG_PRIVATE_KEY" | "$GPG" --batch --import 2>/dev/null

        # Determine the fingerprint(s) that were actually added
        local _fprs_after _imported_fpr
        _fprs_after=$("$GPG" --list-secret-keys --with-colons 2>/dev/null \
            | grep '^fpr:' | cut -d':' -f10 | sort)
        _imported_fpr=$(comm -13 <(echo "$_fprs_before") <(echo "$_fprs_after") | head -1)

        # If key already existed (no diff), fall back to matching the
        # imported material directly
        if [[ -z "$_imported_fpr" ]]; then
            _imported_fpr=$(echo "$GPG_PRIVATE_KEY" \
                | "$GPG" --batch --with-colons --import-options show-only --import 2>/dev/null \
                | grep '^fpr:' | head -1 | cut -d':' -f10)
        fi

        if [[ -n "$_imported_fpr" ]]; then
            # Set ultimate trust for the imported key
            echo "$_imported_fpr:6:" | "$GPG" --import-ownertrust 2>/dev/null
            GPG_SIGN_KEY="$_imported_fpr"
            echo "GPG private key imported and trusted (fingerprint: ${_imported_fpr:0:16}...)"
        else
            echo -e "${YELLOW}WARNING: Could not determine imported key fingerprint${NC}"
        fi

        # Restore original shell options exactly
        eval "$_saved_opts" 2>/dev/null
    fi

    # Resolve signing key: if the ANOLISA deploy key exists, pin to it
    if [[ -z "$GPG_SIGN_KEY" ]] && "$GPG" --list-secret-keys "$SIGN_KEY_EMAIL" &>/dev/null 2>&1; then
        GPG_SIGN_KEY="$SIGN_KEY_EMAIL"
    fi

    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case $1 in
            --init)
                mode="init"
                shift
                ;;
            --check)
                mode="check"
                shift
                ;;
            --export-key)
                mode="export-key"
                # DIR is optional — next arg is consumed only if it doesn't look like a flag
                if [[ -n "${2:-}" && "${2:0:1}" != "-" ]]; then
                    export_key_dir="$2"
                    shift 2
                else
                    export_key_dir=""
                    shift
                fi
                ;;
            --trusted-keys-dir)
                [[ -n "${2:-}" ]] || { echo -e "${RED}ERROR: --trusted-keys-dir requires a directory${NC}" >&2; exit 1; }
                trusted_keys_dir="$2"
                shift 2
                ;;
            --batch)
                batch=true
                # Directory is optional; default to DEFAULT_SKILLS_DIR
                if [[ -n "${2:-}" && "${2:0:1}" != "-" ]]; then
                    batch_dir="$2"
                    shift 2
                else
                    batch_dir=""
                    shift
                fi
                ;;
            --skill-name)
                skill_name="$2"
                shift 2
                ;;
            --force)
                force=true
                shift
                ;;
            -h|--help)
                show_usage
                exit 0
                ;;
            -*)
                echo -e "${RED}ERROR: Unknown option $1${NC}" >&2
                show_usage
                exit 1
                ;;
            *)
                if [[ -z "$skill_dir" ]]; then
                    skill_dir="$1"
                else
                    echo -e "${RED}ERROR: Multiple skill directories specified${NC}" >&2
                    show_usage
                    exit 1
                fi
                shift
                ;;
        esac
    done

    # ── Mode dispatch ──

    if [[ "$mode" == "check" ]]; then
        check_prerequisites
        exit $?
    fi

    if [[ "$mode" == "init" ]]; then
        do_init "$trusted_keys_dir"
        exit 0
    fi

    if [[ "$mode" == "export-key" ]]; then
        do_export_key "$export_key_dir"
        exit $?
    fi

    # ── Batch mode ──

    if [[ "$batch" == true ]]; then
        # Default to the standard deployed skills directory
        if [[ -z "$batch_dir" ]]; then
            batch_dir="$DEFAULT_SKILLS_DIR"
        fi

        batch_dir=$(cd "$batch_dir" 2>/dev/null && pwd) || true
        if [[ ! -d "$batch_dir" ]]; then
            echo -e "${RED}ERROR: Batch directory does not exist: $batch_dir${NC}" >&2
            exit 1
        fi

        # Register the skills directory in config.conf BEFORE signing.
        # config.conf is part of the agent-sec-core skill, so it must be
        # updated first to ensure its manifest hash is correct.
        ensure_config_dir_entry "$batch_dir"

        echo "Batch signing skills under: $batch_dir"
        echo ""

        local failed=0
        local total=0
        for subdir in "$batch_dir"/*/; do
            [[ -d "$subdir" ]] || continue
            total=$((total + 1))
            if ! sign_single_skill "$subdir" "" "$force"; then
                failed=$((failed + 1))
            fi
            echo ""
        done

        echo "Batch complete: $((total - failed))/$total skills signed successfully."
        if [[ $failed -gt 0 ]]; then
            echo -e "${RED}$failed skill(s) failed to sign.${NC}"
            exit 1
        fi

        echo "Done!"
        exit 0
    fi

    # ── Single mode ──

    if [[ -z "$skill_dir" ]]; then
        echo -e "${RED}ERROR: Skill directory not specified${NC}" >&2
        show_usage
        exit 1
    fi

    if ! sign_single_skill "$skill_dir" "$skill_name" "$force"; then
        exit 1
    fi

    echo ""
    echo "Done!"
}

main "$@"
