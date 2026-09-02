#!/usr/bin/env bash
set -euo pipefail

TEST_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$TEST_DIR/.." && pwd)"

# build-all.sh guards main when sourced, which lets these tests exercise the
# preflight without building components or mutating the host package state.
# shellcheck source=../scripts/build-all.sh
source "$PROJECT_ROOT/scripts/build-all.sh"

TEST_TMP="$(mktemp -d)"
trap 'rm -rf "$TEST_TMP"' EXIT

fail() {
    echo "not ok - $*" >&2
    return 1
}

assert_contains() {
    local file="$1" expected="$2"
    grep -Fq -- "$expected" "$file" || fail "expected '$expected' in output"
}

assert_not_contains() {
    local file="$1" unexpected="$2"
    if grep -Fq -- "$unexpected" "$file"; then
        fail "did not expect '$unexpected' in output"
    fi
}

declare -a TEST_DEPENDENCIES=()
declare -A PRESENT_DEPENDENCIES=()

selected_runtime_dependencies() {
    printf '%s\n' "${TEST_DEPENDENCIES[@]}"
}

runtime_dependency_present() {
    local name="$1"
    RUNTIME_DEP_DETAIL="missing test dependency"
    [[ "${PRESENT_DEPENDENCIES[$name]:-false}" == "true" ]]
}

id() {
    if [[ "${1:-}" == "-u" ]]; then
        echo 1000
    else
        command id "$@"
    fi
}

as_root() {
    AS_ROOT_CALLS=$((AS_ROOT_CALLS + 1))
    AS_ROOT_HISTORY+="$*"$'\n'
    if [[ "$*" == "apt-get update" ]]; then
        [[ "$APT_UPDATE_RESULT" == "success" ]]
        return
    fi
    AS_ROOT_ARGS="$*"
    if [[ "$INSTALL_RESULT" == "success" ]]; then
        local record name
        for record in "${TEST_DEPENDENCIES[@]}"; do
            IFS='|' read -r _ name _ <<< "$record"
            PRESENT_DEPENDENCIES[$name]=true
        done
        return 0
    fi
    [[ "$INSTALL_RESULT" != "error" ]]
}

reset_preflight_stubs() {
    INSTALL_MODE="user"
    INSTALL_DEPS=true
    DO_INSTALL=true
    DEPS_ONLY=false
    DRY_RUN=false
    PKG_BASE="deb"
    PKG_INSTALL="apt-get install -y"
    COMPONENTS=(sec-core tokenless ws-ckpt)
    AS_ROOT_CALLS=0
    AS_ROOT_ARGS=""
    AS_ROOT_HISTORY=""
    APT_UPDATE_RESULT="success"
    INSTALL_RESULT="none"
    TEST_STATUS=0
    TEST_OUTPUT="$TEST_TMP/output"
    RUNTIME_SYSTEM_PATH="$TEST_TMP/system-bin"
    rm -rf "$RUNTIME_SYSTEM_PATH"
    mkdir -p "$RUNTIME_SYSTEM_PATH"
    : > "$TEST_OUTPUT"
    TEST_DEPENDENCIES=(
        'sec-core|bubblewrap|system-package|bwrap --version|bubblewrap|bubblewrap||||'
        'tokenless|bash|system-package|bash --version|bash|bash||||'
        'ws-ckpt|rsync|system-package|rsync --version|rsync|rsync||||'
    )
    PRESENT_DEPENDENCIES=()
}

run_preflight() {
    set +e
    preflight_runtime_dependencies > "$TEST_OUTPUT" 2>&1
    TEST_STATUS=$?
    set -e
}

