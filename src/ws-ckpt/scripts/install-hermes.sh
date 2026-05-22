#!/bin/bash

set -euo pipefail

# shellcheck source=lib-discover.sh
source "$(dirname "$0")/lib-discover.sh"

PLUGIN_DST="${HOME}/.hermes/plugins/ws-ckpt"
SKILL_DST="${HOME}/.hermes/skills/ws-ckpt"

# 1. Try plugin install (preferred)
if PLUGIN_SRC=$(find_plugin_src hermes); then
    mkdir -p "$(dirname "$PLUGIN_DST")"
    ln -sfn "$PLUGIN_SRC" "$PLUGIN_DST"
    echo "hermes ws-ckpt plugin linked: $PLUGIN_DST -> $PLUGIN_SRC"
    exit 0
fi

# 2. Fallback to skill install
if SKILL_SRC=$(find_skill_src); then
    mkdir -p "$SKILL_DST"
    cp -pr "$SKILL_SRC"/. "$SKILL_DST/"
    echo "skill installed to $SKILL_DST (from $SKILL_SRC)"
else
    print_search_error
    exit 1
fi
