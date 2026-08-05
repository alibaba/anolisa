#!/usr/bin/env bash
# Validates the portable source-level contract for AgentSight RPM payloads.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
failures=0

require_literal() {
    local file="$1"
    local expected="$2"

    if ! grep -Fq -- "$expected" "$repo_root/$file"; then
        printf 'missing %s in %s\n' "$expected" "$file" >&2
        failures=$((failures + 1))
    fi
}

require_count() {
    local file="$1"
    local expected="$2"
    local count="$3"
    local actual

    actual="$(grep -Fc -- "$expected" "$repo_root/$file" || true)"
    if [[ "$actual" != "$count" ]]; then
        printf 'expected %s occurrences of %s in %s, found %s\n' \
            "$count" "$expected" "$file" "$actual" >&2
        failures=$((failures + 1))
    fi
}

require_in_order() {
    local file="$1"
    local first="$2"
    local second="$3"
    local third="$4"
    local first_line second_line third_line

    first_line="$(grep -Fn -- "$first" "$repo_root/$file" | head -n 1 | cut -d: -f1 || true)"
    second_line="$(grep -Fn -- "$second" "$repo_root/$file" | head -n 1 | cut -d: -f1 || true)"
    third_line="$(grep -Fn -- "$third" "$repo_root/$file" | head -n 1 | cut -d: -f1 || true)"
    if [[ -z "$first_line" || -z "$second_line" || -z "$third_line" \
        || "$first_line" -ge "$second_line" || "$second_line" -ge "$third_line" ]]; then
        printf 'expected installation order in %s: %s, %s, %s\n' \
            "$file" "$first" "$second" "$third" >&2
        failures=$((failures + 1))
    fi
}

for builder in src/agentsight/scripts/rpm-build.sh scripts/rpm-build.sh .github/workflows/_rpm-build.yaml; do
    require_literal "$builder" "./scripts/build-enforcer.sh"
    require_literal "$builder" "target/release/agentsight-enforcer"
    require_literal "$builder" "agentsight-enforcer.service"
done

require_literal src/agentsight/agentsight.spec.in \
    "install -p -m 0755 agentsight-enforcer %{buildroot}/usr/local/bin/"
require_literal src/agentsight/agentsight.spec.in \
    "install -p -m 0644 agentsight-enforcer.service %{buildroot}%{_unitdir}/"
require_literal src/agentsight/agentsight.spec.in "%{_unitdir}/agentsight-enforcer.service"
require_literal src/agentsight/agentsight.spec.in "%systemd_post agentsight-enforcer.service"
require_literal src/agentsight/agentsight.spec.in "%systemd_preun agentsight-enforcer.service"
require_literal src/agentsight/agentsight.spec.in "%systemd_postun agentsight-enforcer.service"
require_literal src/agentsight/scripts/agentsight.service "Wants=agentsight-enforcer.service"
require_literal src/agentsight/scripts/agentsight-enforcer.service "PartOf=agentsight.service"
require_literal src/agentsight/scripts/agentsight.service "UMask=0077"
require_literal src/agentsight/scripts/agentsight-enforcer.service "UMask=0077"

require_literal src/anolisa/manifests/components/agentsight/component.toml \
    'source = "bin/agentsight-enforcer"'
require_literal src/anolisa/manifests/components/agentsight/component.toml \
    'target = "{bindir}/agentsight-enforcer"'
require_literal src/anolisa/manifests/components/agentsight/component.toml \
    'source = "share/anolisa/agentsight/agentsight-enforcer.service"'
require_literal src/anolisa/manifests/components/agentsight/component.toml \
    'target = "{unitdir}/agentsight-enforcer.service"'
require_literal src/anolisa/manifests/components/agentsight/component.toml \
    'unit = "agentsight-enforcer.service"'

require_count src/agentsight/tests/security_pipeline.rs \
    "let required_subscription = client.subscribe_required().expect(\"subscribe required\");" 2
require_count src/agentsight/tests/security_pipeline.rs \
    ".apply(request.clone(), required_subscription.subscription_id())" 2

for document in docs/QUICKSTART.md docs/QUICKSTART_zh.md src/agentsight/README.md src/agentsight/README_zh.md; do
    require_literal "$document" "agentsight-enforcer"
done
require_in_order src/agentsight/README.md \
    "### Install with Anolisa" "### Install via RPM" "### Build from Source"
require_in_order src/agentsight/README_zh.md \
    "### 通过 Anolisa 安装" "### 通过 RPM 安装" "### 从源码构建"

if (( failures > 0 )); then
    exit 1
fi

printf 'AgentSight RPM packaging contract passed.\n'