test_manifest_parser_covers_component_dependencies() {
    reset_preflight_stubs
    local output="$TEST_TMP/manifests"
    : > "$output"
    local component manifest
    for component in cosh skills sec-core cosh-ng tokenless ws-ckpt memory sight; do
        manifest="$(runtime_manifest_path "$component")"
        runtime_dependencies_for_manifest "$component" "$manifest" >> "$output"
    done

    assert_contains "$output" 'cosh|node|language-runtime|node --version|nodejs|nodejs||>=20|'
    assert_contains "$output" 'sec-core|bubblewrap|system-package|bwrap --version|bubblewrap|bubblewrap|||'
    assert_not_contains "$output" 'libssl1.1'
    assert_contains "$output" 'tokenless|python3|system-package|python3 --version|python3|python3|||'
    assert_contains "$output" 'ws-ckpt|btrfs-progs|system-package|mkfs.btrfs --version|btrfs-progs|btrfs-progs|||'
    assert_contains "$output" 'ws-ckpt|rsync|system-package|rsync --version|rsync|rsync|||'
    assert_contains "$output" 'ws-ckpt|btrfs|platform-capability||||btrfs||5.4'
    assert_contains "$output" 'sight|ebpf-btf|platform-capability||||btf||5.8'
    [[ "$(wc -l < "$output")" -eq 14 ]] || fail "unexpected manifest dependency count"

    local source_dependency
    COMPONENTS=(cosh-ng)
    source_dependency="$(source_build_runtime_dependencies)"
    [[ -z "$source_dependency" ]] || \
        fail "cosh-ng source build no longer requires OpenSSL headers"

    source_dependency="$(runtime_dependency_for_source_build \
        'sec-core|nodejs|system-package|node --version|nodejs|nodejs||||')"
    [[ "$source_dependency" == \
        'sec-core|node|language-runtime|node --version|nodejs|nodejs||>=20|' ]] || \
        fail "sec-core source Node dependency was not versioned"

    source_dependency="$(runtime_dependency_for_source_build \
        'sec-core|systemd|system-package|systemctl --version|systemd|systemd||||')"
    [[ -z "$source_dependency" ]] || \
        fail "sec-core source dependencies retained packaged systemd"

    COMPONENTS=(sight)
    source_dependency="$(source_build_runtime_dependencies)"
    [[ "$source_dependency" == \
        'sight|node|language-runtime|node --version|nodejs|nodejs||>=20|' ]] || \
        fail "agentsight source Node dependency was not collected"
}

test_user_skips_ws_ckpt_noop_install_dependencies() {
    reset_preflight_stubs
    local user_output="$TEST_TMP/user-install-dependencies"
    local system_output="$TEST_TMP/system-install-dependencies"

    bash -c '
        source "$1"
        INSTALL_MODE=user
        COMPONENTS=()
        selected_runtime_dependencies
    ' bash "$PROJECT_ROOT/scripts/build-all.sh" > "$user_output"

    assert_not_contains "$user_output" 'ws-ckpt|'
    assert_contains "$user_output" 'cosh|node|language-runtime|node --version'

    bash -c '
        source "$1"
        INSTALL_MODE=system
        COMPONENTS=(ws-ckpt)
        selected_runtime_dependencies
    ' bash "$PROJECT_ROOT/scripts/build-all.sh" > "$system_output"

    assert_contains "$system_output" \
        'ws-ckpt|btrfs-progs|system-package|mkfs.btrfs --version'
    assert_contains "$system_output" 'ws-ckpt|btrfs|platform-capability'
}

test_manifest_parser_uses_toml_keys_not_order() {
    reset_preflight_stubs
    local manifest="$TEST_TMP/reordered-component.toml"
    local output="$TEST_TMP/reordered-dependencies"
    printf '%s\n' \
        '[[component.dependencies]] # reordered but equivalent' \
        'packages={ deb = '\''gnupg'\'',rpm="gnupg2" }' \
        'probe = '\''gpg --version'\''' \
        'kind="system-package"' \
        'name = "gnupg"' \
        > "$manifest"

    runtime_dependencies_for_manifest sec-core "$manifest" > "$output"

    assert_contains "$output" 'sec-core|gnupg|system-package|gpg --version|gnupg2|gnupg|||'
    [[ "$(wc -l < "$output")" -eq 1 ]] || fail "reordered TOML emitted extra records"
}

test_user_reports_all_components_once_without_root() {
    reset_preflight_stubs
    TEST_DEPENDENCIES+=(
        'cosh-ng|bash|system-package|bash --version|bash|bash||||'
        'ws-ckpt|btrfs|platform-capability||||btrfs||5.4|'
    )

    run_preflight

    [[ $TEST_STATUS -ne 0 ]] || fail "user preflight unexpectedly succeeded"
    [[ $AS_ROOT_CALLS -eq 0 ]] || fail "user preflight invoked as_root"
    assert_contains "$TEST_OUTPUT" 'sec-core: bubblewrap [system-package]'
    assert_contains "$TEST_OUTPUT" 'tokenless: bash [system-package]'
    assert_contains "$TEST_OUTPUT" 'cosh-ng: bash [system-package]'
    assert_contains "$TEST_OUTPUT" 'ws-ckpt: btrfs [platform-capability]'
    assert_contains "$TEST_OUTPUT" 'sudo apt-get install -y bubblewrap bash rsync'
    [[ "$(grep -o ' apt-get install ' "$TEST_OUTPUT" | wc -l)" -eq 1 ]] || \
        fail "expected one aggregated install command"
}

test_system_installs_packages_once_and_reprobes() {
    reset_preflight_stubs
    INSTALL_MODE="system"
    INSTALL_RESULT="success"

    run_preflight

    [[ $TEST_STATUS -eq 0 ]] || fail "system preflight did not recover"
    [[ $AS_ROOT_CALLS -eq 2 ]] || fail "expected one APT refresh and package transaction"
    [[ "$AS_ROOT_HISTORY" == \
        $'apt-get update\napt-get install -y bubblewrap bash rsync\n' ]] || \
        fail "unexpected APT transaction order: $AS_ROOT_HISTORY"
    [[ "$AS_ROOT_ARGS" == 'apt-get install -y bubblewrap bash rsync' ]] || \
        fail "unexpected package transaction: $AS_ROOT_ARGS"
    assert_contains "$TEST_OUTPUT" 'installed and verified'
}

test_system_stops_before_packages_for_platform_blocker() {
    reset_preflight_stubs
    INSTALL_MODE="system"
    INSTALL_RESULT="success"
    TEST_DEPENDENCIES+=(
        'sight|ebpf-btf|platform-capability||||btf||5.8|'
    )

    run_preflight

    [[ $TEST_STATUS -ne 0 ]] || fail "platform blocker unexpectedly succeeded"
    [[ $AS_ROOT_CALLS -eq 0 ]] || fail "packages changed before platform validation"
    assert_contains "$TEST_OUTPUT" 'sight: ebpf-btf [platform-capability]'
}

test_system_reprobe_failure_reports_every_dependency() {
    reset_preflight_stubs
    INSTALL_MODE="system"
    INSTALL_RESULT="none"

    run_preflight

    [[ $TEST_STATUS -ne 0 ]] || fail "failed re-probe unexpectedly succeeded"
    [[ $AS_ROOT_CALLS -eq 2 ]] || fail "expected one APT refresh and package transaction"
    assert_contains "$TEST_OUTPUT" 'still missing after package installation'
    assert_contains "$TEST_OUTPUT" 'sec-core: bubblewrap'
    assert_contains "$TEST_OUTPUT" 'ws-ckpt: rsync'
}

test_system_apt_update_failure_stops_before_install() {
    reset_preflight_stubs
    INSTALL_MODE="system"
    INSTALL_RESULT="success"
    APT_UPDATE_RESULT="error"

    run_preflight

    [[ $TEST_STATUS -ne 0 ]] || fail "failed APT refresh unexpectedly continued"
    [[ $AS_ROOT_CALLS -eq 1 ]] || fail "package install ran after failed APT refresh"
    [[ "$AS_ROOT_HISTORY" == $'apt-get update\n' ]] || \
        fail "unexpected commands after failed APT refresh: $AS_ROOT_HISTORY"
    assert_contains "$TEST_OUTPUT" \
        'Failed to refresh APT package indexes; no runtime packages were installed.'
}

test_unknown_package_manager_reports_aggregate() {
    reset_preflight_stubs
    PKG_BASE=""
    PKG_INSTALL=""
    detect_runtime_package_manager() { return 1; }

    run_preflight

    [[ $TEST_STATUS -ne 0 ]] || fail "unknown package manager unexpectedly succeeded"
    [[ $AS_ROOT_CALLS -eq 0 ]] || fail "unknown package manager invoked as_root"
    assert_contains "$TEST_OUTPUT" 'sec-core: bubblewrap'
    assert_contains "$TEST_OUTPUT" 'ws-ckpt: rsync'
    assert_contains "$TEST_OUTPUT" 'Cannot determine a supported deb/rpm package manager'
    assert_not_contains "$TEST_OUTPUT" 'Install them with:'
}

test_rpm_report_uses_manifest_package_names() {
    reset_preflight_stubs
    PKG_BASE="rpm"
    PKG_INSTALL="dnf install -y"
    COMPONENTS=(sec-core sight)
    TEST_DEPENDENCIES=(
        'sec-core|gnupg|system-package|gpg --version|gnupg2|gnupg||||'
        'sight|elfutils-libelf|system-package|grep -aqF libelf.so.1 /etc/ld.so.cache|elfutils-libelf|libelf1||||'
    )

    run_preflight

    [[ $TEST_STATUS -ne 0 ]] || fail "rpm user preflight unexpectedly succeeded"
    [[ $AS_ROOT_CALLS -eq 0 ]] || fail "rpm user preflight invoked as_root"
    assert_contains "$TEST_OUTPUT" 'sudo dnf install -y gnupg2 elfutils-libelf'
}

test_ignore_deps_never_installs_packages() {
    reset_preflight_stubs
    INSTALL_MODE="system"
    INSTALL_DEPS=false

    run_preflight

    [[ $TEST_STATUS -ne 0 ]] || fail "--ignore-deps preflight unexpectedly succeeded"
    [[ $AS_ROOT_CALLS -eq 0 ]] || fail "--ignore-deps invoked as_root"
    assert_contains "$TEST_OUTPUT" '--ignore-deps'
}

test_system_node_rejects_user_nvm_fallback() {
    reset_preflight_stubs
    local user_bin="$TEST_TMP/user-bin"
    mkdir -p "$user_bin"
    printf '#!/bin/bash\necho v24.15.0\n' > "$user_bin/node"
    chmod +x "$user_bin/node"
    printf '#!/bin/bash\necho v18.19.0\n' > "$RUNTIME_SYSTEM_PATH/node"
    chmod +x "$RUNTIME_SYSTEM_PATH/node"
    PATH="$user_bin:$PATH"

    INSTALL_MODE="system"
    if runtime_probe_succeeds node 'node --version' '>=20'; then
        fail "system preflight accepted the installing user's nvm Node"
    fi

    INSTALL_MODE="user"
    runtime_probe_succeeds node 'node --version' '>=20' || \
        fail "user preflight did not preserve the user-local Node priority"
}

test_system_old_repo_node_is_manual_blocker() {
    reset_preflight_stubs
    INSTALL_MODE="system"
    INSTALL_RESULT="success"
    TEST_DEPENDENCIES=(
        'cosh|node|language-runtime|node --version|nodejs|nodejs||>=20|'
        'sec-core|bubblewrap|system-package|bwrap --version|bubblewrap|bubblewrap||||'
    )
    query_repo_ver() { echo 18.19.0; }

    run_preflight

    [[ $TEST_STATUS -ne 0 ]] || fail "old repository Node unexpectedly passed preflight"
    [[ $AS_ROOT_CALLS -eq 0 ]] || fail "packages changed before manual runtime was resolved"
    assert_contains "$TEST_OUTPUT" 'cosh: node [language-runtime]'
    assert_contains "$TEST_OUTPUT" 'sudo apt-get install -y bubblewrap'
    assert_not_contains "$TEST_OUTPUT" 'apt-get install -y nodejs'
    assert_contains "$TEST_OUTPUT" 'node >=20 in'
}

test_system_language_runtime_never_auto_installs() {
    reset_preflight_stubs
    INSTALL_MODE="system"
    INSTALL_RESULT="success"
    TEST_DEPENDENCIES=(
        'cosh|node|language-runtime|node --version|nodejs|nodejs||>=20|'
        'sec-core|bubblewrap|system-package|bwrap --version|bubblewrap|bubblewrap||||'
    )
    REPO_QUERY_CALLS=0
    query_repo_ver() {
        REPO_QUERY_CALLS=$((REPO_QUERY_CALLS + 1))
        echo 24.19.0
    }

    run_preflight

    [[ $TEST_STATUS -ne 0 ]] || fail "language runtime was auto-installed"
    [[ $AS_ROOT_CALLS -eq 0 ]] || fail "packages changed before manual runtime was resolved"
    [[ $REPO_QUERY_CALLS -eq 0 ]] || fail "preflight tried to select a Node repository version"
    assert_contains "$TEST_OUTPUT" 'sudo apt-get install -y bubblewrap'
    assert_not_contains "$TEST_OUTPUT" 'apt-get install -y nodejs'
}

test_install_node_system_does_not_fall_back_to_nvm() {
    reset_preflight_stubs
    INSTALL_MODE="system"
    query_repo_ver() { echo 18.19.0; }
    _configure_npm_mirror() { :; }

    set +e
    ( install_node ) > "$TEST_OUTPUT" 2>&1
    TEST_STATUS=$?
    set -e

    [[ $TEST_STATUS -ne 0 ]] || fail "old system Node repository unexpectedly succeeded"
    assert_contains "$TEST_OUTPUT" 'System Node.js >= 20.0.0 is required'
    assert_not_contains "$TEST_OUTPUT" 'Installing Node.js via nvm'
    assert_not_contains "$TEST_OUTPUT" 'NodeSource'
}

test_install_node_user_installs_node24_with_nvm() {
    reset_preflight_stubs
    HOME="$TEST_TMP/node-user"
    SHELL="/bin/bash"
    mkdir -p "$HOME"
    NODE_INSTALLED=false
    NVM_ARGS=""
    node() {
        $NODE_INSTALLED || return 127
        echo v24.19.0
    }
    npm() { echo 11.9.0; }
    nvm() {
        NVM_ARGS="$*"
        [[ "$1" == "install" && "$2" == "24" ]] || return 1
        NODE_INSTALLED=true
    }
    query_repo_ver() { echo 18.19.0; }
    _configure_npm_mirror() { :; }

    install_node > "$TEST_OUTPUT" 2>&1

    [[ "$NVM_ARGS" == 'install 24' ]] || fail "unexpected nvm install: $NVM_ARGS"
    assert_contains "$TEST_OUTPUT" 'Node.js v24.19.0'
}

test_uv_python_install_mirror_defaults_to_official() (
    reset_preflight_stubs
    HOME="$TEST_TMP/uv-default-home"
    TEST_OUTPUT="$TEST_TMP/uv-default-output"
    rm -rf "$HOME"
    unset UV_PYTHON_INSTALL_MIRROR
    uv() { echo 'uv 0.11.24'; }

    _configure_uv_mirror > "$TEST_OUTPUT"

    local official="https://github.com/astral-sh/python-build-standalone/releases/download"
    [[ "$UV_PYTHON_INSTALL_MIRROR" == "$official" ]] || \
        fail "uv did not select the official Python install source"
    assert_contains "$HOME/.config/uv/uv.toml" \
        "python-install-mirror = \"$official\""
    assert_not_contains "$HOME/.config/uv/uv.toml" 'mirror.nju.edu.cn'
)

test_uv_python_install_mirror_honors_override() (
    reset_preflight_stubs
    HOME="$TEST_TMP/uv-override-home"
    TEST_OUTPUT="$TEST_TMP/uv-override-output"
    rm -rf "$HOME"
    UV_PYTHON_INSTALL_MIRROR="https://python.example.test/releases/download"
    uv() { echo 'uv 0.11.24'; }

    _configure_uv_mirror > "$TEST_OUTPUT"

    assert_contains "$HOME/.config/uv/uv.toml" \
        'python-install-mirror = "https://python.example.test/releases/download"'
)

test_uv_python_install_mirror_migrates_managed_legacy_config() (
    reset_preflight_stubs
    HOME="$TEST_TMP/uv-legacy-home"
    TEST_OUTPUT="$TEST_TMP/uv-legacy-output"
    local uv_cfg="$HOME/.config/uv/uv.toml"
    mkdir -p "$(dirname "$uv_cfg")"
    unset UV_PYTHON_INSTALL_MIRROR
    uv() { echo 'uv 0.11.24'; }
    {
        echo '# uv configuration — managed by build-all.sh'
        echo 'python-install-mirror = "https://mirror.nju.edu.cn/github-release/astral-sh/python-build-standalone"'
        echo '[[index]]'
        echo 'url = "https://mirrors.aliyun.com/pypi/simple/"'
        echo 'default = true'
    } > "$uv_cfg"

    _configure_uv_mirror > "$TEST_OUTPUT"

    assert_contains "$uv_cfg" \
        'python-install-mirror = "https://github.com/astral-sh/python-build-standalone/releases/download"'
    assert_not_contains "$uv_cfg" 'mirror.nju.edu.cn'
    assert_contains "$TEST_OUTPUT" 'uv Python install mirror migrated'
)

test_uv_python_install_mirror_preserves_user_config() (
    reset_preflight_stubs
    HOME="$TEST_TMP/uv-user-config-home"
    TEST_OUTPUT="$TEST_TMP/uv-user-config-output"
    local uv_cfg="$HOME/.config/uv/uv.toml"
    mkdir -p "$(dirname "$uv_cfg")"
    unset UV_PYTHON_INSTALL_MIRROR
    uv() { echo 'uv 0.11.24'; }
    {
        echo '# user-owned uv configuration'
        echo 'python-install-mirror = "https://python.example.test/custom"'
    } > "$uv_cfg"

    _configure_uv_mirror > "$TEST_OUTPUT"

    assert_contains "$uv_cfg" \
        'python-install-mirror = "https://python.example.test/custom"'
    assert_not_contains "$TEST_OUTPUT" 'uv Python install mirror migrated'
)

test_system_package_probe_ignores_user_path() {
    reset_preflight_stubs
    local user_bin="$TEST_TMP/user-bin"
    mkdir -p "$user_bin"
    printf '#!/bin/bash\necho jq-1.7\n' > "$user_bin/jq"
    chmod +x "$user_bin/jq"
    PATH="$user_bin:$PATH"

    INSTALL_MODE="system"
    if runtime_probe_succeeds jq 'jq --version' ''; then
        fail "system package probe accepted jq from the user's PATH"
    fi
}

test_language_runtime_version_is_enforced() {
    reset_preflight_stubs
    local node="$RUNTIME_SYSTEM_PATH/node"
    local user_bin="$TEST_TMP/user-bin"
    mkdir -p "$user_bin"
    printf '#!/bin/bash\necho v18.20.0\n' > "$user_bin/node"
    chmod +x "$user_bin/node"
    PATH="$user_bin:$PATH"
    printf '#!/bin/bash\necho v18.20.0\n' > "$node"
    chmod +x "$node"
    INSTALL_MODE="system"
    if runtime_probe_succeeds node 'node --version' '>=20'; then
        fail "Node 18 unexpectedly satisfied >=20"
    fi
    printf '#!/bin/bash\necho v20.1.0\n' > "$node"
    runtime_probe_succeeds node 'node --version' '>=20' || \
        fail "Node 20 did not satisfy >=20"
}

test_btrfs_module_probe_uses_system_path() {
    reset_preflight_stubs
    RUNTIME_PROC_FILESYSTEMS="$TEST_TMP/filesystems"
    : > "$RUNTIME_PROC_FILESYSTEMS"
    local user_bin="$TEST_TMP/user-bin"
    rm -rf "$user_bin"
    mkdir -p "$user_bin"
    PATH="$user_bin:/usr/bin:/bin"
    local modprobe="$RUNTIME_SYSTEM_PATH/modprobe"
    printf '#!/bin/bash\nexit 0\n' > "$modprobe"
    chmod +x "$modprobe"

    runtime_btrfs_available || fail "loadable btrfs module was rejected"
    printf '#!/bin/bash\nexit 1\n' > "$modprobe"
    if runtime_btrfs_available; then
        fail "unavailable btrfs capability unexpectedly succeeded"
    fi
}

test_btrfs_progs_probe_includes_system_sbin() {
    reset_preflight_stubs
    local user_bin="$TEST_TMP/user-bin"
    rm -rf "$user_bin"
    mkdir -p "$user_bin"
    printf '#!/bin/bash\necho btrfs-progs v6.6\n' > "$user_bin/btrfs"
    chmod +x "$user_bin/btrfs"
    PATH="$user_bin:/usr/bin:/bin"

    if runtime_probe_succeeds btrfs-progs 'mkfs.btrfs --version' ''; then
        fail "btrfs-progs probe passed without mkfs.btrfs"
    fi

    printf '#!/bin/bash\necho mkfs.btrfs, part of btrfs-progs v6.6\n' > \
        "$RUNTIME_SYSTEM_PATH/mkfs.btrfs"
    chmod +x "$RUNTIME_SYSTEM_PATH/mkfs.btrfs"
    runtime_probe_succeeds btrfs-progs 'mkfs.btrfs --version' '' || \
        fail "system mkfs.btrfs was hidden by the restricted user PATH"
}

test_manifest_load_failure_is_not_silently_ignored() {
    reset_preflight_stubs
    selected_runtime_dependencies() { return 1; }

    run_preflight

    [[ $TEST_STATUS -ne 0 ]] || fail "manifest load failure unexpectedly succeeded"
    assert_contains "$TEST_OUTPUT" 'Failed to load runtime dependency manifests'
}

test_deps_only_runs_runtime_preflight() {
    reset_preflight_stubs
    COMPONENTS=(tokenless)
    DEPS_ONLY=true
    RUNTIME_PREFLIGHT_CALLS=0
    detect_distro() { :; }
    install_rust() { :; }
    install_just() { :; }
    preflight_runtime_dependencies() {
        RUNTIME_PREFLIGHT_CALLS=$((RUNTIME_PREFLIGHT_CALLS + 1))
        RUNTIME_PREFLIGHT_FILTERS+="${1:-all} "
    }
    RUNTIME_PREFLIGHT_FILTERS=""

    do_install_deps > "$TEST_OUTPUT" 2>&1

    [[ $RUNTIME_PREFLIGHT_CALLS -eq 2 ]] || \
        fail "user deps-only did not run two-phase preflight"
    [[ "$RUNTIME_PREFLIGHT_FILTERS" == 'platform-only all ' ]] || \
        fail "unexpected user preflight order: $RUNTIME_PREFLIGHT_FILTERS"

    INSTALL_MODE="system"
    RUNTIME_PREFLIGHT_CALLS=0
    RUNTIME_PREFLIGHT_FILTERS=""
    do_install_deps > "$TEST_OUTPUT" 2>&1
    [[ $RUNTIME_PREFLIGHT_CALLS -eq 1 && "$RUNTIME_PREFLIGHT_FILTERS" == 'all ' ]] || \
        fail "system deps-only did not preflight once before setup"
}

test_retry_command_preserves_mode_and_location() {
    reset_preflight_stubs
    COMPONENTS=(memory)
    INSTALL_MODE="system"
    DEPS_ONLY=true
    local retry
    retry="$(cd "$PROJECT_ROOT/src/agent-memory" && runtime_retry_command)"
    retry="${retry% }"
    [[ "$retry" == \
        "$PROJECT_ROOT/scripts/build-all.sh --component memory --system --deps-only" ]] || \
        fail "deps-only retry is not reproducible: $retry"

    DEPS_ONLY=false
    INSTALL_DEPS=false
    retry="$(cd "$PROJECT_ROOT/src/agent-memory" && runtime_retry_command)"
    retry="${retry% }"
    [[ "$retry" == \
        "$PROJECT_ROOT/scripts/build-all.sh --component memory --system --ignore-deps" ]] || \
        fail "ignore-deps retry is not reproducible: $retry"
}

test_user_source_dependency_setup_precedes_full_preflight() {
    reset_preflight_stubs
    COMPONENTS=(cosh)
    TEST_DEPENDENCIES=(
        'cosh|node|language-runtime|node --version|nodejs|nodejs||>=20|'
    )
    query_repo_ver() { echo 18.19.0; }
    detect_distro() { :; }
    install_node() {
        echo SOURCE_DEP_ACTION
        PRESENT_DEPENDENCIES[node]=true
    }
    install_build_tools() { :; }

    set +e
    do_install_deps > "$TEST_OUTPUT" 2>&1
    TEST_STATUS=$?
    set -e

    [[ $TEST_STATUS -eq 0 ]] || fail "user Node setup did not satisfy preflight"
    assert_contains "$TEST_OUTPUT" SOURCE_DEP_ACTION
    assert_contains "$TEST_OUTPUT" 'runtime dependencies are available'
    local setup_line preflight_line
    setup_line="$(grep -n -m1 SOURCE_DEP_ACTION "$TEST_OUTPUT" | cut -d: -f1)"
    preflight_line="$(grep -n -m1 'Runtime dependency preflight' "$TEST_OUTPUT" | cut -d: -f1)"
    (( setup_line < preflight_line )) || fail "full preflight preceded user Node setup"
}

test_system_satisfied_preflight_precedes_source_dependency_setup() {
    reset_preflight_stubs
    COMPONENTS=(cosh)
    TEST_DEPENDENCIES=(
        'cosh|node|language-runtime|node --version|nodejs|nodejs||>=20|'
    )
    PRESENT_DEPENDENCIES[node]=true
    INSTALL_MODE="system"
    detect_distro() { :; }
    install_node() { echo SOURCE_DEP_ACTION; }
    install_build_tools() { :; }

    do_install_deps > "$TEST_OUTPUT" 2>&1

    assert_contains "$TEST_OUTPUT" 'runtime dependencies are available'
    assert_contains "$TEST_OUTPUT" SOURCE_DEP_ACTION
    local setup_line preflight_line
    preflight_line="$(grep -n -m1 'Runtime dependency preflight' "$TEST_OUTPUT" | cut -d: -f1)"
    setup_line="$(grep -n -m1 SOURCE_DEP_ACTION "$TEST_OUTPUT" | cut -d: -f1)"
    (( preflight_line < setup_line )) || fail "source dependency setup preceded preflight"
}

test_platform_preflight_precedes_dependency_changes() {
    reset_preflight_stubs
    COMPONENTS=(sight)
    INSTALL_MODE="system"
    TEST_DEPENDENCIES=(
        'sight|ebpf-btf|platform-capability||||btf||5.8|'
    )
    detect_distro() { :; }
    install_node() { echo SOURCE_DEP_ACTION; }
    install_build_tools() { echo SOURCE_DEP_ACTION; }
    install_rust() { echo SOURCE_DEP_ACTION; }
    check_ebpf_deps() { echo SOURCE_DEP_ACTION; }

    set +e
    do_install_deps > "$TEST_OUTPUT" 2>&1
    TEST_STATUS=$?
    set -e

    [[ $TEST_STATUS -ne 0 ]] || fail "platform blocker unexpectedly succeeded"
    [[ $AS_ROOT_CALLS -eq 0 ]] || fail "platform blocker changed runtime packages"
    assert_contains "$TEST_OUTPUT" 'sight: ebpf-btf [platform-capability]'
    assert_not_contains "$TEST_OUTPUT" SOURCE_DEP_ACTION
}

test_no_install_skips_runtime_preflight() {
    reset_preflight_stubs
    COMPONENTS=(tokenless)
    DO_INSTALL=false
    DEPS_ONLY=false
    RUNTIME_PREFLIGHT_CALLS=0
    detect_distro() { :; }
    install_rust() { :; }
    install_just() { :; }
    preflight_runtime_dependencies() {
        RUNTIME_PREFLIGHT_CALLS=$((RUNTIME_PREFLIGHT_CALLS + 1))
    }

    do_install_deps > "$TEST_OUTPUT" 2>&1

    [[ $RUNTIME_PREFLIGHT_CALLS -eq 0 ]] || fail "build-only ran preflight"
}

test_preflight_failure_precedes_first_install() {
    reset_preflight_stubs
    COMPONENTS=(skills)
    preflight_runtime_dependencies() {
        echo PREFLIGHT_FAILED
        return 1
    }
    install_skills() { echo INSTALL_ACTION; }

    set +e
    do_install > "$TEST_OUTPUT" 2>&1
    TEST_STATUS=$?
    set -e

    [[ $TEST_STATUS -ne 0 ]] || fail "install continued after preflight failure"
    assert_contains "$TEST_OUTPUT" PREFLIGHT_FAILED
    assert_not_contains "$TEST_OUTPUT" INSTALL_ACTION
}

test_ignore_deps_skips_install_preflight() {
    reset_preflight_stubs
    COMPONENTS=(skills)
    INSTALL_DEPS=false
    RUNTIME_PREFLIGHT_CALLS=0
    preflight_runtime_dependencies() {
        RUNTIME_PREFLIGHT_CALLS=$((RUNTIME_PREFLIGHT_CALLS + 1))
        return 1
    }
    install_skills() { echo INSTALL_ACTION; }

    do_install > "$TEST_OUTPUT" 2>&1

    [[ $RUNTIME_PREFLIGHT_CALLS -eq 0 ]] || \
        fail "--ignore-deps unexpectedly ran runtime preflight"
    assert_contains "$TEST_OUTPUT" 'Skipping runtime dependency verification (--ignore-deps)'
    assert_contains "$TEST_OUTPUT" INSTALL_ACTION
}

test_dry_run_skips_host_preflight() {
    reset_preflight_stubs
    COMPONENTS=(skills)
    DRY_RUN=true
    RUNTIME_PREFLIGHT_CALLS=0
    preflight_runtime_dependencies() {
        RUNTIME_PREFLIGHT_CALLS=$((RUNTIME_PREFLIGHT_CALLS + 1))
    }
    install_skills() { echo INSTALL_ACTION; }

    do_install > "$TEST_OUTPUT" 2>&1

    [[ $RUNTIME_PREFLIGHT_CALLS -eq 0 ]] || fail "dry-run probed the host"
    assert_contains "$TEST_OUTPUT" 'host probes skipped'
    assert_contains "$TEST_OUTPUT" INSTALL_ACTION
    local preflight_line install_line
    preflight_line="$(grep -n -m1 'host probes skipped' "$TEST_OUTPUT" | cut -d: -f1)"
    install_line="$(grep -n -m1 INSTALL_ACTION "$TEST_OUTPUT" | cut -d: -f1)"
    (( preflight_line < install_line )) || fail "dry-run listed install before preflight"
}

run_test() {
    local name="$1" status
    set +e
    ( set -e; "$name" )
    status=$?
    set -e
    if [[ $status -eq 0 ]]; then
        echo "ok - $name"
    else
        echo "not ok - $name" >&2
        return 1
    fi
}

run_test test_manifest_parser_covers_component_dependencies
run_test test_user_skips_ws_ckpt_noop_install_dependencies
run_test test_manifest_parser_uses_toml_keys_not_order
run_test test_user_reports_all_components_once_without_root
run_test test_system_installs_packages_once_and_reprobes
run_test test_system_stops_before_packages_for_platform_blocker
run_test test_system_reprobe_failure_reports_every_dependency
run_test test_system_apt_update_failure_stops_before_install
run_test test_unknown_package_manager_reports_aggregate
run_test test_rpm_report_uses_manifest_package_names
run_test test_ignore_deps_never_installs_packages
run_test test_system_node_rejects_user_nvm_fallback
run_test test_system_old_repo_node_is_manual_blocker
run_test test_system_language_runtime_never_auto_installs
run_test test_install_node_system_does_not_fall_back_to_nvm
run_test test_install_node_user_installs_node24_with_nvm
run_test test_uv_python_install_mirror_defaults_to_official
run_test test_uv_python_install_mirror_honors_override
run_test test_uv_python_install_mirror_migrates_managed_legacy_config
run_test test_uv_python_install_mirror_preserves_user_config
run_test test_system_package_probe_ignores_user_path
run_test test_language_runtime_version_is_enforced
run_test test_btrfs_module_probe_uses_system_path
run_test test_btrfs_progs_probe_includes_system_sbin
run_test test_manifest_load_failure_is_not_silently_ignored
run_test test_deps_only_runs_runtime_preflight
run_test test_retry_command_preserves_mode_and_location
run_test test_user_source_dependency_setup_precedes_full_preflight
run_test test_system_satisfied_preflight_precedes_source_dependency_setup
run_test test_platform_preflight_precedes_dependency_changes
run_test test_no_install_skips_runtime_preflight
run_test test_preflight_failure_precedes_first_install
run_test test_ignore_deps_skips_install_preflight
run_test test_dry_run_skips_host_preflight
